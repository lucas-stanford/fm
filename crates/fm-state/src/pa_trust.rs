//! pi-automations' consent, read only, so fm can warn that an automation armed in both would run
//! twice. fm never writes pa's files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::folder::{Env, ProcessEnv};
use crate::keys::{AutomationKey, normalize_path};
use crate::time::parse_iso;
use fm_model::Scope;

/// `$PI_AGENT_DIR/pi-automations/trust.json`, default `~/.pi/agent/pi-automations/trust.json`.
pub fn pa_trust_path(env: &dyn Env) -> Option<PathBuf> {
    let agent_dir = match env.var("PI_AGENT_DIR") {
        Some(dir) => normalize_path(Path::new(dir.trim())),
        None => env.home_dir()?.join(".pi").join("agent"),
    };
    Some(agent_dir.join("pi-automations").join("trust.json"))
}

/// One record of pa's trust.json: only what fm needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaRecord {
    pub key: String,
    pub expires_at: Option<String>,
}

/// pa's consent, as read. Anything unreadable is empty: pa's file is never a reason to fail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaTrust {
    pub records: BTreeMap<String, PaRecord>,
}

impl PaTrust {
    pub fn load(file: &Path) -> PaTrust {
        let Some(Value::Object(obj)) = crate::fsio::read_json::<Value>(file) else {
            return PaTrust::default();
        };
        if obj.get("version").and_then(Value::as_u64) != Some(1) {
            return PaTrust::default();
        }
        let mut records = BTreeMap::new();
        if let Some(Value::Object(recs)) = obj.get("records") {
            for (key, rec) in recs {
                let Some(rec) = rec.as_object() else { continue };
                let key = rec
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or(key)
                    .to_string();
                let expires_at = rec
                    .get("expiresAt")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                records.insert(key.clone(), PaRecord { key, expires_at });
            }
        }
        PaTrust { records }
    }

    /// Keys pa would treat as armed at `now`, content aside: repository consent past its expiry
    /// is not armed (pa ignores expiry on user consent).
    pub fn armed_keys(&self, now: DateTime<Utc>) -> BTreeSet<String> {
        self.records
            .values()
            .filter(|r| {
                let repo =
                    AutomationKey::parse(&r.key).is_some_and(|k| k.scope == Scope::Repository);
                let expired = r
                    .expires_at
                    .as_deref()
                    .and_then(parse_iso)
                    .is_some_and(|t| t <= now);
                !(repo && expired)
            })
            .map(|r| r.key.clone())
            .collect()
    }
}

/// pa's armed keys on this machine, now. Empty when pa is not installed.
pub fn pa_armed_keys() -> BTreeSet<String> {
    pa_trust_path(&ProcessEnv)
        .map(|p| PaTrust::load(&p).armed_keys(Utc::now()))
        .unwrap_or_default()
}

/// True when `fm_key` names an automation in `pa_keys`: same scope (pa's `project:` is
/// `repository:`), same root folder however spelled, same name.
pub fn is_armed_in_pa(fm_key: &str, pa_keys: &BTreeSet<String>) -> bool {
    if pa_keys.contains(fm_key) {
        return true;
    }
    let Some(ours) = AutomationKey::parse(fm_key) else {
        return false;
    };
    pa_keys
        .iter()
        .filter_map(|k| AutomationKey::parse(k))
        .any(|theirs| ours.same_as(&theirs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::FakeEnv;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap()
    }

    #[test]
    fn finds_pas_trust_file_from_pi_agent_dir_or_home() {
        let home = std::env::temp_dir().join("fm-pa-home");
        let env = FakeEnv::with_home(&home);
        assert_eq!(
            pa_trust_path(&env).unwrap(),
            home.join(".pi")
                .join("agent")
                .join("pi-automations")
                .join("trust.json")
        );
        let agent = std::env::temp_dir().join("fm-pa-agent");
        let env = env.set("PI_AGENT_DIR", agent.to_string_lossy());
        assert_eq!(
            pa_trust_path(&env).unwrap(),
            normalize_path(&agent)
                .join("pi-automations")
                .join("trust.json")
        );
        assert!(pa_trust_path(&FakeEnv::default()).is_none());
    }

    #[test]
    fn reads_pas_records_and_drops_expired_repository_consent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("trust.json");
        let root = dir.path().join("repo").join(".agents").join("automations");
        let user = format!("user:{}:daily", root.display());
        let fresh = format!("repository:{}:fresh", root.display());
        let expired = format!("project:{}:old", root.display());
        let doc = serde_json::json!({
            "version": 1,
            "records": {
                user.clone(): { "key": user, "dir": "x", "repoRoot": "", "digest": "sha256:1",
                    "armedAt": "2026-01-01T00:00:00.000Z", "lastSeenAt": "2026-01-01T00:00:00.000Z",
                    "expiresAt": "2026-01-02T00:00:00.000Z", "interpreters": {} },
                fresh.clone(): { "key": fresh, "expiresAt": "2026-10-01T00:00:00.000Z" },
                expired.clone(): { "key": expired, "expiresAt": "2026-09-01T00:00:00.000Z" },
                "junk": 5
            }
        });
        std::fs::write(&file, doc.to_string()).unwrap();
        let keys = PaTrust::load(&file).armed_keys(now());
        assert_eq!(keys, BTreeSet::from([user.clone(), fresh.clone()]));
    }

    #[test]
    fn an_absent_or_corrupt_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("trust.json");
        assert!(PaTrust::load(&file).records.is_empty());
        std::fs::write(&file, "nope").unwrap();
        assert!(PaTrust::load(&file).records.is_empty());
        std::fs::write(&file, r#"{"version":2,"records":{"a":{}}}"#).unwrap();
        assert!(PaTrust::load(&file).records.is_empty());
    }

    #[test]
    fn matches_by_scope_root_and_name() {
        let root = std::env::temp_dir()
            .join("fm-pa")
            .join(".agents")
            .join("automations");
        let fm = format!("repository:{}:x", root.display());
        let pa_project = format!("project:{}:x", root.join(".").display());
        assert!(is_armed_in_pa(&fm, &BTreeSet::from([pa_project])));
        assert!(is_armed_in_pa(&fm, &BTreeSet::from([fm.clone()])));
        let other_name = format!("repository:{}:y", root.display());
        let other_scope = format!("user:{}:x", root.display());
        assert!(!is_armed_in_pa(
            &fm,
            &BTreeSet::from([other_name, other_scope])
        ));
        assert!(!is_armed_in_pa(
            "nonsense",
            &BTreeSet::from(["x:y:z".to_string()])
        ));
    }

    #[test]
    fn never_writes_pas_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("trust.json");
        let before = r#"{"version":1,"records":{}}"#;
        std::fs::write(&file, before).unwrap();
        let _ = PaTrust::load(&file).armed_keys(now());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

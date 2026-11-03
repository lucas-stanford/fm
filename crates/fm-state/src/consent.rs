//! Consent (`trust.json`): every automation, user or repository, needs explicit consent for its
//! exact bytes.
//!
//! Several processes share trust.json: the tick, every `fm` command, the TUI and runs. Each keeps a
//! copy, and a copy can be seconds old by the time it writes. So nothing ever writes a whole copy
//! back: every change re-reads the file under its lock and applies only its own change. Otherwise
//! a tick that loaded before `fm arm` would put the old record back and the arm would be lost.
//!
//! Hashing the files is `fm-spec`'s job; this module stores and compares the hashes ([`Pin`]).

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use fm_model::{Located, Scope, StatePaths, TrustFile, TrustRecord};
use serde_json::Value;

use crate::error::{Result, StateError};
use crate::fsio::{ensure_dir, read_json, update_json};
use crate::time::{iso, parse_iso};

/// What consent is pinned to: the digest over everything, and one hash per file (`None` for a file
/// that was missing), keyed by path relative to the automation folder (`AUTOMATION.md` included).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pin {
    pub digest: String,
    pub files: BTreeMap<String, Option<String>>,
}

/// Why an automation is `off`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OffReason {
    NotArmed,
    /// Repository consent outlived `consent-expiry`.
    Expired,
    /// These files changed since it was armed, sorted.
    Changed(Vec<String>),
}

impl OffReason {
    /// One line, `<what> — <what fixes it>`.
    pub fn message(&self, name: &str) -> String {
        match self {
            OffReason::NotArmed => format!("not armed — fm arm {name}"),
            OffReason::Expired => format!("consent expired — fm arm {name}"),
            OffReason::Changed(files) => {
                format!("changed since armed: {} — fm arm {name}", files.join(", "))
            }
        }
    }
}

/// What a person consented to, beyond the pin: shown in the consent preview, kept on the record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArmShown {
    /// Script relative path -> the exact command shown.
    pub interpreters: BTreeMap<String, String>,
    /// The agent command line shown, flags included.
    pub agent_command: Option<String>,
}

/// The files that differ between the pin at consent and now, sorted. A record from before per-file
/// hashes can only say the digest moved.
pub fn changed_files(rec: &TrustRecord, now: &Pin) -> Vec<String> {
    if rec.files.is_empty() {
        return if rec.digest == now.digest {
            Vec::new()
        } else {
            vec!["AUTOMATION.md or a script".into()]
        };
    }
    let mut files: Vec<String> = rec
        .files
        .iter()
        .filter(|(rel, hash)| now.files.get(*rel) != Some(*hash))
        .map(|(rel, _)| rel.clone())
        .collect();
    files.extend(
        now.files
            .keys()
            .filter(|rel| !rec.files.contains_key(*rel))
            .cloned(),
    );
    files.sort();
    files.dedup();
    files
}

/// Lenient read: version 1 and a records object, else empty; a malformed record is dropped alone.
fn trust_from(raw: Option<Value>) -> TrustFile {
    let mut out = TrustFile {
        version: 1,
        records: BTreeMap::new(),
    };
    let Some(Value::Object(obj)) = raw else {
        return out;
    };
    if obj.get("version").and_then(Value::as_u64) != Some(1) {
        return out;
    }
    if let Some(Value::Object(records)) = obj.get("records") {
        for (key, rec) in records {
            if let Ok(rec) = serde_json::from_value::<TrustRecord>(rec.clone()) {
                out.records.insert(key.clone(), rec);
            }
        }
    }
    out
}

/// Read trust.json without a lock: what the TUI and listings use.
pub fn read_trust(paths: &StatePaths) -> TrustFile {
    trust_from(read_json(&paths.trust()))
}

/// A process's view of trust.json. Reads come from its copy; every change goes to disk at once.
#[derive(Debug)]
pub struct Consent {
    paths: StatePaths,
    data: TrustFile,
    /// Sightings not written yet, key -> ISO: merged into whatever is on disk when saved.
    seen: BTreeMap<String, String>,
}

impl Consent {
    pub fn load(paths: &StatePaths) -> Result<Consent> {
        ensure_dir(&paths.root)?;
        Ok(Consent {
            paths: paths.clone(),
            data: read_trust(paths),
            seen: BTreeMap::new(),
        })
    }

    /// Read the file again, dropping this copy's view (not its pending sightings).
    pub fn reload(&mut self) {
        self.data = read_trust(&self.paths);
    }

    pub fn file(&self) -> &TrustFile {
        &self.data
    }

    pub fn record(&self, key: &str) -> Option<&TrustRecord> {
        self.data.records.get(key)
    }

    pub fn all(&self) -> impl Iterator<Item = &TrustRecord> {
        self.data.records.values()
    }

    /// Why `l` is not armed right now, or `None` when it is. `pin` is its content now.
    pub fn off_reason(&self, l: &Located, pin: &Pin, now: DateTime<Utc>) -> Option<OffReason> {
        let Some(rec) = self.data.records.get(&l.key) else {
            return Some(OffReason::NotArmed);
        };
        // User consent never expires, even on a record written with an expiry.
        if l.root.scope == Scope::Repository
            && rec
                .expires_at
                .as_deref()
                .and_then(parse_iso)
                .is_some_and(|t| t <= now)
        {
            return Some(OffReason::Expired);
        }
        let files = changed_files(rec, pin);
        if files.is_empty() {
            None
        } else {
            Some(OffReason::Changed(files))
        }
    }

    pub fn is_armed(&self, l: &Located, pin: &Pin, now: DateTime<Utc>) -> bool {
        self.off_reason(l, pin, now).is_none()
    }

    /// Record consent for exactly `pin`. `expiry_ms` applies to repository scope only; 0 is none.
    pub fn arm(
        &mut self,
        l: &Located,
        pin: &Pin,
        shown: &ArmShown,
        now: DateTime<Utc>,
        expiry_ms: u64,
    ) -> Result<()> {
        let at = iso(now);
        let expires_at = (expiry_ms > 0 && l.root.scope == Scope::Repository).then(|| {
            iso(now + Duration::milliseconds(i64::try_from(expiry_ms).unwrap_or(i64::MAX / 2)))
        });
        let rec = TrustRecord {
            key: l.key.clone(),
            dir: l.dir.clone(),
            repo_root: l.repo_root.clone(),
            digest: pin.digest.clone(),
            armed_at: at.clone(),
            last_seen_at: at,
            expires_at,
            interpreters: shown.interpreters.clone(),
            files: pin.files.clone(),
            agent_command: shown.agent_command.clone(),
        };
        self.change(|records| {
            records.insert(rec.key.clone(), rec);
        })
    }

    /// Withdraw consent. False when there was none.
    pub fn disarm(&mut self, key: &str) -> Result<bool> {
        self.change(|records| records.remove(key).is_some())
    }

    /// Keep consent across a one-line `enabled:` edit made by `fm enable`/`fm disable`: move the
    /// pin to the new content. False when it was not armed.
    pub fn repin(&mut self, key: &str, pin: &Pin) -> Result<bool> {
        self.change(|records| match records.get_mut(key) {
            Some(rec) => {
                rec.digest = pin.digest.clone();
                rec.files = pin.files.clone();
                true
            }
            None => false,
        })
    }

    /// Note the folder still exists, so the sweep does not reclaim a live consent. Written by the
    /// next [`Consent::save`] or change.
    pub fn touch(&mut self, key: &str, now: DateTime<Utc>) {
        if let Some(rec) = self.data.records.get_mut(key) {
            rec.last_seen_at = iso(now);
            self.seen.insert(key.to_string(), rec.last_seen_at.clone());
        }
    }

    /// Write the sightings [`Consent::touch`] noted. They only move `lastSeenAt` forward on records
    /// still on disk. Best effort: a busy lock leaves them for the next save.
    pub fn save(&mut self) -> Result<()> {
        if self.seen.is_empty() {
            return Ok(());
        }
        match self.change(|_| ()) {
            Err(StateError::LockTimeout { .. }) => Ok(()),
            other => other,
        }
    }

    /// Drop consent for automations whose `AUTOMATION.md` is gone and that have not been seen for
    /// `unseen_for_ms`. Decided again on what is on disk under the lock. Returns the keys dropped.
    pub fn sweep(&mut self, now: DateTime<Utc>, unseen_for_ms: u64) -> Result<Vec<String>> {
        let limit = i64::try_from(unseen_for_ms).unwrap_or(i64::MAX);
        let reclaimable = |rec: &TrustRecord| {
            !rec.dir.join("AUTOMATION.md").exists()
                && parse_iso(&rec.last_seen_at)
                    .is_some_and(|seen| (now - seen).num_milliseconds() >= limit)
        };
        if !self.data.records.values().any(reclaimable) {
            return Ok(Vec::new());
        }
        self.change(|records| {
            let gone: Vec<String> = records
                .iter()
                .filter(|(_, r)| reclaimable(r))
                .map(|(k, _)| k.clone())
                .collect();
            for k in &gone {
                records.remove(k);
            }
            gone
        })
    }

    /// Re-read trust.json under its lock, apply this copy's pending sightings and then `edit`,
    /// write it, and keep the result as this copy. The only way anything here writes.
    fn change<R>(
        &mut self,
        edit: impl FnOnce(&mut BTreeMap<String, TrustRecord>) -> R,
    ) -> Result<R> {
        let seen = std::mem::take(&mut self.seen);
        let result = update_json(&self.paths.trust(), trust_from, |disk: &mut TrustFile| {
            for (key, at) in &seen {
                if let Some(rec) = disk.records.get_mut(key) {
                    let newer = match (parse_iso(&rec.last_seen_at), parse_iso(at)) {
                        (Some(old), Some(new)) => new > old,
                        _ => true,
                    };
                    if newer {
                        rec.last_seen_at = at.clone();
                    }
                }
            }
            edit(&mut disk.records)
        });
        match result {
            Ok((disk, out)) => {
                self.data = disk;
                Ok(out)
            }
            Err(e) => {
                self.seen = seen;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use fm_model::{Automation, Flow, Hooks, Root, Triggers};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap()
    }

    fn located(root: &Path, scope: Scope, name: &str) -> Located {
        let dir = root.join(name);
        Located {
            automation: Automation {
                name: name.into(),
                description: "d".into(),
                enabled: true,
                model: None,
                timeout: None,
                on: Triggers {
                    manual: true,
                    ..Triggers::default()
                },
                hooks: Hooks::default(),
                flow: Flow::Loose,
                body: "b".into(),
            },
            file: dir.join("AUTOMATION.md"),
            dir,
            root: Root {
                path: root.to_path_buf(),
                scope,
            },
            repo_root: (scope == Scope::Repository).then(|| root.to_path_buf()),
            key: format!(
                "{}:{}:{name}",
                if scope == Scope::User {
                    "user"
                } else {
                    "repository"
                },
                root.display()
            ),
        }
    }

    fn pin(md: &str, script: Option<&str>) -> Pin {
        let mut files = BTreeMap::new();
        files.insert("AUTOMATION.md".into(), Some(format!("sha256:{md}")));
        if let Some(s) = script {
            files.insert("scripts/check.js".into(), Some(format!("sha256:{s}")));
        }
        Pin {
            digest: format!("sha256:{md}{}", script.unwrap_or("")),
            files,
        }
    }

    fn setup() -> (tempfile::TempDir, StatePaths, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path().join("state"));
        let root = dir.path().join("automations");
        (dir, paths, root)
    }

    #[test]
    fn arms_a_repository_automation_and_reports_it_armed() {
        let (_d, paths, root) = setup();
        let mut c = Consent::load(&paths).unwrap();
        let l = located(&root, Scope::Repository, "stale-pr-nudge");
        let p = pin("a", Some("b"));
        assert_eq!(c.off_reason(&l, &p, now()), Some(OffReason::NotArmed));
        assert_eq!(
            OffReason::NotArmed.message("stale-pr-nudge"),
            "not armed — fm arm stale-pr-nudge"
        );
        let shown = ArmShown {
            interpreters: [("scripts/check.js".into(), "node scripts/check.js".into())].into(),
            agent_command: Some("claude -p".into()),
        };
        c.arm(&l, &p, &shown, now(), 0).unwrap();
        assert!(c.is_armed(&l, &p, now()));
        let rec = c.record(&l.key).unwrap();
        assert_eq!(rec.agent_command.as_deref(), Some("claude -p"));
        assert_eq!(rec.armed_at, "2026-09-17T12:00:00.000Z");
    }

    #[test]
    fn holds_a_user_automation_off_until_armed_and_names_a_changed_file() {
        let (_d, paths, root) = setup();
        let mut c = Consent::load(&paths).unwrap();
        let l = located(&root, Scope::User, "daily-briefing");
        assert!(!c.is_armed(&l, &pin("a", Some("b")), now()));
        c.arm(&l, &pin("a", Some("b")), &ArmShown::default(), now(), 0)
            .unwrap();
        let reason = c.off_reason(&l, &pin("a", Some("CHANGED")), now()).unwrap();
        assert_eq!(
            reason.message("daily-briefing"),
            "changed since armed: scripts/check.js — fm arm daily-briefing"
        );
        assert_eq!(
            c.off_reason(&l, &pin("CHANGED", Some("b")), now()),
            Some(OffReason::Changed(vec!["AUTOMATION.md".into()]))
        );
        // A script added since, and one removed since, are named too.
        assert_eq!(
            c.off_reason(&l, &pin("a", None), now()),
            Some(OffReason::Changed(vec!["scripts/check.js".into()]))
        );
    }

    #[test]
    fn an_old_record_without_file_hashes_can_only_say_the_digest_moved() {
        let rec = TrustRecord {
            key: "k".into(),
            dir: PathBuf::from("d"),
            repo_root: None,
            digest: "sha256:x".into(),
            armed_at: iso(now()),
            last_seen_at: iso(now()),
            expires_at: None,
            interpreters: BTreeMap::new(),
            files: BTreeMap::new(),
            agent_command: None,
        };
        let same = Pin {
            digest: "sha256:x".into(),
            files: pin("a", None).files,
        };
        assert!(changed_files(&rec, &same).is_empty());
        let moved = Pin {
            digest: "sha256:y".into(),
            ..same
        };
        assert_eq!(changed_files(&rec, &moved).len(), 1);
    }

    #[test]
    fn expires_repository_consent_and_never_user_consent() {
        let (_d, paths, root) = setup();
        let mut c = Consent::load(&paths).unwrap();
        let repo = located(&root, Scope::Repository, "docs-drift");
        let user = located(&root, Scope::User, "daily-briefing");
        let p = pin("a", None);
        let thirty = 30 * 86_400_000;
        c.arm(&repo, &p, &ArmShown::default(), now(), thirty)
            .unwrap();
        c.arm(&user, &p, &ArmShown::default(), now(), thirty)
            .unwrap();
        assert!(c.record(&user.key).unwrap().expires_at.is_none());
        let later = now() + Duration::days(31);
        assert_eq!(c.off_reason(&repo, &p, later), Some(OffReason::Expired));
        assert_eq!(
            OffReason::Expired.message("docs-drift"),
            "consent expired — fm arm docs-drift"
        );
        assert!(c.is_armed(&repo, &p, now() + Duration::days(29)));
        assert!(c.is_armed(&user, &p, now() + Duration::days(3650)));
    }

    #[test]
    fn ignores_an_expiry_on_a_user_record_and_treats_zero_as_none() {
        let (_d, paths, root) = setup();
        let user = located(&root, Scope::User, "u");
        let repo = located(&root, Scope::Repository, "r");
        let p = pin("a", None);
        let mut c = Consent::load(&paths).unwrap();
        c.arm(&user, &p, &ArmShown::default(), now(), 0).unwrap();
        c.arm(&repo, &p, &ArmShown::default(), now(), 0).unwrap();
        assert!(c.record(&repo.key).unwrap().expires_at.is_none());
        // An expiry on a user record, as an earlier build might write: ignored.
        crate::fsio::update_json(&paths.trust(), trust_from, |t| {
            t.records.get_mut(&user.key).unwrap().expires_at = Some(iso(now()));
        })
        .unwrap();
        c.reload();
        assert!(c.is_armed(&user, &p, now() + Duration::days(1)));
        assert!(c.is_armed(&repo, &p, now() + Duration::days(10_000)));
    }

    #[test]
    fn disarms_and_persists_across_instances() {
        let (_d, paths, root) = setup();
        let l = located(&root, Scope::Repository, "docs-drift");
        let p = pin("a", None);
        let shown = ArmShown {
            interpreters: [("a".into(), "b".into())].into(),
            agent_command: None,
        };
        Consent::load(&paths)
            .unwrap()
            .arm(&l, &p, &shown, now(), 0)
            .unwrap();
        let mut again = Consent::load(&paths).unwrap();
        assert!(again.is_armed(&l, &p, now()));
        assert_eq!(
            again.record(&l.key).unwrap().interpreters,
            shown.interpreters
        );
        assert!(again.disarm(&l.key).unwrap());
        assert!(!again.disarm(&l.key).unwrap());
        assert!(!Consent::load(&paths).unwrap().is_armed(&l, &p, now()));
    }

    #[test]
    fn repin_keeps_consent_across_an_enabled_edit_only_when_armed() {
        let (_d, paths, root) = setup();
        let l = located(&root, Scope::User, "x");
        let mut c = Consent::load(&paths).unwrap();
        assert!(!c.repin(&l.key, &pin("b", None)).unwrap());
        c.arm(&l, &pin("a", None), &ArmShown::default(), now(), 0)
            .unwrap();
        assert!(c.repin(&l.key, &pin("b", None)).unwrap());
        assert!(c.is_armed(&l, &pin("b", None), now()));
    }

    #[test]
    fn survives_a_corrupt_trust_json() {
        let (_d, paths, root) = setup();
        std::fs::create_dir_all(&paths.root).unwrap();
        std::fs::write(paths.trust(), "{ not json").unwrap();
        let mut c = Consent::load(&paths).unwrap();
        assert_eq!(c.all().count(), 0);
        // And the next arm writes a good file over it.
        let l = located(&root, Scope::User, "x");
        c.arm(&l, &pin("a", None), &ArmShown::default(), now(), 0)
            .unwrap();
        assert_eq!(read_trust(&paths).records.len(), 1);
    }

    #[test]
    fn drops_one_malformed_record_and_keeps_the_rest() {
        let (_d, paths, root) = setup();
        let l = located(&root, Scope::User, "x");
        let mut c = Consent::load(&paths).unwrap();
        c.arm(&l, &pin("a", None), &ArmShown::default(), now(), 0)
            .unwrap();
        let mut raw: Value = read_json(&paths.trust()).unwrap();
        raw["records"]["broken"] = serde_json::json!({ "key": 5 });
        std::fs::write(paths.trust(), raw.to_string()).unwrap();
        let t = read_trust(&paths);
        assert_eq!(t.records.keys().collect::<Vec<_>>(), vec![&l.key]);
    }

    #[test]
    fn sweeps_only_records_whose_folder_is_gone_and_unseen() {
        let (_d, paths, root) = setup();
        let gone = located(&root, Scope::Repository, "docs-drift");
        let stays = located(&root, Scope::Repository, "release-notes");
        std::fs::create_dir_all(&stays.dir).unwrap();
        std::fs::write(stays.dir.join("AUTOMATION.md"), "x").unwrap();
        let mut c = Consent::load(&paths).unwrap();
        let p = pin("a", None);
        c.arm(&gone, &p, &ArmShown::default(), now(), 0).unwrap();
        c.arm(&stays, &p, &ArmShown::default(), now(), 0).unwrap();
        let month = 30 * 86_400_000;
        assert!(
            c.sweep(now() + Duration::seconds(1), month)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            c.sweep(now() + Duration::days(31), month).unwrap(),
            vec![gone.key.clone()]
        );
        assert!(c.record(&stays.key).is_some());
    }

    #[test]
    fn keeps_an_arm_made_elsewhere_while_an_older_copy_saves_sightings() {
        let (_d, paths, root) = setup();
        let l = located(&root, Scope::User, "daily-briefing");
        Consent::load(&paths)
            .unwrap()
            .arm(&l, &pin("a", None), &ArmShown::default(), now(), 0)
            .unwrap();
        // A tick loads, then the person edits and arms again.
        let mut tick = Consent::load(&paths).unwrap();
        assert!(!tick.is_armed(&l, &pin("b", None), now()));
        Consent::load(&paths)
            .unwrap()
            .arm(&l, &pin("b", None), &ArmShown::default(), now(), 0)
            .unwrap();
        let later = now() + Duration::minutes(1);
        tick.touch(&l.key, later);
        tick.save().unwrap();

        let after = Consent::load(&paths).unwrap();
        assert!(after.is_armed(&l, &pin("b", None), now()));
        assert_eq!(after.record(&l.key).unwrap().last_seen_at, iso(later));
        // The older copy now holds what it wrote, not its stale view.
        assert!(tick.is_armed(&l, &pin("b", None), now()));
    }

    #[test]
    fn never_brings_back_a_record_disarmed_elsewhere_nor_drops_one_armed_elsewhere() {
        let (_d, paths, root) = setup();
        let user = located(&root, Scope::User, "daily-briefing");
        let repo = located(&root, Scope::Repository, "docs-drift");
        let p = pin("a", None);
        Consent::load(&paths)
            .unwrap()
            .arm(&user, &p, &ArmShown::default(), now(), 0)
            .unwrap();
        let mut stale = Consent::load(&paths).unwrap();
        Consent::load(&paths).unwrap().disarm(&user.key).unwrap();
        Consent::load(&paths)
            .unwrap()
            .arm(&repo, &p, &ArmShown::default(), now(), 0)
            .unwrap();
        stale.touch(&user.key, now());
        stale.save().unwrap();
        assert!(!Consent::load(&paths).unwrap().is_armed(&user, &p, now()));
        assert!(Consent::load(&paths).unwrap().is_armed(&repo, &p, now()));

        let mut older = Consent::load(&paths).unwrap();
        Consent::load(&paths)
            .unwrap()
            .arm(&user, &p, &ArmShown::default(), now(), 0)
            .unwrap();
        older.disarm(&repo.key).unwrap();
        assert!(Consent::load(&paths).unwrap().is_armed(&user, &p, now()));
        assert!(!Consent::load(&paths).unwrap().is_armed(&repo, &p, now()));
    }

    #[test]
    fn sweeps_on_what_is_on_disk() {
        let (_d, paths, root) = setup();
        let gone = located(&root, Scope::Repository, "docs-drift");
        let p = pin("a", None);
        Consent::load(&paths)
            .unwrap()
            .arm(&gone, &p, &ArmShown::default(), now(), 0)
            .unwrap();
        let later = now() + Duration::days(31);
        let mut sweeper = Consent::load(&paths).unwrap();
        let mut seer = Consent::load(&paths).unwrap();
        seer.touch(&gone.key, later);
        seer.save().unwrap();
        assert!(sweeper.sweep(later, 30 * 86_400_000).unwrap().is_empty());
        assert!(Consent::load(&paths).unwrap().record(&gone.key).is_some());
    }

    #[test]
    fn does_not_write_when_nothing_changed() {
        let (_d, paths, _root) = setup();
        let mut c = Consent::load(&paths).unwrap();
        c.touch("user:/r:nothing", now());
        c.save().unwrap();
        assert!(!paths.trust().exists());
    }

    #[test]
    fn many_threads_arming_and_ticking_lose_no_arm() {
        let (_d, paths, root) = setup();
        let paths = Arc::new(paths);
        let names: Vec<String> = (0..24).map(|i| format!("race-{i:02}")).collect();
        let p = pin("a", None);
        let mut threads = Vec::new();
        for t in 0..4 {
            let (paths, root, p) = (paths.clone(), root.clone(), p.clone());
            let mine: Vec<String> = names
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 4 == t)
                .map(|(_, n)| n.clone())
                .collect();
            threads.push(thread::spawn(move || {
                for n in mine {
                    let l = located(&root, Scope::User, &n);
                    Consent::load(&paths)
                        .unwrap()
                        .arm(&l, &p, &ArmShown::default(), now(), 0)
                        .unwrap();
                }
            }));
        }
        for _ in 0..2 {
            let (paths, root, names) = (paths.clone(), root.clone(), names.clone());
            threads.push(thread::spawn(move || {
                for i in 0..20 {
                    let mut tick = Consent::load(&paths).unwrap();
                    for n in &names {
                        let l = located(&root, Scope::User, n);
                        tick.touch(&l.key, now() + Duration::seconds(i));
                    }
                    tick.save().unwrap();
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        let c = Consent::load(&paths).unwrap();
        let lost: Vec<_> = names
            .iter()
            .filter(|n| !c.is_armed(&located(&root, Scope::User, n), &p, now()))
            .collect();
        assert!(lost.is_empty(), "lost arms: {lost:?}");
        let tmp: Vec<_> = std::fs::read_dir(&paths.root)
            .unwrap()
            .filter_map(|e| {
                let n = e.unwrap().file_name().to_string_lossy().into_owned();
                n.ends_with(".tmp").then_some(n)
            })
            .collect();
        assert!(tmp.is_empty(), "{tmp:?}");
    }
}

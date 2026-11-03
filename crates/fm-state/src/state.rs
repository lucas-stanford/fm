//! Tick state (`state.json`): claimed instants, registered repository roots, watch checks, paused
//! watches and dismissed runs. Every change is a read-change-write under the file's lock, so the
//! tick and commands never undo each other and a claim is never lost.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fm_model::{StateFile, StatePaths, TickSource, WatchState};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::Result;
use crate::fsio::{ensure_dir, read_json, update_json};
use crate::keys::normalize_path;
use crate::time::{iso, parse_iso};

fn field<T: DeserializeOwned + Default>(obj: &serde_json::Map<String, Value>, key: &str) -> T {
    obj.get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// A map read entry by entry: one bad entry is dropped, not the whole map.
fn entries<T: DeserializeOwned>(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    keep: impl Fn(&T) -> bool,
) -> BTreeMap<String, T> {
    let Some(Value::Object(map)) = obj.get(key) else {
        return BTreeMap::new();
    };
    map.iter()
        .filter_map(|(k, v)| {
            serde_json::from_value::<T>(v.clone())
                .ok()
                .filter(|t| keep(t))
                .map(|t| (k.clone(), t))
        })
        .collect()
}

/// Lenient read: version 1, each field on its own; anything else starts clean.
pub fn state_from(raw: Option<Value>) -> StateFile {
    let clean = StateFile {
        version: 1,
        ..StateFile::default()
    };
    let Some(Value::Object(obj)) = raw else {
        return clean;
    };
    if obj.get("version").and_then(Value::as_u64) != Some(1) {
        return clean;
    }
    let roots = match obj.get("roots") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|r| r.as_str().map(PathBuf::from))
            .collect(),
        _ => Vec::new(),
    };
    let mut watches_raw = obj.clone();
    if let Some(Value::Object(ws)) = watches_raw.get_mut("watches") {
        for w in ws.values_mut().filter_map(Value::as_object_mut) {
            w.entry("durationMs").or_insert(Value::from(0));
        }
    }
    StateFile {
        version: 1,
        last_tick_at: field::<Option<String>>(&obj, "lastTickAt"),
        last_tick_by: field::<Option<TickSource>>(&obj, "lastTickBy"),
        claimed: entries::<String>(&obj, "claimed", |_| true),
        roots,
        watches: entries::<WatchState>(&watches_raw, "watches", |_| true),
        paused_watches: entries::<String>(&obj, "pausedWatches", |at| parse_iso(at).is_some()),
        dismissed_runs: entries::<String>(&obj, "dismissedRuns", |id| !id.is_empty()),
    }
}

/// Changes to a [`StateFile`], for use inside [`StateStore::update`] so several land in one write.
pub trait StateFileExt {
    /// Claim a cron instant; false when it is already claimed (two ticks never start one run).
    fn claim(&mut self, key: &str, instant: &str) -> bool;
    fn claim_of(&self, key: &str) -> Option<&str>;
    /// Add a repository root, normalised; false when already there.
    fn register_root(&mut self, root: &Path) -> bool;
    fn unregister_root(&mut self, root: &Path) -> bool;
    /// Forget claims and watch checks of automations that no longer exist. Returns the claims
    /// dropped.
    fn prune(&mut self, live: &BTreeSet<String>) -> Vec<String>;
    fn record_watch(&mut self, key: &str, check: WatchState);
    /// False when it was already paused.
    fn pause_watch(&mut self, key: &str, now: DateTime<Utc>) -> bool;
    /// False when it was not paused.
    fn resume_watch(&mut self, key: &str) -> bool;
    /// False when that run was already the dismissed one.
    fn dismiss_run(&mut self, key: &str, run_id: &str) -> bool;
    fn note_tick(&mut self, now: DateTime<Utc>, by: TickSource);
}

impl StateFileExt for StateFile {
    fn claim(&mut self, key: &str, instant: &str) -> bool {
        if self.claimed.get(key).map(String::as_str) == Some(instant) {
            return false;
        }
        self.claimed.insert(key.to_string(), instant.to_string());
        true
    }

    fn claim_of(&self, key: &str) -> Option<&str> {
        self.claimed.get(key).map(String::as_str)
    }

    fn register_root(&mut self, root: &Path) -> bool {
        let root = normalize_path(root);
        if self.roots.contains(&root) {
            return false;
        }
        self.roots.push(root);
        self.roots.sort();
        true
    }

    fn unregister_root(&mut self, root: &Path) -> bool {
        let root = normalize_path(root);
        let before = self.roots.len();
        self.roots.retain(|r| *r != root);
        self.roots.len() != before
    }

    fn prune(&mut self, live: &BTreeSet<String>) -> Vec<String> {
        let dropped: Vec<String> = self
            .claimed
            .keys()
            .filter(|k| !live.contains(*k))
            .cloned()
            .collect();
        for k in &dropped {
            self.claimed.remove(k);
        }
        self.watches.retain(|k, _| live.contains(k));
        dropped
    }

    fn record_watch(&mut self, key: &str, check: WatchState) {
        self.watches.insert(key.to_string(), check);
    }

    fn pause_watch(&mut self, key: &str, now: DateTime<Utc>) -> bool {
        if self.paused_watches.contains_key(key) {
            return false;
        }
        self.paused_watches.insert(key.to_string(), iso(now));
        true
    }

    fn resume_watch(&mut self, key: &str) -> bool {
        self.paused_watches.remove(key).is_some()
    }

    fn dismiss_run(&mut self, key: &str, run_id: &str) -> bool {
        if self.dismissed_runs.get(key).map(String::as_str) == Some(run_id) {
            return false;
        }
        self.dismissed_runs
            .insert(key.to_string(), run_id.to_string());
        true
    }

    fn note_tick(&mut self, now: DateTime<Utc>, by: TickSource) {
        self.last_tick_at = Some(iso(now));
        self.last_tick_by = Some(by);
    }
}

/// `state.json` in one state folder.
#[derive(Debug, Clone)]
pub struct StateStore {
    paths: StatePaths,
}

impl StateStore {
    pub fn new(paths: &StatePaths) -> StateStore {
        StateStore {
            paths: paths.clone(),
        }
    }

    /// What is on disk now, without a lock. A missing or corrupt file reads as a clean state.
    pub fn load(&self) -> StateFile {
        state_from(read_json(&self.paths.state()))
    }

    /// Read-change-write under the lock. Writes only when something changed.
    pub fn update<R>(&self, change: impl FnOnce(&mut StateFile) -> R) -> Result<R> {
        ensure_dir(&self.paths.root)?;
        update_json(&self.paths.state(), state_from, change).map(|(_, r)| r)
    }

    /// Claim a cron instant, for a process racing the tick. False when already claimed.
    pub fn claim(&self, key: &str, instant: &str) -> Result<bool> {
        self.update(|s| s.claim(key, instant))
    }

    pub fn register_root(&self, root: &Path) -> Result<bool> {
        self.update(|s| s.register_root(root))
    }

    pub fn unregister_root(&self, root: &Path) -> Result<bool> {
        self.update(|s| s.unregister_root(root))
    }

    pub fn prune(&self, live: &BTreeSet<String>) -> Result<Vec<String>> {
        self.update(|s| s.prune(live))
    }

    pub fn record_watch(&self, key: &str, check: WatchState) -> Result<()> {
        self.update(|s| s.record_watch(key, check))
    }

    pub fn pause_watch(&self, key: &str, now: DateTime<Utc>) -> Result<bool> {
        self.update(|s| s.pause_watch(key, now))
    }

    pub fn resume_watch(&self, key: &str) -> Result<bool> {
        self.update(|s| s.resume_watch(key))
    }

    pub fn dismiss_run(&self, key: &str, run_id: &str) -> Result<bool> {
        self.update(|s| s.dismiss_run(key, run_id))
    }

    pub fn note_tick(&self, now: DateTime<Utc>, by: TickSource) -> Result<()> {
        self.update(|s| s.note_tick(now, by))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Arc;
    use std::thread;

    fn store() -> (tempfile::TempDir, StateStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = StateStore::new(&StatePaths::new(dir.path()));
        (dir, s)
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap()
    }

    #[test]
    fn claims_a_cron_instant_once() {
        let (_d, s) = store();
        assert!(s.claim("k", "2026-09-17T07:00:00Z").unwrap());
        assert!(!s.claim("k", "2026-09-17T07:00:00Z").unwrap());
        assert!(s.claim("k", "2026-09-18T07:00:00Z").unwrap());
        assert_eq!(s.load().claim_of("k"), Some("2026-09-18T07:00:00Z"));
    }

    #[test]
    fn persists_claims_roots_and_the_last_tick() {
        let (d, s) = store();
        let root = d.path().join("srv").join("repo");
        s.update(|f| {
            f.claim("k", "t");
            f.register_root(&root);
            f.note_tick(now(), TickSource::Serve);
        })
        .unwrap();
        let back = StateStore::new(&StatePaths::new(d.path())).load();
        assert_eq!(back.version, 1);
        assert_eq!(back.claim_of("k"), Some("t"));
        assert_eq!(back.roots, vec![normalize_path(&root)]);
        assert_eq!(
            back.last_tick_at.as_deref(),
            Some("2026-09-17T12:00:00.000Z")
        );
        assert_eq!(back.last_tick_by, Some(TickSource::Serve));
    }

    #[test]
    fn registers_a_root_once_however_spelled_and_unregisters_it() {
        let (d, s) = store();
        let root = d.path().join("repo");
        assert!(s.register_root(&root).unwrap());
        assert!(!s.register_root(&root.join("x").join("..")).unwrap());
        assert_eq!(s.load().roots.len(), 1);
        assert!(s.unregister_root(&root).unwrap());
        assert!(!s.unregister_root(&root).unwrap());
        assert!(s.load().roots.is_empty());
    }

    #[test]
    fn dismisses_a_run_and_replaces_it_with_a_later_one() {
        let (_d, s) = store();
        assert!(s.dismiss_run("user:/r:x", "run-1").unwrap());
        assert!(!s.dismiss_run("user:/r:x", "run-1").unwrap());
        assert!(s.dismiss_run("user:/r:x", "run-2").unwrap());
        assert_eq!(s.load().dismissed_runs["user:/r:x"], "run-2");
    }

    #[test]
    fn pauses_and_resumes_a_watch() {
        let (_d, s) = store();
        assert!(s.pause_watch("k", now()).unwrap());
        assert!(!s.pause_watch("k", now()).unwrap());
        assert_eq!(s.load().paused_watches["k"], "2026-09-17T12:00:00.000Z");
        assert!(s.resume_watch("k").unwrap());
        assert!(!s.resume_watch("k").unwrap());
    }

    #[test]
    fn prunes_claims_and_checks_for_automations_that_no_longer_exist() {
        let (_d, s) = store();
        s.update(|f| {
            f.claim("live", "t");
            f.claim("gone", "t");
            f.record_watch(
                "gone",
                WatchState {
                    checked_at: "t".into(),
                    ..WatchState::default()
                },
            );
        })
        .unwrap();
        assert_eq!(
            s.prune(&BTreeSet::from(["live".to_string()])).unwrap(),
            vec!["gone".to_string()]
        );
        let f = s.load();
        assert!(f.claim_of("gone").is_none() && f.watches.is_empty());
        assert_eq!(f.claim_of("live"), Some("t"));
    }

    #[test]
    fn starts_clean_when_the_file_is_corrupt_or_another_version() {
        let (d, s) = store();
        std::fs::write(d.path().join("state.json"), "]]not json[[").unwrap();
        assert_eq!(s.load().claimed.len(), 0);
        std::fs::write(
            d.path().join("state.json"),
            r#"{"version":2,"claimed":{"k":"t"}}"#,
        )
        .unwrap();
        assert!(s.load().claimed.is_empty());
        assert!(s.claim("k", "t").unwrap());
    }

    #[test]
    fn drops_bad_entries_one_by_one() {
        let (d, s) = store();
        std::fs::write(
            d.path().join("state.json"),
            r#"{"version":1,"claimed":{"a":"t","b":5},"roots":["/x",3],
               "watches":{"w":{"checkedAt":"t"},"bad":{"durationMs":1}},
               "pausedWatches":{"p":"2026-09-17T12:00:00Z","q":"later"},
               "dismissedRuns":{"r":"run-1","s":""},"lastTickBy":"nobody"}"#,
        )
        .unwrap();
        let f = s.load();
        assert_eq!(f.claimed.keys().collect::<Vec<_>>(), vec!["a"]);
        assert_eq!(f.roots, vec![PathBuf::from("/x")]);
        assert_eq!(f.watches.keys().collect::<Vec<_>>(), vec!["w"]);
        assert_eq!(f.paused_watches.keys().collect::<Vec<_>>(), vec!["p"]);
        assert_eq!(f.dismissed_runs.keys().collect::<Vec<_>>(), vec!["r"]);
        assert_eq!(f.last_tick_by, None);
    }

    #[test]
    fn does_not_write_when_nothing_changed() {
        let (d, s) = store();
        assert!(!s.resume_watch("k").unwrap());
        assert!(!d.path().join("state.json").exists());
    }

    #[test]
    fn concurrent_claims_never_lose_an_update() {
        let (d, _) = store();
        let paths = Arc::new(StatePaths::new(d.path()));
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let paths = paths.clone();
                thread::spawn(move || {
                    let s = StateStore::new(&paths);
                    for i in 0..15 {
                        assert!(s.claim(&format!("k{t}-{i}"), "t").unwrap());
                        // A command racing the tick on another field.
                        s.register_root(Path::new(&format!("/repo{t}"))).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let f = StateStore::new(&paths).load();
        assert_eq!(f.claimed.len(), 8 * 15);
        assert_eq!(f.roots.len(), 8);
    }

    #[test]
    fn two_racing_claims_of_one_instant_have_one_winner() {
        let (d, _) = store();
        let paths = Arc::new(StatePaths::new(d.path()));
        let wins: usize = (0..8)
            .map(|_| {
                let paths = paths.clone();
                thread::spawn(move || StateStore::new(&paths).claim("k", "07:00").unwrap())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum();
        assert_eq!(wins, 1);
    }
}

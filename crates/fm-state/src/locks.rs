//! Run locks, the tick lock and stop files.
//!
//! A run lock per automation means never two runs of it at once. It is an atomic create
//! (`O_EXCL`) of `locks/<slug>.lock` holding the owner's pid and start time, so a lock left by a
//! dead process (or by a process whose pid was since reused) is swept and recorded `aborted`.
//! A held lock is never queued behind.
//!
//! Stopping a run is a file the run polls (`locks/<slug>.stop`), never a signal: on Windows a
//! signal kills outright and leaves the lock behind. The file names its run, so a late request
//! cannot stop the next run.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use fm_model::{Outcome, Retention, RunLockInfo, RunRecord, StatePaths, Trigger};
use serde::{Deserialize, Serialize};

use crate::error::{IoContext, Result, StateError};
use crate::fsio::{
    LOCK_TIMEOUT, ensure_dir, lock_file, open_new, random_hex, read_json, write_atomic,
    write_json_atomic,
};
use crate::keys::{name_of, root_of, scope_of};
use crate::process::{ProcessProbe, own_start_time, owner_alive};
use crate::progress::fold_abandoned;
use crate::records::{append_record, new_run_id};
use crate::time::{iso, parse_iso};

/// An unreadable lock this old was left by a writer killed between creating and filling it.
const UNREADABLE_STALE: Duration = Duration::from_secs(2);

/// What a run lock file holds: who owns it, plus what the sweep needs to write the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLockFile {
    #[serde(flatten)]
    pub info: RunLockInfo,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    /// Past this long after `startedAt` the lock is abandoned even with its owner alive: the
    /// timeout, every hook's budget, and slack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
}

/// The lock owner fields for a run this process is about to start.
pub fn own_lock_info(
    run_id: &str,
    trigger: Trigger,
    agent: Option<String>,
    now: DateTime<Utc>,
    probe: &dyn ProcessProbe,
) -> RunLockInfo {
    RunLockInfo {
        run_id: run_id.to_string(),
        pid: std::process::id(),
        pid_started_at: own_start_time(probe).map(iso),
        started_at: iso(now),
        trigger,
        agent,
    }
}

/// Create `file` with `body` only if it does not exist. `Ok(false)` when it does.
fn write_exclusive(file: &Path, body: &str) -> io::Result<bool> {
    let mut f = match open_new(file, true) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        // Windows: a lock being deleted cannot be created again until the deletion completes.
        Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
        Err(e) => return Err(e),
    };
    if let Err(e) = f.write_all(body.as_bytes()).and_then(|()| f.sync_all()) {
        drop(f);
        let _ = fs::remove_file(file);
        return Err(e);
    }
    Ok(true)
}

/// The guard every removal of a lock someone else wrote happens under, so a lock judged abandoned
/// cannot be replaced by a fresh one before it is removed.
fn with_guard<R>(paths: &StatePaths, f: impl FnOnce() -> Result<R>) -> Result<R> {
    let _g = lock_file(&paths.locks_dir().join("sweep.guard"), LOCK_TIMEOUT)?;
    f()
}

/// The outcome of trying to take a run lock.
#[derive(Debug)]
pub enum Acquire {
    Acquired(RunLock),
    /// Someone has it. An unreadable lock reads as held by pid 0, run `unknown`.
    Held(RunLockInfo),
}

/// A held run lock. Released by [`RunLock::release`] or on drop, and only while it is still ours.
#[derive(Debug)]
pub struct RunLock {
    file: PathBuf,
    run_id: String,
    released: bool,
}

impl RunLock {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn path(&self) -> &Path {
        &self.file
    }
    pub fn release(mut self) {
        self.release_inner();
    }
    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let ours =
            read_json::<RunLockFile>(&self.file).is_some_and(|l| l.info.run_id == self.run_id);
        if ours {
            let _ = fs::remove_file(&self.file);
        }
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        self.release_inner();
    }
}

fn unknown_owner() -> RunLockInfo {
    RunLockInfo {
        run_id: "unknown".into(),
        pid: 0,
        pid_started_at: None,
        started_at: "1970-01-01T00:00:00.000Z".into(),
        trigger: Trigger::Schedule,
        agent: None,
    }
}

/// Take the run lock for `lock.key`, or say who holds it. Never waits.
pub fn acquire_run_lock(paths: &StatePaths, lock: &RunLockFile) -> Result<Acquire> {
    let file = paths.lock_file(&lock.key);
    ensure_dir(&paths.locks_dir())?;
    let body =
        serde_json::to_string(lock).map_err(|e| StateError::io(&file, io::Error::other(e)))?;
    if write_exclusive(&file, &body).at(&file)? {
        return Ok(Acquire::Acquired(RunLock {
            file,
            run_id: lock.info.run_id.clone(),
            released: false,
        }));
    }
    Ok(Acquire::Held(
        read_json::<RunLockFile>(&file).map_or_else(unknown_owner, |l| l.info),
    ))
}

pub fn read_run_lock(paths: &StatePaths, key: &str) -> Option<RunLockFile> {
    read_json(&paths.lock_file(key))
}

/// Change the lock this process holds (atomically). The owner, run and key cannot change, and a
/// lock that is gone is not recreated. False when there was none or it could not be written.
pub fn update_run_lock(
    paths: &StatePaths,
    key: &str,
    change: impl FnOnce(&mut RunLockFile),
) -> bool {
    let file = paths.lock_file(key);
    let Some(current) = read_run_lock(paths, key) else {
        return false;
    };
    let mut next = current.clone();
    change(&mut next);
    next.key = current.key;
    next.info.pid = current.info.pid;
    next.info.pid_started_at = current.info.pid_started_at;
    next.info.run_id = current.info.run_id;
    let Ok(body) = serde_json::to_string(&next) else {
        return false;
    };
    file.exists() && write_atomic(&file, body.as_bytes()).is_ok()
}

/// Abandoned: the owner is gone (or its pid now belongs to another process), or the lock outlived
/// its budget.
pub fn is_run_lock_stale(lock: &RunLockFile, probe: &dyn ProcessProbe, now: DateTime<Utc>) -> bool {
    if !owner_alive(probe, lock.info.pid, lock.info.pid_started_at.as_deref()) {
        return true;
    }
    let Some(started) = parse_iso(&lock.info.started_at) else {
        return true;
    };
    lock.budget_ms
        .is_some_and(|b| (now - started).num_milliseconds() > i64::try_from(b).unwrap_or(i64::MAX))
}

fn age_of(file: &Path) -> Option<Duration> {
    fs::metadata(file).ok()?.modified().ok()?.elapsed().ok()
}

fn aborted_record(lock: &RunLockFile, now: DateTime<Utc>) -> RunRecord {
    let id = if lock.info.run_id.is_empty() || lock.info.run_id == "unknown" {
        new_run_id(now)
    } else {
        lock.info.run_id.clone()
    };
    RunRecord {
        id,
        key: lock.key.clone(),
        name: name_of(&lock.key).to_string(),
        scope: scope_of(&lock.key),
        root: PathBuf::from(root_of(&lock.key)),
        trigger: lock.info.trigger,
        scheduled_for: lock.scheduled_for.clone(),
        late: false,
        recorded_at: iso(now),
        started_at: Some(lock.info.started_at.clone()),
        ended_at: Some(iso(now)),
        outcome: Outcome::Aborted,
        reason: Some("host stopped".into()),
        error: None,
        agent: lock.info.agent.clone(),
        model: None,
        timeout: None,
        hooks: Vec::new(),
        output: None,
        session: None,
        chart: None,
        steps: Vec::new(),
        taken: Vec::new(),
        notes: Vec::new(),
        message: None,
        count: None,
        pid: Some(lock.info.pid),
        history: Vec::new(),
    }
}

/// Clear abandoned run locks, writing an `aborted` record (`host stopped`) for each, with whatever
/// progress the run had reported. Returns the records written.
pub fn sweep_stale_run_locks(
    paths: &StatePaths,
    probe: &dyn ProcessProbe,
    retention: Option<&Retention>,
    now: DateTime<Utc>,
) -> Result<Vec<RunRecord>> {
    let dir = paths.locks_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut written = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".lock") || name == "tick.lock" {
            continue;
        }
        let file = entry.path();
        let Ok(raw) = fs::read_to_string(&file) else {
            continue;
        };
        let parsed = serde_json::from_str::<RunLockFile>(&raw).ok();
        let abandoned = match &parsed {
            Some(lock) => is_run_lock_stale(lock, probe, now),
            None => age_of(&file).is_some_and(|a| a > UNREADABLE_STALE),
        };
        if !abandoned {
            continue;
        }
        let record = with_guard(paths, || {
            // Only the lock as it was judged: it may have been released and taken again since.
            if fs::read_to_string(&file).ok().as_deref() != Some(raw.as_str()) {
                return Ok(None);
            }
            let rec = match &parsed {
                Some(lock) => {
                    let rec = fold_abandoned(paths, aborted_record(lock, now), now);
                    append_record(paths, &rec, retention)?;
                    clear_stop(paths, &lock.key);
                    Some(rec)
                }
                None => None,
            };
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(StateError::io(&file, e)),
            }
            Ok(rec)
        })?;
        written.extend(record);
    }
    Ok(written)
}

// --------------------------------------------------------------------------- stop files

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StopRequest {
    run_id: String,
    at: String,
}

/// Ask the process running `key` to stop run `run_id`.
pub fn request_stop(paths: &StatePaths, key: &str, run_id: &str, now: DateTime<Utc>) -> Result<()> {
    write_json_atomic(
        &paths.stop_file(key),
        &StopRequest {
            run_id: run_id.to_string(),
            at: iso(now),
        },
    )
}

/// True when a stop was asked for this run. A request naming another run is stale and is cleared.
pub fn stop_requested(paths: &StatePaths, key: &str, run_id: &str) -> bool {
    let Some(req) = read_json::<StopRequest>(&paths.stop_file(key)) else {
        return false;
    };
    if req.run_id == run_id {
        return true;
    }
    clear_stop(paths, key);
    false
}

/// Remove a stop request. The run does this on the way out, whatever the outcome.
pub fn clear_stop(paths: &StatePaths, key: &str) {
    let _ = fs::remove_file(paths.stop_file(key));
}

/// What [`stop_run`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// The stop file is written; the run will see it.
    Requested,
    /// No run holds the lock, or its owner is gone.
    NotRunning,
    /// Another run of the automation holds the lock now; nothing was written.
    OtherRun(String),
}

/// Stop `run_id` of `key` if it is the run going now. Never writes a request for a finished run,
/// nor one that could stop the run after it.
pub fn stop_run(
    paths: &StatePaths,
    key: &str,
    run_id: &str,
    probe: &dyn ProcessProbe,
    now: DateTime<Utc>,
) -> Result<StopOutcome> {
    let Some(lock) = read_run_lock(paths, key) else {
        return Ok(StopOutcome::NotRunning);
    };
    if !owner_alive(probe, lock.info.pid, lock.info.pid_started_at.as_deref()) {
        return Ok(StopOutcome::NotRunning);
    }
    if lock.info.run_id != run_id {
        return Ok(StopOutcome::OtherRun(lock.info.run_id));
    }
    request_stop(paths, key, run_id, now)?;
    Ok(StopOutcome::Requested)
}

// --------------------------------------------------------------------------- tick lock

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TickBody {
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid_started_at: Option<String>,
    at: String,
    token: String,
}

/// The tick lock: one tick evaluates at a time. Released on drop, only while still ours.
#[derive(Debug)]
pub struct TickLock {
    file: PathBuf,
    token: String,
}

impl Drop for TickLock {
    fn drop(&mut self) {
        if read_json::<TickBody>(&self.file).is_some_and(|b| b.token == self.token) {
            let _ = fs::remove_file(&self.file);
        }
    }
}

/// Take the tick lock, or `None` while another live tick holds it. A lock whose owner is dead, or
/// older than `stale_ms`, is replaced, so a crash cannot stop the clock.
pub fn acquire_tick_lock(
    paths: &StatePaths,
    stale_ms: u64,
    probe: &dyn ProcessProbe,
    now: DateTime<Utc>,
) -> Result<Option<TickLock>> {
    let file = paths.tick_lock();
    ensure_dir(&paths.root)?;
    ensure_dir(&paths.locks_dir())?;
    let token = random_hex(8);
    let body = serde_json::to_string(&TickBody {
        pid: std::process::id(),
        pid_started_at: own_start_time(probe).map(iso),
        at: iso(now),
        token: token.clone(),
    })
    .map_err(|e| StateError::io(&file, io::Error::other(e)))?;
    let take = |file: &Path| -> Result<Option<TickLock>> {
        Ok(write_exclusive(file, &body).at(file)?.then(|| TickLock {
            file: file.to_path_buf(),
            token: token.clone(),
        }))
    };
    if let Some(lock) = take(&file)? {
        return Ok(Some(lock));
    }
    with_guard(paths, || {
        let Ok(raw) = fs::read_to_string(&file) else {
            // Released since: try once more.
            return take(&file);
        };
        let abandoned = match serde_json::from_str::<TickBody>(&raw) {
            Ok(held) => {
                let old = parse_iso(&held.at).is_none_or(|at| {
                    (now - at).num_milliseconds() > i64::try_from(stale_ms).unwrap_or(i64::MAX)
                });
                old || !owner_alive(probe, held.pid, held.pid_started_at.as_deref())
            }
            Err(_) => age_of(&file).is_some_and(|a| a > UNREADABLE_STALE),
        };
        if !abandoned {
            return Ok(None);
        }
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(StateError::io(&file, e)),
        }
        take(&file)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProbe, SystemProbe};
    use crate::records::read_records;
    use chrono::TimeZone;
    use std::sync::{Arc, Barrier};
    use std::thread;

    const KEY: &str = "repository:/srv/repo/.agents/automations:stale-pr-nudge";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap()
    }

    fn me() -> FakeProbe {
        FakeProbe::new().alive(std::process::id(), Some(now() - chrono::Duration::hours(1)))
    }

    fn lock(run_id: &str, pid: u32, started: DateTime<Utc>) -> RunLockFile {
        RunLockFile {
            info: RunLockInfo {
                run_id: run_id.into(),
                pid,
                pid_started_at: None,
                started_at: iso(started),
                trigger: Trigger::Schedule,
                agent: Some("claude".into()),
            },
            key: KEY.into(),
            scheduled_for: None,
            budget_ms: None,
        }
    }

    fn state() -> (tempfile::TempDir, StatePaths) {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        (d, p)
    }

    #[test]
    fn gives_a_run_lock_to_one_holder_and_reports_who_has_it() {
        let (_d, p) = state();
        let me = std::process::id();
        let Acquire::Acquired(first) = acquire_run_lock(&p, &lock("run-1", me, now())).unwrap()
        else {
            panic!("first acquire");
        };
        match acquire_run_lock(&p, &lock("run-2", me, now())).unwrap() {
            Acquire::Held(info) => assert_eq!(info.run_id, "run-1"),
            Acquire::Acquired(_) => panic!("two holders"),
        }
        assert_eq!(
            read_run_lock(&p, KEY).unwrap().info.agent.as_deref(),
            Some("claude")
        );
        first.release();
        assert!(read_run_lock(&p, KEY).is_none());
        assert!(matches!(
            acquire_run_lock(&p, &lock("run-3", me, now())).unwrap(),
            Acquire::Acquired(_)
        ));
    }

    #[test]
    fn an_unreadable_lock_is_held_by_an_unknown_owner() {
        let (_d, p) = state();
        fs::create_dir_all(p.locks_dir()).unwrap();
        fs::write(p.lock_file(KEY), "").unwrap();
        match acquire_run_lock(&p, &lock("run-1", 1, now())).unwrap() {
            Acquire::Held(info) => assert_eq!((info.pid, info.run_id.as_str()), (0, "unknown")),
            Acquire::Acquired(_) => panic!("stole an unreadable lock"),
        }
    }

    #[test]
    fn many_threads_contending_for_one_run_lock_have_exactly_one_winner() {
        let (_d, p) = state();
        let p = Arc::new(p);
        let n = 16;
        let barrier = Arc::new(Barrier::new(n));
        let threads: Vec<_> = (0..n)
            .map(|i| {
                let (p, barrier) = (p.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    match acquire_run_lock(
                        &p,
                        &lock(&format!("run-{i}"), std::process::id(), now()),
                    )
                    .unwrap()
                    {
                        Acquire::Acquired(l) => {
                            // Hold it while the others try.
                            thread::sleep(Duration::from_millis(50));
                            l.release();
                            1
                        }
                        Acquire::Held(_) => 0,
                    }
                })
            })
            .collect();
        let winners: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
        assert_eq!(winners, 1);
    }

    #[test]
    fn a_dropped_lock_is_released_but_never_someone_elses() {
        let (_d, p) = state();
        {
            let _l = acquire_run_lock(&p, &lock("run-1", 1, now())).unwrap();
        }
        assert!(read_run_lock(&p, KEY).is_none());
        let Acquire::Acquired(mine) = acquire_run_lock(&p, &lock("run-1", 1, now())).unwrap()
        else {
            panic!()
        };
        // Swept and taken by another run meanwhile.
        fs::write(
            p.lock_file(KEY),
            serde_json::to_string(&lock("run-2", 2, now())).unwrap(),
        )
        .unwrap();
        mine.release();
        assert_eq!(read_run_lock(&p, KEY).unwrap().info.run_id, "run-2");
    }

    #[test]
    fn update_keeps_identity_and_never_recreates_a_released_lock() {
        let (_d, p) = state();
        let Acquire::Acquired(l) = acquire_run_lock(&p, &lock("run-1", 7, now())).unwrap() else {
            panic!()
        };
        assert!(update_run_lock(&p, KEY, |f| {
            f.info.agent = Some("codex".into());
            f.info.pid = 999;
            f.info.run_id = "other".into();
            f.key = "user:/x:y".into();
        }));
        let after = read_run_lock(&p, KEY).unwrap();
        assert_eq!(after.info.agent.as_deref(), Some("codex"));
        assert_eq!(
            (
                after.info.pid,
                after.info.run_id.as_str(),
                after.key.as_str()
            ),
            (7, "run-1", KEY)
        );
        l.release();
        assert!(!update_run_lock(&p, KEY, |f| f.info.agent = None));
        assert!(!p.lock_file(KEY).exists());
        // An unreadable lock is not an error either.
        fs::write(p.lock_file(KEY), "not json").unwrap();
        assert!(!update_run_lock(&p, KEY, |_| {}));
    }

    #[test]
    fn calls_a_lock_stale_when_its_owner_is_gone_or_its_pid_was_reused() {
        let probe = FakeProbe::new().alive(42, Some(now() - chrono::Duration::minutes(5)));
        let mut l = lock("r", 43, now());
        assert!(is_run_lock_stale(&l, &probe, now()));
        l.info.pid = 42;
        assert!(!is_run_lock_stale(&l, &probe, now()));
        l.info.pid_started_at = Some(iso(now() - chrono::Duration::minutes(5)));
        assert!(!is_run_lock_stale(&l, &probe, now()));
        l.info.pid_started_at = Some(iso(now() - chrono::Duration::days(3)));
        assert!(
            is_run_lock_stale(&l, &probe, now()),
            "pid reused by a newer process"
        );
    }

    #[test]
    fn calls_a_live_lock_stale_only_past_its_budget() {
        let probe = FakeProbe::new().alive(42, None);
        let mut l = lock("r", 42, now());
        l.budget_ms = Some(1_080_000);
        assert!(!is_run_lock_stale(
            &l,
            &probe,
            now() + chrono::Duration::milliseconds(1_000_000)
        ));
        assert!(is_run_lock_stale(
            &l,
            &probe,
            now() + chrono::Duration::milliseconds(1_100_000)
        ));
    }

    #[test]
    fn sweeps_an_abandoned_lock_into_an_aborted_record() {
        let (_d, p) = state();
        let mut dead = lock("run-9", 1 << 30, now() - chrono::Duration::hours(3));
        dead.scheduled_for = Some("2026-09-17T07:00:00Z".into());
        acquire_run_lock(&p, &dead).map(std::mem::forget).unwrap();
        request_stop(&p, KEY, "run-9", now()).unwrap();

        let swept = sweep_stale_run_locks(&p, &FakeProbe::new(), None, now()).unwrap();
        assert_eq!(swept.len(), 1);
        let r = &swept[0];
        assert_eq!(r.id, "run-9");
        assert_eq!(r.outcome, Outcome::Aborted);
        assert_eq!(r.reason.as_deref(), Some("host stopped"));
        assert_eq!(r.name, "stale-pr-nudge");
        assert_eq!(r.scope, fm_model::Scope::Repository);
        assert_eq!(r.root, PathBuf::from("/srv/repo/.agents/automations"));
        assert_eq!(r.scheduled_for.as_deref(), Some("2026-09-17T07:00:00Z"));
        assert_eq!(r.agent.as_deref(), Some("claude"));
        assert!(read_run_lock(&p, KEY).is_none());
        assert!(!p.stop_file(KEY).exists());
        assert_eq!(read_records(&p, KEY, 10), swept);
        // A second sweep finds nothing.
        assert!(
            sweep_stale_run_locks(&p, &FakeProbe::new(), None, now())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn leaves_a_healthy_lock_and_the_tick_lock_alone() {
        let (_d, p) = state();
        let l = lock("run-live", std::process::id(), now());
        let held = acquire_run_lock(&p, &l).unwrap();
        fs::write(p.locks_dir().join("tick.lock"), "{}").unwrap();
        assert!(
            sweep_stale_run_locks(&p, &me(), None, now())
                .unwrap()
                .is_empty()
        );
        assert!(read_run_lock(&p, KEY).is_some());
        assert!(p.locks_dir().join("tick.lock").exists());
        drop(held);
    }

    #[test]
    fn removes_an_unreadable_lock_only_once_it_is_old() {
        let (_d, p) = state();
        fs::create_dir_all(p.locks_dir()).unwrap();
        fs::write(p.lock_file(KEY), "").unwrap();
        assert!(
            sweep_stale_run_locks(&p, &me(), None, now())
                .unwrap()
                .is_empty()
        );
        assert!(
            p.lock_file(KEY).exists(),
            "a fresh empty lock may be a writer mid-create"
        );
        thread::sleep(UNREADABLE_STALE + Duration::from_millis(200));
        assert!(
            sweep_stale_run_locks(&p, &me(), None, now())
                .unwrap()
                .is_empty()
        );
        assert!(!p.lock_file(KEY).exists());
    }

    #[test]
    fn concurrent_sweeps_write_one_record() {
        let (_d, p) = state();
        let p = Arc::new(p);
        acquire_run_lock(&p, &lock("run-9", 1 << 30, now()))
            .map(std::mem::forget)
            .unwrap();
        let barrier = Arc::new(Barrier::new(6));
        let total: usize = (0..6)
            .map(|_| {
                let (p, barrier) = (p.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    sweep_stale_run_locks(&p, &FakeProbe::new(), None, now())
                        .unwrap()
                        .len()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .sum();
        assert_eq!(total, 1);
        assert_eq!(read_records(&p, KEY, 10).len(), 1);
    }

    #[test]
    fn a_stop_is_seen_by_the_run_it_names_only() {
        let (_d, p) = state();
        assert!(!stop_requested(&p, KEY, "run-1"));
        request_stop(&p, KEY, "run-1", now()).unwrap();
        assert!(stop_requested(&p, KEY, "run-1"));
        assert!(!stop_requested(&p, "user:/r:other", "run-1"));
        // A request for a run that is over is cleared, so it cannot stop the next one.
        assert!(!stop_requested(&p, KEY, "run-2"));
        assert!(!p.stop_file(KEY).exists());
        assert!(!stop_requested(&p, KEY, "run-1"));
        fs::write(p.stop_file(KEY), "{ not json").unwrap();
        assert!(!stop_requested(&p, KEY, "run-1"));
        clear_stop(&p, KEY);
        clear_stop(&p, KEY);
    }

    #[test]
    fn stop_run_targets_only_the_run_going_now() {
        let (_d, p) = state();
        let probe = me();
        assert_eq!(
            stop_run(&p, KEY, "run-1", &probe, now()).unwrap(),
            StopOutcome::NotRunning
        );
        let held = acquire_run_lock(&p, &lock("run-2", std::process::id(), now())).unwrap();
        assert_eq!(
            stop_run(&p, KEY, "run-1", &probe, now()).unwrap(),
            StopOutcome::OtherRun("run-2".into())
        );
        assert!(!p.stop_file(KEY).exists());
        assert_eq!(
            stop_run(&p, KEY, "run-2", &probe, now()).unwrap(),
            StopOutcome::Requested
        );
        assert!(stop_requested(&p, KEY, "run-2"));
        drop(held);
        // An owner that died is not running, and gets no stop file.
        clear_stop(&p, KEY);
        acquire_run_lock(&p, &lock("run-3", 1 << 30, now()))
            .map(std::mem::forget)
            .unwrap();
        assert_eq!(
            stop_run(&p, KEY, "run-3", &probe, now()).unwrap(),
            StopOutcome::NotRunning
        );
        assert!(!p.stop_file(KEY).exists());
    }

    #[test]
    fn gives_the_tick_lock_to_exactly_one_and_reclaims_dead_or_old_ones() {
        let (_d, p) = state();
        let probe = me();
        let first = acquire_tick_lock(&p, 300_000, &probe, now()).unwrap();
        assert!(first.is_some());
        assert!(
            acquire_tick_lock(&p, 300_000, &probe, now())
                .unwrap()
                .is_none()
        );
        drop(first);
        assert!(
            acquire_tick_lock(&p, 300_000, &probe, now())
                .unwrap()
                .is_some()
        );

        let body = |pid: u32, at: DateTime<Utc>| {
            serde_json::to_string(&TickBody {
                pid,
                pid_started_at: None,
                at: iso(at),
                token: "t".into(),
            })
            .unwrap()
        };
        fs::write(p.tick_lock(), body(1 << 30, now())).unwrap();
        let dead = acquire_tick_lock(&p, 300_000, &probe, now()).unwrap();
        assert!(dead.is_some(), "a dead owner's tick lock is replaced");
        drop(dead);
        fs::write(
            p.tick_lock(),
            body(std::process::id(), now() - chrono::Duration::minutes(10)),
        )
        .unwrap();
        assert!(
            acquire_tick_lock(&p, 300_000, &probe, now())
                .unwrap()
                .is_some(),
            "too old"
        );
    }

    #[test]
    fn many_threads_contending_for_a_stale_tick_lock_have_one_winner() {
        let (_d, p) = state();
        fs::create_dir_all(&p.root).unwrap();
        fs::write(
            p.tick_lock(),
            serde_json::to_string(&TickBody {
                pid: 1 << 30,
                pid_started_at: None,
                at: iso(now()),
                token: "t".into(),
            })
            .unwrap(),
        )
        .unwrap();
        let p = Arc::new(p);
        let probe = Arc::new(me());
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (p, probe, barrier) = (p.clone(), probe.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    acquire_tick_lock(&p, 300_000, probe.as_ref(), now()).unwrap()
                })
            })
            .collect();
        let held: Vec<_> = threads
            .into_iter()
            .filter_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(held.len(), 1);
    }

    #[test]
    fn own_lock_info_names_this_process_and_its_start() {
        let info = own_lock_info("run-1", Trigger::Manual, None, now(), &SystemProbe);
        assert_eq!(info.pid, std::process::id());
        assert!(info.pid_started_at.is_some());
        assert!(owner_alive(
            &SystemProbe,
            info.pid,
            info.pid_started_at.as_deref()
        ));
    }
}

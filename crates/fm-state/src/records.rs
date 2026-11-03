//! Run records: one JSON line per run, and per skipped or missed instant, in `runs/<slug>.jsonl`.
//! A line torn by a killed writer is skipped on read and closed off on the next append, so one
//! killed process costs one record, never two.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use chrono::{DateTime, Local, Utc};
use fm_model::{Retention, RunRecord, StatePaths};

use crate::error::{IoContext, Result, StateError};
use crate::fsio::{ensure_dir, random_hex, with_lock, write_atomic};

/// `20260917-140047-3fa2`: local time, sortable, readable, and unique enough within a second.
pub fn new_run_id(now: DateTime<Utc>) -> String {
    format!(
        "{}-{}",
        now.with_timezone(&Local).format("%Y%m%d-%H%M%S"),
        random_hex(2)
    )
}

/// True when the file is non-empty and its last byte is not a newline.
fn ends_without_newline(file: &Path) -> bool {
    let Ok(mut f) = fs::File::open(file) else {
        return false;
    };
    let Ok(len) = f.seek(SeekFrom::End(0)) else {
        return false;
    };
    if len == 0 || f.seek(SeekFrom::End(-1)).is_err() {
        return false;
    }
    let mut b = [0u8; 1];
    f.read_exact(&mut b).is_ok() && b[0] != b'\n'
}

/// Append one JSON line, closing off a torn last line first. Not locked: callers hold the lock.
fn append_line(file: &Path, line: &str) -> Result<()> {
    if let Some(dir) = file.parent() {
        ensure_dir(dir)?;
    }
    let prefix = if ends_without_newline(file) { "\n" } else { "" };
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(file).at(file)?;
    f.write_all(format!("{prefix}{line}\n").as_bytes()).at(file)
}

fn lines(file: &Path) -> Vec<String> {
    fs::read(file)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .split('\n')
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Append one record to its automation's file, then rotate if `retention` says so.
pub fn append_record(
    paths: &StatePaths,
    rec: &RunRecord,
    retention: Option<&Retention>,
) -> Result<()> {
    let file = paths.records_file(&rec.key);
    let line =
        serde_json::to_string(rec).map_err(|e| StateError::io(&file, std::io::Error::other(e)))?;
    with_lock(&file, || {
        append_line(&file, &line)?;
        if let Some(r) = retention {
            rotate_locked(&file, r)?;
        }
        Ok(())
    })
}

/// The last `n` records, oldest first. Torn or unreadable lines are skipped.
pub fn read_records(paths: &StatePaths, key: &str, n: usize) -> Vec<RunRecord> {
    let all = lines(&paths.records_file(key));
    let start = all.len().saturating_sub(n);
    all[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The newest record that reads.
pub fn last_record(paths: &StatePaths, key: &str) -> Option<RunRecord> {
    lines(&paths.records_file(key))
        .iter()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
}

fn rotate_locked(file: &Path, retention: &Retention) -> Result<()> {
    let all = lines(file);
    let max = retention.max_lines_per_automation as usize;
    if all.len() <= max {
        return Ok(());
    }
    let keep = &all[all.len().saturating_sub(retention.keep_lines as usize)..];
    let mut text = keep.join("\n");
    if !keep.is_empty() {
        text.push('\n');
    }
    write_atomic(file, text.as_bytes())
}

/// Trim a file grown past `max-lines-per-automation` down to `keep-lines`, atomically.
pub fn rotate_if_needed(paths: &StatePaths, key: &str, retention: &Retention) -> Result<()> {
    let file = paths.records_file(key);
    with_lock(&file, || rotate_locked(&file, retention))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;
    use fm_model::{Outcome, Scope, Trigger};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap()
    }

    pub(crate) fn rec(key: &str, id: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            key: key.into(),
            name: "x".into(),
            scope: Scope::User,
            root: PathBuf::from("/r"),
            trigger: Trigger::Schedule,
            scheduled_for: None,
            late: false,
            recorded_at: crate::time::iso(now()),
            started_at: None,
            ended_at: None,
            outcome: Outcome::Ok,
            reason: None,
            error: None,
            agent: None,
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
            pid: None,
        }
    }

    const KEY: &str = "user:/r:x";

    fn ids(paths: &StatePaths, key: &str) -> Vec<String> {
        read_records(paths, key, 50)
            .into_iter()
            .map(|r| r.id)
            .collect()
    }

    #[test]
    fn appends_and_reads_back_oldest_first() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        append_record(&p, &rec(KEY, "a"), None).unwrap();
        append_record(&p, &rec(KEY, "b"), None).unwrap();
        assert_eq!(ids(&p, KEY), vec!["a", "b"]);
        assert_eq!(last_record(&p, KEY).unwrap().id, "b");
        assert_eq!(read_records(&p, KEY, 1)[0].id, "b");
    }

    #[test]
    fn returns_nothing_for_an_automation_that_never_ran() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        assert!(read_records(&p, "user:/r:nothing", 50).is_empty());
        assert!(last_record(&p, "user:/r:nothing").is_none());
    }

    #[test]
    fn skips_a_torn_line_and_the_next_append_does_not_glue_onto_it() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        append_record(&p, &rec(KEY, "a"), None).unwrap();
        // What a process killed mid-write leaves: a fragment with no newline.
        let mut f = OpenOptions::new()
            .append(true)
            .open(p.records_file(KEY))
            .unwrap();
        f.write_all(br#"{"id":"b","key":"#).unwrap();
        drop(f);
        assert_eq!(ids(&p, KEY), vec!["a"]);
        assert_eq!(last_record(&p, KEY).unwrap().id, "a");
        append_record(&p, &rec(KEY, "c"), None).unwrap();
        assert_eq!(ids(&p, KEY), vec!["a", "c"]);
    }

    #[test]
    fn rotates_past_the_limit_to_keep_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        let r = Retention {
            max_lines_per_automation: 10,
            keep_lines: 4,
        };
        for i in 0..10 {
            append_record(&p, &rec(KEY, &format!("r{i}")), Some(&r)).unwrap();
        }
        assert_eq!(read_records(&p, KEY, 50).len(), 10);
        append_record(&p, &rec(KEY, "r10"), Some(&r)).unwrap();
        assert_eq!(ids(&p, KEY), vec!["r7", "r8", "r9", "r10"]);
    }

    #[test]
    fn does_not_rotate_below_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        let r = Retention {
            max_lines_per_automation: 100,
            keep_lines: 10,
        };
        for i in 0..5 {
            append_record(&p, &rec(KEY, &format!("r{i}")), Some(&r)).unwrap();
        }
        rotate_if_needed(&p, KEY, &r).unwrap();
        assert_eq!(read_records(&p, KEY, 50).len(), 5);
    }

    #[test]
    fn keeps_two_automations_apart() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        append_record(&p, &rec("user:/r:a", "a"), None).unwrap();
        append_record(&p, &rec("user:/r:b", "b"), None).unwrap();
        assert_eq!(ids(&p, "user:/r:a"), vec!["a"]);
        assert_eq!(ids(&p, "user:/r:b"), vec!["b"]);
    }

    #[test]
    fn makes_run_ids_that_sort_and_are_unique() {
        let ids: std::collections::BTreeSet<String> = (0..50).map(|_| new_run_id(now())).collect();
        assert!(ids.len() > 40);
        for id in &ids {
            let b = id.as_bytes();
            assert_eq!(id.len(), 20, "{id}");
            assert!(b[..8].iter().all(u8::is_ascii_digit), "{id}");
            assert_eq!(b[8], b'-');
            assert!(b[9..15].iter().all(u8::is_ascii_digit), "{id}");
            assert_eq!(b[15], b'-');
            assert!(
                b[16..]
                    .iter()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn concurrent_appends_with_rotation_lose_nothing_they_should_keep() {
        let d = tempfile::tempdir().unwrap();
        let p = Arc::new(StatePaths::new(d.path()));
        let r = Retention {
            max_lines_per_automation: 1000,
            keep_lines: 1000,
        };
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let p = p.clone();
                thread::spawn(move || {
                    for i in 0..25 {
                        append_record(&p, &rec(KEY, &format!("t{t}-{i}")), Some(&r)).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(read_records(&p, KEY, 1000).len(), 200);
    }
}

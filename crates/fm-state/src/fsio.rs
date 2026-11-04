//! Files: folders with owner-only access, atomic writes, lenient reads, and the short
//! read-change-write lock every shared JSON file is changed under.
//!
//! The short lock is an OS lock (`flock` on Unix, `LockFileEx` on Windows) on a sidecar
//! `<file>.lock`, so a process that dies holding it releases it: there is nothing to sweep.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{IoContext, Result, StateError};

/// How long a read-change-write waits for its lock before giving up.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// `mkdir -p`, owner-only (0700) on Unix.
pub fn ensure_dir(dir: &Path) -> Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir).at(dir)
}

fn ensure_parent(file: &Path) -> Result<()> {
    match file.parent() {
        Some(p) if !p.as_os_str().is_empty() => ensure_dir(p),
        _ => Ok(()),
    }
}

/// Four hex digits from the OS's random source (time-based if that fails).
pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if getrandom::fill(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            ^ std::process::id();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (nanos >> ((i % 4) * 8)) as u8;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Open a new file for writing, owner-only on Unix. `create_new` makes it an atomic create.
pub(crate) fn open_new(file: &Path, exclusive: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true);
    if exclusive {
        o.create_new(true);
    } else {
        o.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(file)
}

/// Rename, retrying briefly: on Windows a reader or a virus scanner can hold the target a moment.
pub(crate) fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    let mut last = None;
    for attempt in 0..50 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied => {
                last = Some(e);
                thread::sleep(Duration::from_millis(2 + attempt));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("rename failed")))
}

/// Write via a temporary sibling with this writer's own name, then rename, so a reader never sees
/// a partial file and two writers never share a temporary file.
pub fn write_atomic(file: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent(file)?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = file.with_file_name(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        random_hex(4)
    ));
    let written = (|| {
        let mut f = open_new(&tmp, true)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        rename_retrying(&tmp, file)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(StateError::io(file, e));
    }
    Ok(())
}

/// Pretty JSON plus a newline, atomically.
pub fn write_json_atomic<T: Serialize + ?Sized>(file: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| StateError::io(file, io::Error::other(e)))?;
    text.push('\n');
    write_atomic(file, text.as_bytes())
}

/// Read and parse, or `None` when the file is absent, unreadable or not JSON of that shape.
pub fn read_json<T: DeserializeOwned>(file: &Path) -> Option<T> {
    let text = fs::read_to_string(file).ok()?;
    serde_json::from_str(&text).ok()
}

/// The sidecar lock file for `file`: `<file>.lock`.
pub fn lock_path_for(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    file.with_file_name(name)
}

/// Held for one read-change-write. Released on drop, and by the OS if the process dies.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

/// Take the OS lock on `lock_file`, waiting up to `timeout`.
pub fn lock_file(lock_file: &Path, timeout: Duration) -> Result<FileLock> {
    ensure_parent(lock_file)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_file)
        .at(lock_file)?;
    let start = Instant::now();
    let mut wait = Duration::from_millis(1);
    loop {
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => return Ok(FileLock { _file: file }),
            Err(fs4::TryLockError::WouldBlock) => {}
            Err(fs4::TryLockError::Error(e)) => return Err(StateError::io(lock_file, e)),
        }
        if start.elapsed() >= timeout {
            return Err(StateError::LockTimeout {
                path: lock_file.to_path_buf(),
                waited_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            });
        }
        thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_millis(10));
    }
}

/// Run `f` holding the lock on `<file>.lock`.
pub fn with_lock<R>(file: &Path, f: impl FnOnce() -> Result<R>) -> Result<R> {
    let _held = lock_file(&lock_path_for(file), LOCK_TIMEOUT)?;
    f()
}

/// The JSON read-change-write: under the lock, read what is on disk now (`read` turns it into a
/// value, whatever state it is in), apply `change`, and write the result only if it differs.
pub fn update_json<T, R>(
    file: &Path,
    read: impl FnOnce(Option<serde_json::Value>) -> T,
    change: impl FnOnce(&mut T) -> R,
) -> Result<(T, R)>
where
    T: Serialize + PartialEq + Clone,
{
    with_lock(file, || {
        let before = read(read_json::<serde_json::Value>(file));
        let mut after = before.clone();
        let out = change(&mut after);
        if after != before {
            write_json_atomic(file, &after)?;
        }
        Ok((after, out))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn writes_atomically_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("x.json");
        write_json_atomic(&file, &serde_json::json!({"a": 1})).unwrap();
        write_json_atomic(&file, &serde_json::json!({"a": 2})).unwrap();
        assert_eq!(
            read_json::<serde_json::Value>(&file).unwrap(),
            serde_json::json!({"a": 2})
        );
        let names: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["x.json".to_string()]);
    }

    #[test]
    fn reads_absent_or_corrupt_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.json");
        assert!(read_json::<serde_json::Value>(&file).is_none());
        fs::write(&file, "{ not json").unwrap();
        assert!(read_json::<serde_json::Value>(&file).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn folders_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a").join("b");
        ensure_dir(&sub).unwrap();
        let mode = fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn a_held_lock_times_out_the_next_taker() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("x.lock");
        let held = lock_file(&lock, LOCK_TIMEOUT).unwrap();
        let err = lock_file(&lock, Duration::from_millis(50)).unwrap_err();
        assert!(matches!(err, StateError::LockTimeout { .. }), "{err}");
        drop(held);
        lock_file(&lock, Duration::from_millis(50)).unwrap();
    }

    #[test]
    fn the_lock_lets_one_thread_in_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = Arc::new(dir.path().join("x.json"));
        let inside = Arc::new(AtomicUsize::new(0));
        let overlaps = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (file, inside, overlaps) = (file.clone(), inside.clone(), overlaps.clone());
                thread::spawn(move || {
                    for _ in 0..20 {
                        with_lock(&file, || {
                            if inside.fetch_add(1, Ordering::SeqCst) != 0 {
                                overlaps.fetch_add(1, Ordering::SeqCst);
                            }
                            thread::yield_now();
                            inside.fetch_sub(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(overlaps.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn update_json_loses_no_update_under_contention() {
        let dir = tempfile::tempdir().unwrap();
        let file = Arc::new(dir.path().join("counter.json"));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let file = file.clone();
                thread::spawn(move || {
                    for _ in 0..10 {
                        update_json(
                            &file,
                            |v| v.and_then(|v| v.as_u64()).unwrap_or(0),
                            |n| *n += 1,
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(read_json::<u64>(&file), Some(80));
    }
}

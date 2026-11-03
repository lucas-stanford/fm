//! Watching for a stop request while a run goes.
//!
//! A person stops a run by writing `locks/<slug>.stop` naming it (`fm_state::request_stop`). The
//! process runner stops a child when a file appears, but that file can be a stale request for an
//! earlier run, which must be cleared, not obeyed. So a small thread reads the request properly
//! (`fm_state::stop_requested`) and, only for this run, creates a file of the run's own for the
//! runner to see.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fm_model::{StatePaths, slug};
use fm_state::stop_requested;

/// The run's own stop file: `locks/<slug>.<run id>.halt`.
pub fn halt_file(paths: &StatePaths, key: &str, run_id: &str) -> PathBuf {
    paths
        .locks_dir()
        .join(format!("{}.{run_id}.halt", slug(key)))
}

pub(crate) struct StopWatch {
    paths: StatePaths,
    key: String,
    run_id: String,
    halt: PathBuf,
    asked: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn check(paths: &StatePaths, key: &str, run_id: &str, halt: &Path, asked: &AtomicBool) -> bool {
    if asked.load(Ordering::SeqCst) {
        return true;
    }
    if !stop_requested(paths, key, run_id) {
        return false;
    }
    asked.store(true, Ordering::SeqCst);
    if let Some(dir) = halt.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(halt, run_id);
    true
}

impl StopWatch {
    pub(crate) fn start(paths: &StatePaths, key: &str, run_id: &str, poll: Duration) -> StopWatch {
        let halt = halt_file(paths, key, run_id);
        let _ = std::fs::remove_file(&halt);
        let asked = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let thread = {
            let (paths, key, run_id, halt) = (
                paths.clone(),
                key.to_string(),
                run_id.to_string(),
                halt.clone(),
            );
            let (asked, done) = (Arc::clone(&asked), Arc::clone(&done));
            let poll = poll.clamp(Duration::from_millis(10), Duration::from_secs(1));
            thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    if check(&paths, &key, &run_id, &halt, &asked) {
                        return;
                    }
                    thread::sleep(poll);
                }
            })
        };
        StopWatch {
            paths: paths.clone(),
            key: key.to_string(),
            run_id: run_id.to_string(),
            halt,
            asked,
            done,
            thread: Some(thread),
        }
    }

    /// Asked to stop this run? A request naming another run is cleared, not obeyed.
    pub(crate) fn asked(&self) -> bool {
        check(
            &self.paths,
            &self.key,
            &self.run_id,
            &self.halt,
            &self.asked,
        )
    }

    /// The file a process of this run is stopped by.
    pub(crate) fn halt_file(&self) -> &Path {
        &self.halt
    }

    pub(crate) fn finish(mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.halt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn obeys_only_a_request_for_its_own_run() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path());
        let key = "user:/r:x";
        let w = StopWatch::start(&paths, key, "run-b", Duration::from_millis(10));
        fm_state::request_stop(&paths, key, "run-a", Utc::now()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while paths.stop_file(key).exists() {
            assert!(
                std::time::Instant::now() < until,
                "stale request not cleared"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!w.asked());
        assert!(!w.halt_file().exists());

        fm_state::request_stop(&paths, key, "run-b", Utc::now()).unwrap();
        while !w.halt_file().exists() {
            assert!(std::time::Instant::now() < until, "request not seen");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(w.asked());
        let halt = w.halt_file().to_path_buf();
        w.finish();
        assert!(!halt.exists());
    }
}

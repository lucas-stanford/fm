//! The real Windows watcher, end to end: registers a Task Scheduler task under a test name
//! (`fm-test-<random>`) that runs the probe binary against a temporary `FM_HOME`, waits for the
//! job's first tick and for the run it starts, then deletes the task. Ignored by default because
//! it touches the real scheduler:
//!
//! ```text
//! cargo test -p fm-clock --test windows_watcher -- --ignored --nocapture
//! ```
#![cfg(windows)]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;
use fm_clock::watcher::{self, Names, WatcherContext, WatcherState};
use fm_model::{StatePaths, TickSource};

/// Deletes the test task however the test ends.
struct Cleanup<'a>(&'a WatcherContext<'a>);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let r = watcher::stop(self.0);
        eprintln!("cleanup: {:?}", r.lines);
    }
}

#[test]
#[ignore = "registers a real Task Scheduler task (fm-test-*) for up to three minutes"]
fn the_windows_watcher_ticks_within_90s_starts_runs_and_is_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(tmp.path().join("state"));
    fm_state::open_state(&paths).unwrap();
    // The probe discovers under <FM_HOME>/home, never the real home.
    let home = paths.root.join("home");
    let root = automations_in(&home);
    write_automation(
        &root,
        "e2e",
        "  schedule:\n    cron: \"* * * * *\"\n    timezone: UTC\n",
        "",
        &[],
    );
    let l = located_in(&[fm_spec::user_root(&home)], "e2e");
    arm(&paths, &l, chrono::Utc::now());

    let probe = PathBuf::from(env!("CARGO_BIN_EXE_fm-clock-probe"));
    let suffix = format!(
        "{:x}{:x}",
        std::process::id(),
        chrono::Utc::now().timestamp_subsec_nanos()
    );
    let mut ctx = WatcherContext::system(paths.clone()).unwrap();
    ctx.names = Names::test(&suffix);
    ctx.current_exe = Some(probe.clone());
    let _cleanup = Cleanup(&ctx);
    eprintln!("task: {}", ctx.names.task);

    let started = Instant::now();
    let r = watcher::start(&ctx, &probe);
    eprintln!("start ({:?}): {:?}", started.elapsed(), r.lines);
    assert!(r.ok, "{:?}", r.lines);
    assert!(r.lines.contains(&"first tick ok".to_string()));
    let st = fm_state::StateStore::new(&paths).load();
    assert_eq!(st.last_tick_by, Some(TickSource::Job));

    let status = watcher::status(&ctx, true);
    eprintln!("status: {} {:?}", status.summary, status.lines);
    assert_eq!(status.state, WatcherState::On);
    assert!(status.lines.contains(&"  job: present".to_string()));

    // The per-minute schedule fires from the job, and the detached fire outlives the tick.
    let fired = paths.root.join("fired.txt");
    let deadline = Instant::now() + Duration::from_secs(150);
    while !fired.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(1));
    }
    let text = std::fs::read_to_string(&fired).unwrap_or_default();
    eprintln!("fired after {:?}: {text}", started.elapsed());
    assert!(
        text.contains(&format!(
            "fire --key {} --trigger schedule --scheduled-for ",
            l.key
        )),
        "{text}"
    );

    let r = watcher::stop(&ctx);
    eprintln!("stop: {:?}", r.lines);
    assert_eq!(r.lines[0], "watcher: off — job removed (schtasks)");
    let gone = ctx.exec.exec(&watcher::windows::query(&ctx.names.task));
    assert!(!gone.ok(), "the task is still registered");
    assert!(!watcher::watcher_registered(&paths));
}

#[test]
#[ignore = "registers a real Task Scheduler task (fm-test-*) for up to two minutes"]
fn the_windows_watcher_removes_itself_once_fm_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(tmp.path().join("state"));
    fm_state::open_state(&paths).unwrap();
    // A copy of the probe that this test deletes, as an uninstall would.
    let copy = tmp.path().join("bin").join("fm.exe");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_fm-clock-probe"), &copy).unwrap();
    let suffix = format!(
        "{:x}{:x}",
        std::process::id(),
        chrono::Utc::now().timestamp_subsec_nanos()
    );
    let mut ctx = WatcherContext::system(paths.clone()).unwrap();
    ctx.names = Names::test(&suffix);
    ctx.current_exe = Some(copy.clone());
    ctx.wait = Duration::ZERO;
    let _cleanup = Cleanup(&ctx);

    let r = watcher::start(&ctx, &copy);
    assert!(r.ok, "{:?}", r.lines);
    std::fs::remove_file(&copy).unwrap();
    // Run it now rather than wait for the minute.
    ctx.exec.exec(&watcher::windows::run_now(&ctx.names.task));
    let deadline = Instant::now() + Duration::from_secs(100);
    let mut present = true;
    while present && Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
        present = ctx
            .exec
            .exec(&watcher::windows::query(&ctx.names.task))
            .ok();
    }
    assert!(!present, "the task did not remove itself");
    assert!(!watcher::watcher_registered(&paths));
}

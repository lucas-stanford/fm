//! Watches in the tick, per `watch-scripts.md`, with scripts and starts faked. Ported from
//! pi-automations' `test/watch.test.ts` and `test/watch-controls.test.ts`.

mod common;

use std::time::Duration;

use common::*;
use fm_clock::{fingerprint, tick};
use fm_model::{Outcome, RunLockInfo, Trigger, WatchState};
use fm_run::process::{EnvMode, Output, ProcessOutcome, Stdin};
use fm_state::{RunLockFile, StateStore, acquire_run_lock};

const WATCH: &str = "  watch:\n    script: scripts/watch.js\n    every: 5m\n";
const NOW: &str = "2026-10-07T12:00:00Z";

fn watched(w: &World, name: &str) -> String {
    w.write(name, WATCH, &[("scripts/watch.js", "console.log('x')\n")]);
    w.arm(name, at("2026-10-07T11:00:00Z"))
}

fn plus(min: i64) -> chrono::DateTime<chrono::Utc> {
    at(NOW) + chrono::Duration::minutes(min)
}

fn watch_of(w: &World, key: &str) -> Option<WatchState> {
    w.state().watches.get(key).cloned()
}

#[test]
fn starts_a_watch_run_when_the_script_finds_something_and_records_it_first() {
    let w = World::new();
    let key = watched(&w, "w");
    let at_start = std::cell::RefCell::new(None);
    let mut starter = Starter::ok();
    starter.on_start = Some(Box::new(|s| {
        *at_start.borrow_mut() = watch_of(&w, &s.key);
    }));
    let scripts = Scripts::new(|_| exited(0, "PR 12\n", ""));

    let r = tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();

    assert_eq!(r.checked, std::slice::from_ref(&key));
    assert_eq!(r.fired, std::slice::from_ref(&key));
    assert_eq!(
        starter.calls(),
        [Started {
            key: key.clone(),
            trigger: Trigger::Watch,
            scheduled_for: None,
            late: false,
        }]
    );
    // On disk at the moment of the start, so a crash cannot start it twice.
    let saved = at_start.borrow().clone().unwrap();
    assert_eq!(saved.exit, Some(0));
    assert_eq!(saved.fired_at.as_deref(), Some("2026-10-07T12:00:00.000Z"));
    assert_eq!(saved.fired_for, Some(fingerprint("PR 12")));
    assert_eq!(saved.said.as_deref(), Some("PR 12"));
}

#[test]
fn runs_the_script_in_its_folder_with_the_watch_environment_and_no_input() {
    let w = World::new();
    let key = watched(&w, "env");
    StateStore::new(&w.paths)
        .record_watch(
            &key,
            WatchState {
                checked_at: "2026-10-07T11:55:00.000Z".into(),
                problem: Some("scripts/watch.js timed out after 2m".into()),
                answered_at: Some("2026-10-07T11:50:00.000Z".into()),
                ..WatchState::default()
            },
        )
        .unwrap();
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(3, "", ""));

    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();

    let spec = scripts.specs().pop().unwrap();
    assert_eq!(spec.cwd, w.located("env").dir);
    assert_eq!(spec.stdin, Stdin::Null);
    // The default hook timeout (5m) is capped at two minutes.
    assert_eq!(spec.timeout, Duration::from_secs(120));
    assert_eq!(spec.max_output_bytes, 65_536);
    assert!(
        spec.args
            .iter()
            .any(|a| a.to_string_lossy().ends_with("watch.js"))
    );
    let EnvMode::Scrubbed { overlay } = &spec.env_mode else {
        panic!("watch scripts run with credentials scrubbed");
    };
    let get = |k: &str| {
        overlay
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("AUTOMATION_NAME"), Some("env"));
    assert_eq!(get("AUTOMATION_KEY"), Some(key.as_str()));
    assert_eq!(get("AUTOMATION_TRIGGER"), Some("watch"));
    assert_eq!(get("AUTOMATION_PHASE"), Some("watch"));
    // The last check that answered, not the last one that ran.
    assert_eq!(
        get("AUTOMATION_LAST_CHECKED"),
        Some("2026-10-07T11:50:00.000Z")
    );
    assert_eq!(get("AUTOMATION_RUN_ID"), None);
}

#[test]
fn a_shorter_hook_timeout_is_used_as_is() {
    let w = World::new();
    watched(&w, "t");
    std::fs::write(w.paths.config(), "hook-timeout = \"30s\"\n").unwrap();
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(3, "", ""));
    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(scripts.specs()[0].timeout, Duration::from_secs(30));
}

#[test]
fn keeps_what_another_process_saved_while_the_scripts_ran() {
    let w = World::new();
    let key = watched(&w, "merge");
    let repo = w.tmp.path().join("repo");
    let scripts = Scripts::new(|_| {
        StateStore::new(&w.paths).register_root(&repo).unwrap();
        exited(3, "", "nothing")
    });
    let starter = Starter::ok();

    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();

    assert_eq!(w.state().roots, [fm_state::normalize_path(&repo)]);
    assert_eq!(watch_of(&w, &key).unwrap().exit, Some(3));
}

#[test]
fn does_not_check_inside_its_interval_nor_refire_the_same_finding_after_it() {
    let w = World::new();
    let key = watched(&w, "again");
    let scripts = Scripts::new(|_| exited(0, "PR 12", ""));
    let first = Starter::ok();
    tick(&w.ctx(&first, &scripts), at(NOW)).unwrap();
    assert_eq!(first.calls().len(), 1);

    let soon = Starter::ok();
    tick(&w.ctx(&soon, &scripts), plus(2)).unwrap();
    assert_eq!(scripts.count(), 1);

    let later = Starter::ok();
    let r = tick(&w.ctx(&later, &scripts), plus(5)).unwrap();
    assert_eq!(scripts.count(), 2);
    assert_eq!(r.checked, [key]);
    assert!(later.calls().is_empty());
}

#[test]
fn starts_nothing_when_the_script_finds_nothing_and_keeps_what_it_said() {
    let w = World::new();
    let key = watched(&w, "none");
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(3, "", "nothing is waiting on you\n"));
    let r = tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(r.checked, std::slice::from_ref(&key));
    assert!(starter.calls().is_empty());
    let s = watch_of(&w, &key).unwrap();
    assert_eq!(s.exit, Some(3));
    assert_eq!(s.said.as_deref(), Some("nothing is waiting on you"));
}

#[test]
fn a_non_zero_exit_forgets_the_finding_so_the_same_output_starts_a_run_again() {
    let w = World::new();
    watched(&w, "forget");
    let answers = std::sync::Mutex::new(vec![
        exited(0, "PR 12", ""),
        exited(1, "", "network down"),
        exited(0, "PR 12", ""),
    ]);
    let scripts = Scripts::new(|_| answers.lock().unwrap().remove(0));
    let mut fired = 0;
    for m in [0, 5, 10] {
        let s = Starter::ok();
        tick(&w.ctx(&s, &scripts), plus(m)).unwrap();
        fired += s.calls().len();
    }
    assert_eq!(fired, 2);
}

#[test]
fn a_check_that_could_not_answer_keeps_the_finding() {
    let w = World::new();
    let key = watched(&w, "keep");
    let answers = std::sync::Mutex::new(vec![
        exited(0, "PR 12", ""),
        ProcessOutcome::TimedOut {
            after: Duration::from_secs(120),
            output: Output::default(),
        },
        ProcessOutcome::StartFailed {
            message: "node: not found".into(),
        },
        ProcessOutcome::Signaled {
            signal: "SIGKILL".into(),
            output: Output {
                stdout: "half".into(),
                ..Output::default()
            },
        },
        exited(0, "PR 12", ""),
    ]);
    let scripts = Scripts::new(|_| answers.lock().unwrap().remove(0));
    let mut fired = 0;
    let mut problems = Vec::new();
    for m in [0, 5, 10, 15, 20] {
        let s = Starter::ok();
        tick(&w.ctx(&s, &scripts), plus(m)).unwrap();
        fired += s.calls().len();
        problems.push(watch_of(&w, &key).unwrap().problem);
    }
    assert_eq!(
        fired, 1,
        "the same finding after a hiccup is the run already started"
    );
    assert_eq!(
        problems,
        [
            None,
            Some("scripts/watch.js timed out after 2m".to_string()),
            Some("scripts/watch.js could not start — node: not found".to_string()),
            Some("scripts/watch.js was killed by SIGKILL".to_string()),
            None,
        ]
    );
    assert_eq!(
        watch_of(&w, &key).unwrap().fired_for,
        Some(fingerprint("PR 12"))
    );
}

#[test]
fn does_not_look_while_the_automation_is_running() {
    let w = World::new();
    let key = watched(&w, "busy");
    let _held = acquire_run_lock(
        &w.paths,
        &RunLockFile {
            info: RunLockInfo {
                run_id: "r1".into(),
                pid: std::process::id(),
                pid_started_at: None,
                started_at: "2026-10-07T11:59:00.000Z".into(),
                trigger: Trigger::Watch,
                agent: None,
            },
            key: key.clone(),
            scheduled_for: None,
            budget_ms: Some(600_000),
        },
    )
    .unwrap();
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(0, "x", ""));
    let r = tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(scripts.count(), 0);
    assert!(r.checked.is_empty());
}

#[test]
fn checks_only_armed_automations() {
    let w = World::new();
    w.write("off", WATCH, &[("scripts/watch.js", "")]);
    write_automation(
        &w.root(),
        "disabled",
        WATCH,
        "enabled: false\n",
        &[("scripts/watch.js", "")],
    );
    // Armed, then the watch script changed: off.
    watched(&w, "edited");
    std::fs::write(
        w.root().join("edited").join("scripts").join("watch.js"),
        "console.log('changed')\n",
    )
    .unwrap();
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(0, "x", ""));
    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(scripts.count(), 0);
}

#[test]
fn writes_one_failed_record_triggered_by_the_watch_when_the_run_cannot_start() {
    let w = World::new();
    let key = watched(&w, "nostart");
    let starter = Starter::failing("no worker");
    let scripts = Scripts::new(|_| exited(0, "x", ""));
    let r = tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(r.failed, [(key.clone(), "no worker".to_string())]);
    let recs = w.records(&key);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].trigger, Trigger::Watch);
    assert_eq!(recs[0].outcome, Outcome::Failed);
    assert_eq!(
        recs[0].error.as_ref().unwrap().kind,
        fm_model::ErrorKind::Host
    );
}

#[test]
fn leaves_an_automation_alone_when_its_schedule_fired_in_the_same_tick() {
    let w = World::new();
    w.write(
        "both",
        &format!("  schedule:\n    cron: \"0 * * * *\"\n    timezone: UTC\n{WATCH}"),
        &[("scripts/watch.js", "")],
    );
    let key = w.arm("both", at("2026-10-07T11:00:00Z"));
    w.seed_last_tick("2026-10-07T11:59:00.000Z");
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(0, "x", ""));
    tick(&w.ctx(&starter, &scripts), at("2026-10-07T12:00:30Z")).unwrap();
    assert_eq!(
        starter.calls(),
        [Started {
            key,
            trigger: Trigger::Schedule,
            scheduled_for: Some("2026-10-07T12:00:00.000Z".into()),
            late: false,
        }]
    );
    assert_eq!(scripts.count(), 0);
}

#[test]
fn runs_no_script_for_a_paused_watch_and_runs_it_again_once_resumed() {
    let w = World::new();
    let key = watched(&w, "paused");
    let store = StateStore::new(&w.paths);
    store.pause_watch(&key, at(NOW)).unwrap();
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(0, "x", ""));
    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(scripts.count(), 0);
    store.resume_watch(&key).unwrap();
    tick(&w.ctx(&starter, &scripts), plus(1)).unwrap();
    assert_eq!(scripts.count(), 1);
    assert_eq!(starter.calls().len(), 1);
}

#[test]
fn starts_nothing_for_a_watch_paused_while_its_script_ran_and_keeps_the_finding_new() {
    let w = World::new();
    let key = watched(&w, "pause-mid");
    let k = key.clone();
    let scripts = Scripts::new(|_| {
        StateStore::new(&w.paths).pause_watch(&k, at(NOW)).unwrap();
        exited(0, "PR 12", "")
    });
    let starter = Starter::ok();
    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert!(starter.calls().is_empty());
    let s = watch_of(&w, &key).unwrap();
    assert_eq!(s.exit, Some(0));
    assert_eq!(s.fired_for, None);

    // Resumed: the same output is still new.
    StateStore::new(&w.paths).resume_watch(&key).unwrap();
    let again = Scripts::new(|_| exited(0, "PR 12", ""));
    let s2 = Starter::ok();
    tick(&w.ctx(&s2, &again), plus(5)).unwrap();
    assert_eq!(s2.calls().len(), 1);
}

#[test]
fn a_watch_whose_script_went_missing_is_off_and_never_run() {
    let w = World::new();
    let key = watched(&w, "gone");
    std::fs::remove_file(w.root().join("gone").join("scripts").join("watch.js")).unwrap();
    let starter = Starter::ok();
    let scripts = Scripts::new(|_| exited(0, "x", ""));
    tick(&w.ctx(&starter, &scripts), at(NOW)).unwrap();
    assert_eq!(scripts.count(), 0);
    assert!(watch_of(&w, &key).is_none());
}

/// The real thing: a real script through `fm_run::process::run`, when node is installed.
#[test]
fn a_real_watch_script_starts_a_real_check() {
    let Some(node) = fm_spec::which("node") else {
        eprintln!("skipped: node is not on PATH");
        return;
    };
    let w = World::new();
    w.write(
        "real",
        WATCH,
        &[(
            "scripts/watch.js",
            "if (process.stdin.isTTY) process.exit(9);\nconsole.log(process.env.AUTOMATION_PHASE + ' ' + process.env.AUTOMATION_NAME + ' ' + (process.env.WATCH_TEST_API_KEY || 'scrubbed'));\n",
        )],
    );
    let key = w.arm("real", at("2026-10-07T11:00:00Z"));
    let starter = Starter::ok();
    let mut ctx = w.ctx(&starter, &fm_clock::SystemScripts);
    let which = move |name: &str| (name == "node").then(|| node.clone());
    ctx.which = &which;
    let r = tick(&ctx, at(NOW)).unwrap();
    assert_eq!(
        r.fired,
        std::slice::from_ref(&key),
        "{:?}",
        watch_of(&w, &key)
    );
    let s = watch_of(&w, &key).unwrap();
    assert_eq!(s.said.as_deref(), Some("watch real scrubbed"));
    assert_eq!(s.fired_for, Some(fingerprint("watch real scrubbed")));
}

//! The tick against a real state folder and real discovery, with starting runs faked. Ported from
//! pi-automations' `test/tick.test.ts`.

mod common;

use common::*;
use fm_clock::tick;
use fm_model::{Outcome, RunLockInfo, TickSource, Trigger};
use fm_state::{RunLockFile, StateFileExt, acquire_run_lock, acquire_tick_lock};

const HOURLY: &str = "  schedule:\n    cron: \"0 * * * *\"\n    timezone: UTC\n";
const DAILY_ONCE: &str =
    "  schedule:\n    cron: \"0 7 * * *\"\n    timezone: UTC\n    catch-up: once\n";
const EVERY_MINUTE: &str = "  schedule:\n    cron: \"* * * * *\"\n    timezone: UTC\n";

fn lock_run(w: &World, key: &str, pid: u32, started: &str, budget_ms: u64) -> fm_state::Acquire {
    let file = RunLockFile {
        info: RunLockInfo {
            run_id: "in-flight".into(),
            pid,
            pid_started_at: None,
            started_at: started.into(),
            trigger: Trigger::Schedule,
            agent: None,
        },
        key: key.into(),
        scheduled_for: None,
        budget_ms: Some(budget_ms),
    };
    acquire_run_lock(&w.paths, &file).unwrap()
}

#[test]
fn claims_before_it_starts_so_a_crash_between_cannot_double_fire() {
    let w = World::new();
    w.write("a", HOURLY, &[]);
    let key = w.arm("a", at("2026-09-17T10:00:00Z"));
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let claimed_at_start = std::cell::RefCell::new(None);
    let mut starter = Starter::ok();
    starter.on_start = Some(Box::new(|s| {
        *claimed_at_start.borrow_mut() = w.state().claim_of(&s.key).map(str::to_string);
    }));
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();

    assert_eq!(r.fired, std::slice::from_ref(&key));
    assert_eq!(
        starter.calls(),
        [Started {
            key: key.clone(),
            trigger: Trigger::Schedule,
            scheduled_for: Some("2026-09-17T12:00:00.000Z".into()),
            late: false,
        }]
    );
    // The claim was on disk when the run started, not just in memory.
    assert_eq!(
        claimed_at_start.borrow().as_deref(),
        Some("2026-09-17T12:00:00.000Z")
    );
}

#[test]
fn does_not_fire_the_same_instant_twice() {
    let w = World::new();
    w.write("a", HOURLY, &[]);
    w.arm("a", at("2026-09-17T10:00:00Z"));
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let scripts = no_scripts();
    let first = Starter::ok();
    tick(&w.ctx(&first, &scripts), at("2026-09-17T12:00:30Z")).unwrap();
    assert_eq!(first.calls().len(), 1);

    // A second tick in the same minute (its lastTickAt rewound by hand): the claim stops it.
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let second = Starter::ok();
    tick(&w.ctx(&second, &scripts), at("2026-09-17T12:00:40Z")).unwrap();
    assert!(second.calls().is_empty());
}

#[test]
fn records_skipped_rather_than_queueing_while_a_run_is_going() {
    let w = World::new();
    w.write("a", HOURLY, &[]);
    let key = w.arm("a", at("2026-09-17T10:00:00Z"));
    let pid = 424_242;
    let probe = fm_state::FakeProbe::new()
        .alive(std::process::id(), None)
        .alive(pid, None);
    let _held = lock_run(&w, &key, pid, "2026-09-17T11:55:00.000Z", 600_000);
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();
    let mut ctx = w.ctx(&starter, &scripts);
    ctx.probe = &probe;

    let r = tick(&ctx, at("2026-09-17T12:00:30Z")).unwrap();

    assert!(starter.calls().is_empty());
    assert_eq!(r.skipped, std::slice::from_ref(&key));
    let recs = w.records(&key);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].outcome, Outcome::Skipped);
    assert_eq!(
        recs[0].reason.as_deref(),
        Some("still running since 2026-09-17T11:55:00.000Z")
    );
    assert_eq!(
        recs[0].scheduled_for.as_deref(),
        Some("2026-09-17T12:00:00.000Z")
    );
}

#[test]
fn writes_one_missed_record_with_a_count() {
    let w = World::new();
    w.write("a", HOURLY, &[]);
    let key = w.arm("a", at("2026-09-17T08:00:00Z"));
    w.seed_last_tick("2026-09-17T08:30:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:30:00Z")).unwrap();

    assert!(starter.calls().is_empty());
    assert_eq!(r.missed, std::slice::from_ref(&key));
    let recs = w.records(&key);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].outcome, Outcome::Missed);
    assert_eq!(recs[0].reason.as_deref(), Some("host unavailable"));
    assert_eq!(recs[0].count, Some(4));
    assert_eq!(recs[0].trigger, Trigger::Schedule);
    assert_eq!(
        recs[0].scheduled_for.as_deref(),
        Some("2026-09-17T12:00:00.000Z")
    );
    // Claimed, so the next tick does not report it again.
    assert_eq!(w.state().claim_of(&key), Some("2026-09-17T12:00:00.000Z"));
    let again = Starter::ok();
    let r2 = tick(&w.ctx(&again, &scripts), at("2026-09-17T12:31:00Z")).unwrap();
    assert!(r2.missed.is_empty());
    assert_eq!(w.records(&key).len(), 1);
}

#[test]
fn marks_a_late_catch_up_run_and_records_the_older_instants_missed() {
    let w = World::new();
    w.write("a", DAILY_ONCE, &[]);
    let key = w.arm("a", at("2026-09-14T00:00:00Z"));
    w.seed_last_tick("2026-09-15T06:00:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();

    tick(&w.ctx(&starter, &scripts), at("2026-09-17T09:14:00Z")).unwrap();

    assert_eq!(
        starter.calls(),
        [Started {
            key: key.clone(),
            trigger: Trigger::Schedule,
            scheduled_for: Some("2026-09-17T07:00:00.000Z".into()),
            late: true,
        }]
    );
    let recs = w.records(&key);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].outcome, Outcome::Missed);
    assert_eq!(recs[0].count, Some(2));
    assert_eq!(
        recs[0].scheduled_for.as_deref(),
        Some("2026-09-16T07:00:00.000Z")
    );
}

#[test]
fn ignores_automations_that_are_not_armed_or_have_no_schedule() {
    let w = World::new();
    w.write("off", HOURLY, &[]);
    write_automation(&w.root(), "disabled", HOURLY, "enabled: false\n", &[]);
    w.write("manual", "  manual: true\n", &[]);
    w.arm("manual", at("2026-09-17T10:00:00Z"));
    // Armed, then changed: off again.
    w.write("changed", HOURLY, &[]);
    w.arm("changed", at("2026-09-17T10:00:00Z"));
    write_automation(&w.root(), "changed", HOURLY, "timeout: 5m\n", &[]);
    // Invalid.
    std::fs::create_dir_all(w.root().join("broken")).unwrap();
    std::fs::write(
        w.root().join("broken").join("AUTOMATION.md"),
        "no frontmatter",
    )
    .unwrap();
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();

    assert!(starter.calls().is_empty());
    assert_eq!(r.considered, 0);
}

#[test]
fn an_automation_that_fails_its_machine_check_is_not_fired() {
    let w = World::new();
    w.write(
        "needs-node",
        &format!("{HOURLY}  watch:\n    script: scripts/watch.js\n    every: 5m\n"),
        &[("scripts/watch.js", "")],
    );
    w.arm("needs-node", at("2026-09-17T10:00:00Z"));
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();
    let mut ctx = w.ctx(&starter, &scripts);
    ctx.which = &|_| None;

    let r = tick(&ctx, at("2026-09-17T12:00:30Z")).unwrap();

    assert_eq!(r.considered, 0);
    assert!(starter.calls().is_empty());
}

#[test]
fn sweeps_an_abandoned_run_lock_and_records_it() {
    let w = World::new();
    w.write("a", "  manual: true\n", &[]);
    let key = w.arm("a", at("2026-09-15T00:00:00Z"));
    let _held = lock_run(&w, &key, 1 << 30, "2026-09-16T00:00:00.000Z", 1000);
    let starter = Starter::ok();
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();

    assert_eq!(r.swept, std::slice::from_ref(&key));
    let recs = w.records(&key);
    assert_eq!(recs[0].outcome, Outcome::Aborted);
    assert_eq!(recs[0].reason.as_deref(), Some("host stopped"));
}

#[test]
fn advances_last_tick_and_says_who_ticked_even_when_nothing_was_due() {
    let w = World::new();
    let starter = Starter::ok();
    let scripts = no_scripts();
    let mut ctx = w.ctx(&starter, &scripts);
    ctx.by = TickSource::Cli;
    tick(&ctx, at("2026-09-17T12:30:00Z")).unwrap();
    let st = w.state();
    assert_eq!(st.last_tick_at.as_deref(), Some("2026-09-17T12:30:00.000Z"));
    assert_eq!(st.last_tick_by, Some(TickSource::Cli));
}

/// An automation named so its stagger is at least `min` seconds (the stagger is a hash of the key).
fn staggered(w: &World, min: u64) -> (String, u64) {
    let cron = fm_spec::CronSchedule::new("* * * * *", Some("UTC")).unwrap();
    for i in 0..200 {
        let name = format!("s{i}");
        let key = fm_spec::automation_key(fm_model::Scope::User, &w.root(), &name);
        let st = fm_clock::stagger_for(&key, &cron, 30, at("2026-09-17T12:00:00Z"));
        if st >= min {
            w.write(&name, EVERY_MINUTE, &[]);
            let armed = w.arm(&name, at("2026-09-17T11:00:00Z"));
            assert_eq!(armed, key);
            return (key, st);
        }
    }
    panic!("no name with a stagger of {min}s");
}

#[test]
fn does_not_advance_last_tick_past_an_instant_held_for_its_stagger() {
    let w = World::new();
    let (key, _st) = staggered(&w, 5);
    w.seed_last_tick("2026-09-17T11:59:02.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:02Z")).unwrap();

    assert_eq!(r.waiting, [key]);
    assert!(starter.calls().is_empty());
    assert_eq!(
        w.state().last_tick_at.as_deref(),
        Some("2026-09-17T11:59:59.999Z")
    );
}

#[test]
fn fires_the_newest_ready_instant_and_keeps_firing_minute_after_minute() {
    let w = World::new();
    let (key, _st) = staggered(&w, 5);
    w.seed_last_tick("2026-09-17T11:59:02.000Z");
    let scripts = no_scripts();

    let mut fired = Vec::new();
    for minute in 0..6 {
        let starter = Starter::ok();
        let now = at(&format!("2026-09-17T12:{minute:02}:02Z"));
        tick(&w.ctx(&starter, &scripts), now).unwrap();
        fired.extend(
            starter
                .calls()
                .into_iter()
                .map(|s| s.scheduled_for.unwrap()),
        );
    }
    // One warm-up tick while the first instant holds, then one fire per tick, in order.
    assert_eq!(
        fired,
        [
            "2026-09-17T12:00:00.000Z",
            "2026-09-17T12:01:00.000Z",
            "2026-09-17T12:02:00.000Z",
            "2026-09-17T12:03:00.000Z",
            "2026-09-17T12:04:00.000Z",
        ]
    );
    assert!(w.records(&key).is_empty());
}

#[test]
fn a_confirmed_failed_start_leaves_one_failed_record_and_keeps_the_claim() {
    let w = World::new();
    w.write("a", HOURLY, &[]);
    let key = w.arm("a", at("2026-09-17T10:00:00Z"));
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let starter = Starter::failing("fm.exe not found");
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();

    assert_eq!(r.failed, [(key.clone(), "fm.exe not found".to_string())]);
    assert!(r.fired.is_empty());
    let recs = w.records(&key);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].outcome, Outcome::Failed);
    assert_eq!(recs[0].trigger, Trigger::Schedule);
    assert_eq!(
        recs[0].scheduled_for.as_deref(),
        Some("2026-09-17T12:00:00.000Z")
    );
    let err = recs[0].error.as_ref().unwrap();
    assert_eq!(err.kind, fm_model::ErrorKind::Host);
    assert_eq!(
        err.message,
        "the run could not be started: fm.exe not found"
    );
    assert_eq!(w.state().claim_of(&key), Some("2026-09-17T12:00:00.000Z"));
}

#[test]
fn one_tick_at_a_time() {
    let w = World::new();
    w.write("a", HOURLY, &[]);
    w.arm("a", at("2026-09-17T10:00:00Z"));
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let held = acquire_tick_lock(&w.paths, 300_000, &w.probe, at("2026-09-17T12:00:00Z"))
        .unwrap()
        .unwrap();
    let starter = Starter::ok();
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();
    assert!(r.locked);
    assert!(starter.calls().is_empty());

    // A lock older than five minutes is abandoned, whoever holds it.
    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:06:00Z")).unwrap();
    assert!(!r.locked);
    drop(held);
}

#[test]
fn fires_repository_automations_from_registered_roots() {
    let w = World::new();
    let repo = w.tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let root = automations_in(&repo);
    write_automation(&root, "r", HOURLY, "", &[]);
    let l = located_in(
        &[fm_model::Root {
            path: fm_spec::normalize(&root),
            scope: fm_model::Scope::Repository,
        }],
        "r",
    );
    arm(&w.paths, &l, at("2026-09-17T10:00:00Z"));
    fm_state::StateStore::new(&w.paths)
        .register_root(&repo)
        .unwrap();
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();

    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();

    assert_eq!(r.fired, std::slice::from_ref(&l.key));
    assert!(l.key.starts_with("repository:"));
}

#[test]
fn never_starts_an_automation_nobody_armed() {
    let w = World::new();
    w.write("unarmed", EVERY_MINUTE, &[]);
    w.seed_last_tick("2026-09-17T11:59:02.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();
    let r = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:01:02Z")).unwrap();
    assert!(starter.calls().is_empty());
    assert_eq!(r.considered, 0);
}

#[test]
fn forgets_claims_of_automations_that_are_gone() {
    let w = World::new();
    fm_state::StateStore::new(&w.paths)
        .claim("user:/gone:x", "2026-09-17T07:00:00.000Z")
        .unwrap();
    let starter = Starter::ok();
    let scripts = no_scripts();
    tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap();
    assert!(w.state().claimed.is_empty());
}

#[test]
fn a_config_fm_cannot_use_stops_the_tick_in_one_line() {
    let w = World::new();
    std::fs::write(w.paths.config(), "nonsense = 1\n").unwrap();
    let starter = Starter::ok();
    let scripts = no_scripts();
    let err = tick(&w.ctx(&starter, &scripts), at("2026-09-17T12:00:30Z")).unwrap_err();
    assert!(err.to_string().contains("nonsense"), "{err}");
    assert!(!err.to_string().contains('\n'));
}

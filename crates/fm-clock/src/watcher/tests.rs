//! The watcher verbs against a fake scheduler and a fake clock: nothing is registered for real.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use fm_model::{StatePaths, TickSource};
use fm_state::{Os, StateStore};

use super::*;

struct FakeExec<'s> {
    calls: RefCell<Vec<Cmd>>,
    answer: Box<dyn Fn(&Cmd) -> ExecResult + 's>,
}

impl<'s> FakeExec<'s> {
    fn new(answer: impl Fn(&Cmd) -> ExecResult + 's) -> Self {
        FakeExec {
            calls: RefCell::new(Vec::new()),
            answer: Box::new(answer),
        }
    }
    fn lines(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|c| c.to_string()).collect()
    }
}

impl Exec for FakeExec<'_> {
    fn exec(&self, cmd: &Cmd) -> ExecResult {
        self.calls.borrow_mut().push(cmd.clone());
        (self.answer)(cmd)
    }
}

fn ok(stdout: &str) -> ExecResult {
    ExecResult {
        code: Some(0),
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

fn fail(code: i32, stderr: &str) -> ExecResult {
    ExecResult {
        code: Some(code),
        stdout: String::new(),
        stderr: stderr.into(),
    }
}

struct FakeClock {
    now: Cell<DateTime<Utc>>,
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        self.now.get()
    }
    fn sleep(&self, d: Duration) {
        self.now
            .set(self.now.get() + chrono::Duration::from_std(d).unwrap());
    }
}

fn t0() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-29T17:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

struct World {
    _tmp: tempfile::TempDir,
    paths: StatePaths,
    home: PathBuf,
    exe: PathBuf,
    clock: FakeClock,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(tmp.path().join("state"));
    fm_state::open_state(&paths).unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let exe = tmp.path().join("bin").join("fm");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    World {
        _tmp: tmp,
        paths,
        home,
        exe,
        clock: FakeClock {
            now: Cell::new(t0()),
        },
    }
}

fn ctx<'a>(w: &'a World, os: Os, exec: &'a dyn Exec, wait: Duration) -> WatcherContext<'a> {
    WatcherContext {
        os,
        paths: w.paths.clone(),
        home: w.home.clone(),
        exec,
        env: vec![
            ("PATH".into(), "/usr/local/bin:/usr/bin".into()),
            ("GITHUB_TOKEN".into(), "ghp_not_a_real_token".into()),
        ],
        names: Names::default(),
        system_root: w.home.join("Windows"),
        wait,
        clock: &w.clock,
        current_exe: Some(w.exe.clone()),
    }
}

/// A scheduler that ticks when asked to run the job now.
fn ticks_on_run_now(paths: &StatePaths, clock: &FakeClock, cmd: &Cmd) -> ExecResult {
    let run_now = cmd
        .args
        .first()
        .is_some_and(|a| a == "/Run" || a == "kickstart")
        || cmd.args.iter().any(|a| a == "--no-block");
    if run_now {
        StateStore::new(paths)
            .note_tick(clock.now(), TickSource::Job)
            .unwrap();
    }
    if cmd.program == "id" {
        return ok("501\n");
    }
    ok("")
}

#[test]
fn windows_start_writes_the_task_registers_it_and_waits_for_a_real_tick() {
    let w = world();
    let exec = FakeExec::new(|c| ticks_on_run_now(&w.paths, &w.clock, c));
    let c = ctx(&w, Os::Windows, &exec, FIRST_TICK_WAIT);
    let r = start(&c, &w.exe);
    assert!(r.ok, "{:?}", r.lines);
    assert_eq!(
        r.lines,
        [
            "not carried into the job: GITHUB_TOKEN — credentials never are",
            r"watcher: on (schtasks) · agent-automations\tick",
            "first tick ok",
        ]
    );
    let xml_file = w.paths.root.join("watcher").join("tick.xml");
    let bytes = std::fs::read(&xml_file).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
    let cmd_text =
        std::fs::read_to_string(w.paths.root.join("watcher").join("fm-tick.cmd")).unwrap();
    assert!(cmd_text.contains(&format!("\"{}\" tick --by job", w.exe.display())));
    assert!(!cmd_text.contains("ghp_not_a_real_token"));
    let calls = exec.lines();
    assert_eq!(
        calls[0],
        format!(
            r"schtasks /Create /TN agent-automations\tick /XML {} /F",
            xml_file.display()
        )
    );
    assert_eq!(calls[1], r"schtasks /Run /TN agent-automations\tick");
    let rec = read_record(&w.paths).unwrap();
    assert_eq!(rec.method, Method::Schtasks);
    assert_eq!(rec.exe, w.exe);
    assert_eq!(rec.refused, ["GITHUB_TOKEN"]);
    assert!(watcher_registered(&w.paths));
}

#[test]
fn a_registration_failure_is_reported_not_claimed() {
    let w = world();
    let exec = FakeExec::new(|c| {
        if c.args.first().is_some_and(|a| a == "/Create") {
            fail(1, "ERROR: Access is denied.\r\n")
        } else {
            ok("")
        }
    });
    let r = start(&ctx(&w, Os::Windows, &exec, FIRST_TICK_WAIT), &w.exe);
    assert!(!r.ok);
    assert_eq!(
        r.lines.last().unwrap(),
        "watcher: could not register the job — ERROR: Access is denied."
    );
    assert!(!watcher_registered(&w.paths));
}

#[test]
fn verification_fails_when_no_tick_arrives() {
    let w = world();
    let exec = FakeExec::new(|_| ok(""));
    let r = start(&ctx(&w, Os::Windows, &exec, FIRST_TICK_WAIT), &w.exe);
    assert!(!r.ok);
    let last = r.lines.last().unwrap();
    assert!(
        last.starts_with("first tick did not arrive within 90s — check "),
        "{last}"
    );
    assert!(last.ends_with("and fm watcher status"));
    // It waited on the fake clock, not the real one.
    assert!(w.clock.now() - t0() >= chrono::Duration::seconds(90));
}

#[test]
fn a_tick_by_serve_is_not_the_job_s_first_tick() {
    let w = world();
    let exec = FakeExec::new(|c| {
        if c.args.first().is_some_and(|a| a == "/Run") {
            StateStore::new(&w.paths)
                .note_tick(w.clock.now(), TickSource::Cli)
                .unwrap();
        }
        ok("")
    });
    let r = start(&ctx(&w, Os::Windows, &exec, Duration::from_secs(5)), &w.exe);
    assert!(!r.ok);
}

#[test]
fn refuses_while_fm_serve_is_ticking() {
    let w = world();
    StateStore::new(&w.paths)
        .note_tick(t0() - chrono::Duration::seconds(30), TickSource::Serve)
        .unwrap();
    let exec = FakeExec::new(|_| ok(""));
    let r = start(&ctx(&w, Os::Windows, &exec, Duration::ZERO), &w.exe);
    assert!(!r.ok);
    assert_eq!(
        r.lines,
        [
            "watcher: fm serve is ticking — one clock at a time: stop fm serve, then fm watcher start"
        ]
    );
    assert!(exec.lines().is_empty());
}

#[test]
fn refuses_an_exe_that_is_not_there() {
    let w = world();
    let exec = FakeExec::new(|_| ok(""));
    let r = start(
        &ctx(&w, Os::Windows, &exec, Duration::ZERO),
        &w.home.join("nope"),
    );
    assert!(!r.ok);
    assert!(r.lines[0].starts_with("watcher: "), "{:?}", r.lines);
    assert!(r.lines[0].contains("not found"));
}

#[test]
fn macos_start_writes_the_agent_and_bootstraps_it_in_the_login_session() {
    let w = world();
    let exec = FakeExec::new(|c| ticks_on_run_now(&w.paths, &w.clock, c));
    let r = start(&ctx(&w, Os::MacOs, &exec, FIRST_TICK_WAIT), &w.exe);
    assert!(r.ok, "{:?}", r.lines);
    let plist = w
        .home
        .join("Library")
        .join("LaunchAgents")
        .join("io.agentautomations.fm.tick.plist");
    let text = std::fs::read_to_string(&plist).unwrap();
    assert!(text.contains("<string>/usr/local/bin:/usr/bin</string>"));
    let calls = exec.lines();
    assert_eq!(calls[0], "id -u");
    assert_eq!(
        calls[1],
        "launchctl bootout gui/501/io.agentautomations.fm.tick"
    );
    assert_eq!(
        calls[2],
        format!("launchctl bootstrap gui/501 {}", plist.display())
    );
    assert_eq!(
        calls[3],
        "launchctl kickstart gui/501/io.agentautomations.fm.tick"
    );
    let sh = std::fs::read_to_string(w.paths.root.join("watcher").join("fm-tick.sh")).unwrap();
    assert!(sh.contains("tick --by job"));
    assert!(sh.contains("launchctl bootout"));
}

#[test]
fn linux_start_uses_a_systemd_user_timer_when_there_is_one() {
    let w = world();
    let exec = FakeExec::new(|c| ticks_on_run_now(&w.paths, &w.clock, c));
    let r = start(&ctx(&w, Os::Linux, &exec, FIRST_TICK_WAIT), &w.exe);
    assert!(r.ok, "{:?}", r.lines);
    assert_eq!(r.lines[1], "watcher: on (systemd) · fm-tick.timer");
    let units = w.home.join(".config").join("systemd").join("user");
    assert!(units.join("fm-tick.service").is_file());
    assert!(units.join("fm-tick.timer").is_file());
    assert_eq!(
        exec.lines(),
        [
            "systemctl --user show-environment",
            "systemctl --user daemon-reload",
            "systemctl --user enable --now fm-tick.timer",
            "systemctl --user start --no-block fm-tick.service",
        ]
    );
}

#[test]
fn linux_falls_back_to_crontab_without_systemd() {
    let w = world();
    let exec = FakeExec::new(|c| match c.program.as_str() {
        "systemctl" => fail(1, "Failed to connect to bus"),
        "crontab" if c.args == ["-l"] => ok("0 3 * * * backup\n"),
        _ => ok(""),
    });
    let r = start(&ctx(&w, Os::Linux, &exec, Duration::ZERO), &w.exe);
    assert!(r.ok, "{:?}", r.lines);
    assert_eq!(r.lines[1], "watcher: on (crontab) · # fm-tick (fm watcher)");
    let calls = exec.calls.borrow();
    let install = calls.iter().find(|c| c.args == ["-"]).unwrap();
    let text = install.stdin.as_deref().unwrap();
    assert!(text.starts_with("0 3 * * * backup\n* * * * * /bin/sh "));
    assert!(text.ends_with("# fm-tick (fm watcher)\n"));
}

#[test]
fn linux_without_systemd_or_cron_points_at_serve() {
    let w = world();
    let exec = FakeExec::new(|c| match c.program.as_str() {
        "systemctl" => fail(1, "no bus"),
        _ => ExecResult {
            code: None,
            stdout: String::new(),
            stderr: "not found".into(),
        },
    });
    let r = start(&ctx(&w, Os::Linux, &exec, Duration::ZERO), &w.exe);
    assert!(!r.ok);
    assert!(
        r.lines
            .last()
            .unwrap()
            .ends_with("keep fm serve running instead")
    );
}

#[test]
fn stop_removes_the_job_and_its_files() {
    let w = world();
    let exec = FakeExec::new(|_| ok(""));
    let c = ctx(&w, Os::Windows, &exec, Duration::ZERO);
    assert!(start(&c, &w.exe).ok);
    let r = stop(&c);
    assert!(r.ok);
    assert_eq!(r.lines, ["watcher: off — job removed (schtasks)"]);
    assert!(
        exec.lines()
            .contains(&r"schtasks /Delete /TN agent-automations\tick /F".to_string())
    );
    assert!(!watcher_registered(&w.paths));
    assert!(!w.paths.root.join("watcher").join("tick.xml").exists());
    assert!(!w.paths.root.join("watcher").join("fm-tick.cmd").exists());
}

#[test]
fn stop_is_safe_when_nothing_is_registered() {
    let w = world();
    let exec = FakeExec::new(|_| fail(1, "ERROR: The system cannot find the file specified."));
    let r = stop(&ctx(&w, Os::Windows, &exec, Duration::ZERO));
    assert!(r.ok);
    assert_eq!(r.lines, ["watcher: off — it was not registered"]);
}

#[test]
fn stop_on_linux_removes_the_units_and_the_crontab_line() {
    let w = world();
    let mine = "* * * * * /bin/sh '/x/fm-tick.sh' >> '/x/tick.log' 2>&1 # fm-tick (fm watcher)";
    let exec = FakeExec::new(
        |c| match (c.program.as_str(), c.args.first().map(String::as_str)) {
            ("crontab", Some("-l")) => ok(&format!("0 3 * * * backup\n{mine}\n")),
            _ => ok(""),
        },
    );
    let r = stop(&ctx(&w, Os::Linux, &exec, Duration::ZERO));
    assert_eq!(r.lines, ["watcher: off — job removed (systemd, crontab)"]);
    let calls = exec.calls.borrow();
    let install = calls.iter().find(|c| c.args == ["-"]).unwrap();
    assert_eq!(install.stdin.as_deref(), Some("0 3 * * * backup\n"));
}

fn status_of(w: &World, exec: &dyn Exec, deep: bool) -> WatcherStatus {
    status(&ctx(w, Os::Windows, exec, Duration::ZERO), deep)
}

#[test]
fn status_is_off_without_a_job() {
    let w = world();
    let exec = FakeExec::new(|_| ok(""));
    let s = status_of(&w, &exec, true);
    assert_eq!(s.state, WatcherState::Off);
    assert_eq!(s.summary, "off — fm watcher start");
    assert_eq!(
        s.lines[0],
        "watcher: off — no schedule fires and no watch is checked on its own"
    );
    assert!(exec.lines().is_empty());
}

#[test]
fn status_names_fm_serve_when_it_is_the_clock() {
    let w = world();
    StateStore::new(&w.paths)
        .note_tick(t0() - chrono::Duration::seconds(20), TickSource::Serve)
        .unwrap();
    let exec = FakeExec::new(|_| ok(""));
    let s = status_of(&w, &exec, false);
    assert_eq!(s.state, WatcherState::Serve);
    assert_eq!(s.summary, "on (fm serve) — last tick 20s ago");
}

#[test]
fn status_is_on_when_a_job_ticks_without_a_record() {
    let w = world();
    StateStore::new(&w.paths)
        .note_tick(t0() - chrono::Duration::seconds(12), TickSource::Job)
        .unwrap();
    let exec = FakeExec::new(|_| ok(""));
    let s = status_of(&w, &exec, false);
    assert_eq!(s.state, WatcherState::On);
    assert_eq!(s.summary, "on — last tick 12s ago");
}

#[test]
fn status_is_on_while_the_job_ticks_and_late_once_it_goes_quiet() {
    let w = world();
    let exec = FakeExec::new(|_| ok(""));
    let c = ctx(&w, Os::Windows, &exec, Duration::ZERO);
    assert!(start(&c, &w.exe).ok);
    assert_eq!(status(&c, false).summary, "on — waiting for the first tick");

    StateStore::new(&w.paths)
        .note_tick(t0() + chrono::Duration::seconds(48), TickSource::Job)
        .unwrap();
    w.clock.now.set(t0() + chrono::Duration::seconds(60));
    let s = status(&c, true);
    assert_eq!(s.state, WatcherState::On);
    assert_eq!(s.summary, "on — last tick 12s ago");
    assert!(s.lines.contains(&"  job: present".to_string()));

    w.clock.now.set(t0() + chrono::Duration::minutes(15));
    let s = status(&c, false);
    assert_eq!(s.state, WatcherState::Late);
    assert_eq!(s.summary, "late — last tick 14m ago — fm watcher start");
}

#[test]
fn status_says_when_the_scheduler_lost_the_job_or_fm_moved() {
    let w = world();
    let exec = FakeExec::new(|c| {
        if c.args.first().is_some_and(|a| a == "/Query") {
            fail(1, "ERROR: The system cannot find the file specified.")
        } else {
            ok("")
        }
    });
    let c = ctx(&w, Os::Windows, &exec, Duration::ZERO);
    assert!(start(&c, &w.exe).ok);
    let s = status(&c, true);
    assert_eq!(s.state, WatcherState::Late);
    assert_eq!(
        s.summary,
        "late — the scheduler lost the job — fm watcher start"
    );

    let mut other = ctx(&w, Os::Windows, &exec, Duration::ZERO);
    other.current_exe = Some(w.home.join("other-fm"));
    assert_eq!(
        status(&other, false).summary,
        "late — registered for another executable — review before migrating"
    );

    std::fs::remove_file(&w.exe).unwrap();
    let s = status(&c, false);
    assert_eq!(s.state, WatcherState::Late);
    assert!(
        s.summary.ends_with("is gone — fm watcher start"),
        "{}",
        s.summary
    );
}

#[test]
fn job_names_never_collide_with_pi_automations() {
    let n = Names::default();
    assert_eq!(n.task, r"agent-automations\tick");
    assert_eq!(n.label, "io.agentautomations.fm.tick");
    assert_eq!(n.unit, "fm-tick");
    assert_ne!(n.task, r"pi-automations\tick");
    assert_ne!(n.label, "io.agentautomations.pi.tick");
    assert_ne!(n.unit, "pi-automations-tick");
    let t = Names::test("abcd");
    assert_eq!(t.task, "fm-test-abcd");
}

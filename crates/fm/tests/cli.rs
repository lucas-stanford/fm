//! The `fm` command line, end to end, on a state folder and home under the target folder.

mod common;

use common::{Fx, fm_exe, toml_path};
use predicates::prelude::*;
use predicates::str::contains;

fn json_list(fx: &Fx) -> Vec<serde_json::Value> {
    let out = fx.fm().args(["list", "--json"]).output().unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

fn state_of(fx: &Fx, name: &str) -> String {
    json_list(fx)
        .iter()
        .find(|s| s["name"] == name)
        .map(|s| s["state"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

#[test]
fn help_version_and_usage_errors() {
    let fx = Fx::new();
    fx.fm()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: fm"))
        .stdout(contains("fm step <node>"));
    fx.fm()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("fm {}\n", env!("CARGO_PKG_VERSION")));
    fx.fm().arg("frobnicate").assert().code(1);
    fx.fm().arg("arm").assert().code(1);
    fx.fm()
        .args(["agent", "default", "vim"])
        .assert()
        .code(1)
        .stderr(contains("unknown agent CLI \"vim\""));
}

#[test]
fn list_empty() {
    let fx = Fx::new();
    fx.fm()
        .arg("list")
        .assert()
        .success()
        .stdout(contains("USER · "))
        .stdout(contains("none"));
    assert!(json_list(&fx).is_empty());
}

#[test]
fn list_shows_each_state() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.write(
        &fx.user_root(),
        "resting",
        "enabled: false\non:\n  manual: true\n",
    );
    fx.write(&fx.user_root(), "Bad_Name", "on:\n  manual: true\n");
    fx.fm()
        .arg("list")
        .assert()
        .success()
        .stdout(contains("hello"))
        .stdout(contains("not armed — fm arm hello"))
        .stdout(contains("disabled"))
        .stdout(contains("invalid"));
    let all = json_list(&fx);
    assert_eq!(all.len(), 3);
    assert_eq!(state_of(&fx, "hello"), "off");
    assert_eq!(state_of(&fx, "resting"), "disabled");
    assert_eq!(state_of(&fx, "Bad_Name"), "invalid");
}

#[test]
fn check_shows_the_preview_with_the_exact_agent_command() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.config(&format!(
        "default-agent = \"copilot\"\n[agents.copilot]\npath = {}\n",
        toml_path(&fm_exe())
    ));
    fx.fm()
        .args(["check", "hello"])
        .assert()
        .success()
        .stdout(contains("hello  off"))
        .stdout(contains(fx.user_key("hello")))
        .stdout(contains("Consent preview for hello"))
        .stdout(contains(format!(
            "{} --output-format json",
            fm_exe().display()
        )))
        .stdout(contains("< <prompt>"));
    fx.fm()
        .arg("check")
        .assert()
        .success()
        .stdout(contains("hello  off  — not armed"));
}

#[test]
fn check_warns_when_no_agent_is_chosen() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.fm()
        .args(["check", "hello"])
        .assert()
        .success()
        .stdout(contains("no agent CLI chosen — `fm agent default <cli>`"));
}

#[test]
fn arm_needs_yes_off_a_terminal_then_disarm() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.fm()
        .args(["arm", "hello"])
        .assert()
        .code(2)
        .stdout(contains("Arm hello"))
        .stderr(contains("not armed — re-run with --yes"));
    assert_eq!(state_of(&fx, "hello"), "off");

    fx.config(&format!(
        "default-agent = \"claude\"\n[agents.claude]\npath = {}\n",
        toml_path(&fm_exe())
    ));
    fx.fm()
        .args(["arm", "hello", "--yes"])
        .assert()
        .success()
        .stdout(contains("hello armed"));
    assert_eq!(state_of(&fx, "hello"), "armed");
    let trust = std::fs::read_to_string(fx.state.join("trust.json")).unwrap();
    assert!(trust.contains("agentCommand"), "{trust}");
    assert!(trust.contains("--output-format stream-json"), "{trust}");

    fx.fm()
        .args(["arm", "hello"])
        .assert()
        .success()
        .stdout(contains("already armed"));
    fx.fm()
        .args(["disarm", "hello"])
        .assert()
        .success()
        .stdout(contains("hello disarmed"));
    assert_eq!(state_of(&fx, "hello"), "off");
}

#[test]
fn a_changed_file_turns_it_off() {
    let fx = Fx::new();
    let file = fx.manual("hello");
    fx.fm().args(["arm", "hello", "--yes"]).assert().success();
    std::fs::write(
        &file,
        std::fs::read_to_string(&file).unwrap() + "Say it twice.\n",
    )
    .unwrap();
    fx.fm().arg("list").assert().success().stdout(contains(
        "changed since armed: AUTOMATION.md — fm arm hello",
    ));
}

#[test]
fn enable_and_disable_keep_consent() {
    let fx = Fx::new();
    let file = fx.manual("hello");
    fx.fm().args(["arm", "hello", "--yes"]).assert().success();
    fx.fm()
        .args(["disable", "hello"])
        .assert()
        .success()
        .stdout(contains("hello disabled"));
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("enabled: false")
    );
    assert_eq!(state_of(&fx, "hello"), "disabled");
    fx.fm()
        .args(["disable", "hello"])
        .assert()
        .success()
        .stdout(contains("already disabled"));
    fx.fm()
        .args(["enable", "hello"])
        .assert()
        .success()
        .stdout(contains("hello enabled"));
    assert_eq!(state_of(&fx, "hello"), "armed");
}

#[test]
fn enable_refuses_a_repository_automation() {
    let fx = Fx::new();
    fx.write(&fx.repo_root(), "shared", "on:\n  manual: true\n");
    fx.fm()
        .args(["disable", "shared"])
        .assert()
        .code(2)
        .stderr(contains("edit and commit it"));
}

#[test]
fn arming_a_repository_automation_registers_its_root() {
    let fx = Fx::new();
    fx.write(&fx.repo_root(), "shared", "on:\n  manual: true\n");
    fx.fm()
        .args(["arm", "shared", "--yes"])
        .assert()
        .success()
        .stdout(contains("Arm shared"))
        .stdout(contains("expires"));
    let state = std::fs::read_to_string(fx.state.join("state.json")).unwrap();
    assert!(state.contains("roots"), "{state}");
    // From another folder, the registered root still finds it.
    let mut c = fx.fm();
    c.current_dir(&fx.home);
    let out = c.args(["list", "--json"]).output().unwrap();
    let all: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["state"], "armed");
}

#[test]
fn names_resolve_by_name_prefix_or_key_and_refuse_ambiguity() {
    let fx = Fx::new();
    fx.manual("daily");
    fx.manual("daily-notes");
    fx.write(&fx.repo_root(), "daily", "on:\n  manual: true\n");
    fx.fm()
        .args(["check", "daily"])
        .assert()
        .code(2)
        .stderr(contains("ambiguous \"daily\""))
        .stderr(contains(fx.user_key("daily")))
        .stderr(contains(fx.repo_key("daily")))
        .stderr(contains("— use the full key"));
    fx.fm()
        .args(["check", "repository/daily"])
        .assert()
        .success()
        .stdout(contains(fx.repo_key("daily")));
    fx.fm()
        .args(["check", &fx.user_key("daily")])
        .assert()
        .success()
        .stdout(contains("daily  off"));
    fx.fm()
        .args(["check", "daily-n"])
        .assert()
        .success()
        .stdout(contains("daily-notes  off"));
    fx.fm()
        .args(["check", "nope"])
        .assert()
        .code(2)
        .stderr(contains("no automation named \"nope\""));
}

#[test]
fn log_shows_runs_with_notes_links_and_message() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.fm()
        .args(["log", "hello"])
        .assert()
        .success()
        .stdout(contains("hello has not run yet"));
    let key = fx.user_key("hello");
    let paths = fm_model::StatePaths::new(&fx.state);
    for (id, outcome) in [
        ("20261029-090000-aaaa", "failed"),
        ("20261029-100000-bbbb", "ok"),
    ] {
        let rec = serde_json::json!({
            "id": id, "key": key, "name": "hello", "scope": "user",
            "root": fx.user_root(), "trigger": "manual", "late": false,
            "recordedAt": "2026-10-29T10:00:05.000Z",
            "startedAt": "2026-10-29T10:00:00.000Z", "endedAt": "2026-10-29T10:00:05.000Z",
            "outcome": outcome, "agent": "claude",
            "notes": [{ "ts": "2026-10-29T10:00:03.000Z", "text": "posted the digest",
                        "links": [{ "label": "canvas", "url": "https://example.com/d" }] }],
            "message": { "ts": "2026-10-29T10:00:04.000Z", "text": "all green", "links": [] }
        });
        let rec: fm_model::RunRecord = serde_json::from_value(rec).unwrap();
        fm_state::append_record(&paths, &rec, None).unwrap();
    }
    fx.fm()
        .args(["log", "hello"])
        .assert()
        .success()
        .stdout(contains("failed"))
        .stdout(contains("ok"))
        .stdout(contains("5s"))
        .stdout(contains("note: posted the digest"))
        .stdout(contains("canvas: https://example.com/d"))
        .stdout(contains("message: all green"));
    let out = fx
        .fm()
        .args(["runs", "hello", "-n", "1", "--json"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("20261029-100000-bbbb"));
}

#[test]
fn agent_set_and_default_write_config() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.fm()
        .args(["agent", "default", "claude"])
        .assert()
        .success()
        .stdout(contains("default agent is claude"));
    assert!(fx.config_text().contains("default-agent = \"claude\""));
    fx.fm()
        .args(["agent", "set", "hello", "copilot"])
        .assert()
        .success()
        .stdout(contains("hello runs through copilot"));
    let text = fx.config_text();
    assert!(text.contains(&fx.user_key("hello")), "{text}");
    assert!(text.contains("agent = \"copilot\""), "{text}");
    let all = json_list(&fx);
    assert_eq!(all[0]["agent"], "copilot");
    fx.fm()
        .args(["agent", "set", "hello", "default"])
        .assert()
        .success()
        .stdout(contains("uses the default agent, claude"));
    assert!(!fx.config_text().contains("copilot"));
    fx.fm()
        .args(["agent", "set", "hello", "vim"])
        .assert()
        .code(1);
    fx.fm()
        .args(["agent", "default", "none"])
        .assert()
        .success();
    assert!(!fx.config_text().contains("default-agent"));
}

#[test]
fn agent_list_detects_from_config_and_path() {
    let fx = Fx::new();
    fx.config(&format!(
        "default-agent = \"pi\"\n[agents.pi]\npath = {}\n",
        toml_path(&fm_exe())
    ));
    fx.fm()
        .args(["agent", "list"])
        .assert()
        .success()
        .stdout(contains("* pi"))
        .stdout(contains(env!("CARGO_PKG_VERSION")))
        .stdout(contains("copilot"))
        .stdout(contains("not on PATH"));
}

#[test]
fn also_armed_in_pa_only_when_armed_here_too() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.pa_armed(&[fx.user_key("hello")]);
    assert_eq!(json_list(&fx)[0]["also_armed_in_pa"], false);
    fx.fm()
        .args(["arm", "hello", "--yes"])
        .assert()
        .success()
        .stdout(contains(
            "also armed in pi-automations — it would run twice; disarm it there with `pa disarm hello`",
        ));
    assert_eq!(json_list(&fx)[0]["also_armed_in_pa"], true);
    fx.fm()
        .arg("list")
        .assert()
        .success()
        .stdout(contains("also armed in pa"));
}

#[test]
fn run_stop_and_watch_refusals() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.write(
        &fx.user_root(),
        "scheduled",
        "on:\n  schedule: \"0 9 * * *\"\n",
    );
    fx.fm()
        .args(["run", "hello"])
        .assert()
        .code(2)
        .stderr(contains("hello: not armed — fm arm hello"));
    fx.fm()
        .args(["run", "scheduled"])
        .assert()
        .code(2)
        .stderr(contains("has no manual trigger"));
    fx.fm()
        .args(["stop", "hello"])
        .assert()
        .code(2)
        .stderr(contains("hello is not running"));
    fx.fm()
        .args(["watch", "pause", "hello"])
        .assert()
        .code(2)
        .stderr(contains("has no watch to pause"));
}

/// A stand-in agent CLI that answers like pi: a script replaying `answer` as pi's JSON lines,
/// using only shell built-ins (the tests' `PATH` is an empty folder).
fn fake_pi(fx: &Fx, answer: &str) -> std::path::PathBuf {
    let lines = [
        serde_json::json!({"type": "session", "id": "sess-cli"}),
        serde_json::json!({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": answer}]}}),
        serde_json::json!({"type": "agent_end", "messages": [{"role": "assistant", "content": [{"type": "text", "text": answer}]}]}),
    ];
    let fixture = fx.root.join("pi.jsonl");
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    std::fs::write(&fixture, text.join("\n") + "\n").unwrap();
    if cfg!(windows) {
        let file = fx.root.join("pi.cmd");
        std::fs::write(&file, format!("@type \"{}\"\r\n", fixture.display())).unwrap();
        file
    } else {
        let file = fx.root.join("pi.sh");
        std::fs::write(
            &file,
            format!(
                "#!/bin/sh\nwhile IFS= read -r l || [ -n \"$l\" ]; do printf '%s\\n' \"$l\"; done < '{}'\n",
                fixture.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        file
    }
}

#[test]
fn run_goes_through_the_agent_and_log_shows_one_ok_record() {
    let fx = Fx::new();
    let (script, text) = if cfg!(windows) {
        ("scripts/after.cmd", "@echo kept\r\n")
    } else {
        ("scripts/after", "#!/bin/sh\necho kept\n")
    };
    let file = fx.write(
        &fx.user_root(),
        "hello",
        &format!("on:\n  manual: true\nhooks:\n  post:\n    - script: {script}\n"),
    );
    let script_file = file.parent().unwrap().join(script);
    std::fs::create_dir_all(script_file.parent().unwrap()).unwrap();
    std::fs::write(&script_file, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let agent = fake_pi(&fx, "hello from the fake agent");
    fx.config(&format!(
        "default-agent = \"pi\"\n[agents.pi]\npath = {}\n",
        toml_path(&agent)
    ));
    fx.fm().args(["arm", "hello", "--yes"]).assert().success();
    let out = fx.fm().args(["run", "hello"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    // The answer once (pi's message, not again as the final answer; the record's line after it
    // is the summary), and the last post hook's end, written just before the run ends.
    let alone = stdout
        .lines()
        .filter(|l| l.trim() == "hello from the fake agent")
        .count();
    assert_eq!(alone, 1, "{stdout}");
    assert!(
        stdout.contains(&format!("· post[0] {script} ok")),
        "{stdout}"
    );
    let out = fx.fm().args(["log", "hello", "--json"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 1, "{text}");
    let rec = &records[0];
    assert_eq!(rec["outcome"], "ok");
    assert_eq!(rec["trigger"], "manual");
    assert_eq!(rec["agent"], "pi");
    assert_eq!(rec["output"]["text"], "hello from the fake agent");
    assert_eq!(rec["session"]["id"], "sess-cli");
    assert_eq!(rec["session"]["format"], "pi/json");
    // Nothing left behind: no lock, no live view.
    let paths = fm_model::StatePaths::new(&fx.state);
    let key = fx.user_key("hello");
    assert!(!paths.lock_file(&key).exists());
    assert!(!paths.live_file(&key).exists());
}

#[test]
fn hook_reports_through_fm_on_path() {
    let fx = Fx::new();
    let (script, text) = if cfg!(windows) {
        (
            "scripts/report.cmd",
            "@fm note \"reported from hook\"\r\n@exit /b %errorlevel%\r\n",
        )
    } else {
        (
            "scripts/report",
            "#!/bin/sh\nexec fm note 'reported from hook'\n",
        )
    };
    let file = fx.write(
        &fx.user_root(),
        "hello",
        &format!("on:\n  manual: true\nhooks:\n  pre:\n    - script: {script}\n"),
    );
    let script_file = file.parent().unwrap().join(script);
    std::fs::create_dir_all(script_file.parent().unwrap()).unwrap();
    std::fs::write(&script_file, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let agent = fake_pi(&fx, "done");
    fx.config(&format!(
        "default-agent = \"pi\"\n[agents.pi]\npath = {}\n",
        toml_path(&agent)
    ));
    fx.fm().args(["arm", "hello", "--yes"]).assert().success();
    fx.fm().args(["run", "hello"]).assert().success();

    let out = fx.fm().args(["log", "hello", "--json"]).output().unwrap();
    assert!(out.status.success());
    let records: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["notes"][0]["text"], "reported from hook");
}

#[test]
fn fire_is_quiet_on_stdout_and_exits_with_the_outcome() {
    let fx = Fx::new();
    fx.manual("hello");
    let agent = fake_pi(&fx, "done");
    fx.config(&format!(
        "default-agent = \"pi\"\n[agents.pi]\npath = {}\n",
        toml_path(&agent)
    ));
    let key = fx.user_key("hello");
    // Not armed: skipped, exit 10, one record.
    fx.fm()
        .args(["fire", "--key", &key, "--trigger", "schedule"])
        .args(["--scheduled-for", "2026-10-29T09:00:00.000Z", "--late"])
        .assert()
        .code(10)
        .stdout("");
    fx.fm().args(["arm", "hello", "--yes"]).assert().success();
    fx.fm()
        .args(["fire", "--key", &key, "--trigger", "manual"])
        .assert()
        .code(0)
        .stdout("");
    let out = fx.fm().args(["log", "hello", "--json"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let seen: Vec<(&str, &str)> = records
        .iter()
        .map(|r| {
            (
                r["outcome"].as_str().unwrap(),
                r["trigger"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(seen.len(), 2, "{text}");
    assert!(seen.contains(&("skipped", "schedule")) && seen.contains(&("ok", "manual")));
    let skipped = records.iter().find(|r| r["outcome"] == "skipped").unwrap();
    assert_eq!(skipped["reason"], "not armed");
    assert_eq!(skipped["late"], true);
    assert_eq!(skipped["scheduledFor"], "2026-10-29T09:00:00.000Z");
    // An unknown key is refused with no record.
    fx.fm()
        .args(["fire", "--key", "user:/x:a", "--trigger", "manual"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(contains("a: not found"));
}

#[test]
fn progress_verbs_reach_the_run() {
    let fx = Fx::new();
    let paths = fm_model::StatePaths::new(&fx.state);
    let run = "20261029-100000-abcd";
    fm_state::open_progress(
        &paths,
        &fm_state::OpenArgs {
            run_id: run.into(),
            key: fx.user_key("hello"),
            name: "hello".into(),
            chart: "flowchart TD\n  a --> b\n".into(),
            nodes: fm_run::chart_nodes("flowchart TD\n  a --> b\n"),
            strict: false,
        },
        chrono::Utc::now(),
    )
    .unwrap();
    fx.fm()
        .args(["step", "a", "--status", "done"])
        .env("AUTOMATION_RUN", run)
        .assert()
        .code(0)
        .stdout("a done\n");
    fx.fm()
        .args(["status", "--json"])
        .env("AUTOMATION_RUN", run)
        .env("AUTOMATION_STATE_DIR", &fx.state)
        .assert()
        .code(0)
        .stdout(contains("\"status\":\"done\""));
    fx.fm()
        .args(["step", "a", "--status", "done"])
        .assert()
        .code(2)
        .stderr(contains("fm: not inside a run — AUTOMATION_RUN is not set"));
}

#[test]
fn a_bad_config_is_refused_in_one_line() {
    let fx = Fx::new();
    fx.config("colour = \"blue\"\n");
    let out = fx.fm().arg("list").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err.trim().lines().count(), 1, "{err}");
    assert!(err.contains("config.toml"), "{err}");
}

#[test]
fn tick_by_hand_says_what_it_did_and_who_ticked() {
    let fx = Fx::new();
    fx.manual("hello");
    fx.fm()
        .arg("tick")
        .assert()
        .success()
        .stdout("fired 0 · missed 0 · swept 0\n");
    let state = fm_state::StateStore::new(&fm_model::StatePaths::new(&fx.state)).load();
    assert_eq!(state.last_tick_by, Some(fm_model::TickSource::Cli));
    let out = fx.fm().args(["tick", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["fired"], serde_json::json!([]));
    assert_eq!(v["locked"], false);
}

#[test]
fn the_watcher_job_ticks_quietly_and_is_named_as_the_source() {
    let fx = Fx::new();
    fx.fm()
        .args(["tick", "--by", "job"])
        .assert()
        .success()
        .stdout("");
    let state = fm_state::StateStore::new(&fm_model::StatePaths::new(&fx.state)).load();
    assert_eq!(state.last_tick_by, Some(fm_model::TickSource::Job));
    fx.fm()
        .args(["watcher", "status"])
        .assert()
        .success()
        .stdout(contains("watcher: on").and(contains("by the watcher")));
}

#[test]
fn watcher_status_says_it_is_off_and_how_to_start_it() {
    let fx = Fx::new();
    fx.fm()
        .args(["watcher", "status"])
        .assert()
        .success()
        .stdout(contains("watcher: off").and(contains("fm watcher start")));
    let out = fx
        .fm()
        .args(["watcher", "status", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["state"], "off");
    assert_eq!(v["summary"], "off — fm watcher start");
}

#[test]
fn serve_is_refused_while_the_watcher_is_registered() {
    let fx = Fx::new();
    std::fs::write(fx.state.join("watcher.json"), "{}").unwrap();
    fx.fm().arg("serve").assert().code(2).stderr(
        "fm serve: the watcher is already ticking — one clock at a time: fm watcher stop\n",
    );
}

#[test]
fn a_due_schedule_is_started_as_a_detached_fm_fire_that_leaves_a_record() {
    let fx = Fx::new();
    fx.config("stagger-max-seconds = 0\n");
    fx.write(
        &fx.user_root(),
        "hello",
        "on:\n  schedule:\n    cron: \"* * * * *\"\n    timezone: UTC\n    catch-up: once\n",
    );
    fx.fm().args(["arm", "hello", "--yes"]).assert().success();
    // Armed an hour ago, so the current minute is due whatever the second is now.
    let trust = fx.state.join("trust.json");
    let mut t: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&trust).unwrap()).unwrap();
    let hour_ago = fm_state::time::iso(chrono::Utc::now() - chrono::Duration::hours(1));
    for r in t["records"].as_object_mut().unwrap().values_mut() {
        r["armedAt"] = serde_json::json!(hour_ago);
    }
    std::fs::write(&trust, t.to_string()).unwrap();

    let out = fx.fm().args(["tick", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["fired"][0], fx.user_key("hello"), "{v}");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let last = loop {
        let s = json_list(&fx);
        if !s[0]["last"].is_null() && s[0]["running"].is_null() {
            break s[0]["last"].clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no record from the detached fm fire"
        );
        std::thread::sleep(std::time::Duration::from_millis(250));
    };
    assert_eq!(last["outcome"], "skipped", "{last}");
    assert!(
        last["reason"].as_str().unwrap().contains("no agent CLI"),
        "{last}"
    );
    let runs = std::fs::read_dir(fx.state.join("runs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .unwrap();
    let rec: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(runs)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(rec["trigger"], "schedule", "{rec}");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.state.join("state.json")).unwrap())
            .unwrap();
    let claimed = &state["claimed"][fx.user_key("hello")];
    assert!(claimed.is_string(), "{state}");
    assert_eq!(&rec["scheduledFor"], claimed, "{rec}");
}

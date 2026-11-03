//! Detection and the fake agent, with real processes.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fm_agents::fake::RECORD_STDIN_MARKER;
use fm_agents::{adapter, detect_with, find_in_path};
use fm_model::{CliId, CliModel, CliSettings, CommandPlan, StreamState, TurnRequest};

const FAKE: &str = env!("CARGO_BIN_EXE_fm-fake-agent");

fn fixture_path(cli: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(cli)
        .join(name)
}

/// Write an executable script: a `.cmd` on Windows, an `sh` script elsewhere.
fn script(dir: &Path, name: &str, windows_body: &str, unix_body: &str) -> PathBuf {
    if cfg!(windows) {
        let p = dir.join(format!("{name}.cmd"));
        std::fs::write(&p, format!("@echo off\r\n{windows_body}\r\n")).unwrap();
        p
    } else {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{unix_body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }
}

fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
    std::env::join_paths(dirs).unwrap()
}

const T: Duration = Duration::from_secs(20);

#[test]
fn detect_finds_on_path_and_reads_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let p = script(
        dir.path(),
        "claude",
        "echo 9.8.7 (Claude Code)",
        "echo '9.8.7 (Claude Code)'",
    );
    let install = detect_with(
        CliId::Claude,
        &CliSettings::default(),
        &path_of(&[dir.path()]),
        None,
        T,
    )
    .expect("found");
    assert_eq!(install.path, p);
    assert_eq!(install.version.as_deref(), Some("9.8.7"));
}

#[test]
fn detect_reports_missing_cli_and_bad_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = detect_with(
        CliId::Gemini,
        &CliSettings::default(),
        &path_of(&[dir.path()]),
        None,
        T,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "gemini: gemini is not on PATH — install Gemini CLI, or set agents.gemini.path in config.toml"
    );
    let settings = CliSettings {
        path: Some(dir.path().join("nope")),
        ..Default::default()
    };
    let err = detect_with(CliId::Gemini, &settings, &path_of(&[dir.path()]), None, T).unwrap_err();
    assert!(err.starts_with("agents.gemini.path: "), "{err}");
    assert!(err.contains(" — "));
}

#[test]
fn detect_uses_settings_path_and_the_fake_agent() {
    let settings = CliSettings {
        path: Some(PathBuf::from(FAKE)),
        ..Default::default()
    };
    let install = detect_with(CliId::Codex, &settings, &path_of(&[]), None, T).unwrap();
    assert_eq!(install.path, PathBuf::from(FAKE));
    assert_eq!(install.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn detect_times_out_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    // `.ps1` (run by powershell itself) and `exec` keep the sleeper our own child, so the kill
    // reaches it.
    let p = if cfg!(windows) {
        let p = dir.path().join("pi.ps1");
        std::fs::write(&p, "Start-Sleep -Seconds 30\r\n").unwrap();
        p
    } else {
        script(dir.path(), "pi", "", "exec sleep 30")
    };
    let settings = CliSettings {
        path: Some(p),
        ..Default::default()
    };
    let start = Instant::now();
    let install = detect_with(
        CliId::Pi,
        &settings,
        &path_of(&[]),
        None,
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(install.version, None);
    assert!(start.elapsed() < Duration::from_secs(15));
}

#[cfg(windows)]
#[test]
fn windows_pathext_picks_the_cmd_shim_not_the_sh_script() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    // npm writes an extension-less sh script beside the .cmd and .ps1 shims.
    std::fs::write(first.path().join("opencode"), "#!/bin/sh\n").unwrap();
    std::fs::write(first.path().join("opencode.ps1"), "").unwrap();
    std::fs::write(first.path().join("opencode.cmd"), "").unwrap();
    std::fs::write(second.path().join("opencode.exe"), "").unwrap();
    let path = path_of(&[first.path(), second.path()]);
    assert_eq!(
        find_in_path("opencode", &path, None),
        Some(first.path().join("opencode.cmd"))
    );
    assert_eq!(
        find_in_path("opencode", &path, Some(std::ffi::OsStr::new(".EXE"))),
        Some(first.path().join("opencode.ps1"))
    );
    assert_eq!(
        find_in_path("opencode.exe", &path, None),
        Some(second.path().join("opencode.exe"))
    );
}

#[cfg(windows)]
#[test]
fn windows_ps1_only_runs_through_powershell() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("copilot.ps1");
    std::fs::write(&p, "Write-Output 'GitHub Copilot CLI 7.6.5.'\r\n").unwrap();
    let install = detect_with(
        CliId::Copilot,
        &CliSettings::default(),
        &path_of(&[dir.path()]),
        None,
        T,
    )
    .unwrap();
    assert_eq!(install.path, p);
    assert_eq!(install.version.as_deref(), Some("7.6.5"));
}

#[cfg(unix)]
#[test]
fn unix_skips_files_that_are_not_executable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pi"), "#!/bin/sh\n").unwrap();
    assert_eq!(find_in_path("pi", &path_of(&[dir.path()]), None), None);
    let p = script(dir.path(), "pi", "", "true");
    assert_eq!(find_in_path("pi", &path_of(&[dir.path()]), None), Some(p));
}

/// What `fm-run` does with a plan, minimally: spawn, write the prompt, read lines, finish.
fn run_plan(id: CliId, plan: &CommandPlan, prompt: &str) -> (StreamState, Option<i32>, String) {
    let mut child = Command::new(&plan.program)
        .args(&plan.args)
        .envs(plan.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(prompt.as_bytes()).unwrap();
    drop(stdin);
    let mut stderr = child.stderr.take().unwrap();
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut state = StreamState::default();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        adapter(id).parse_line(&line, &mut state, 0);
    }
    let status = child.wait().unwrap();
    (state, status.code(), err_reader.join().unwrap())
}

fn turn(prompt: &str, extra: Vec<String>, resume: Option<&str>) -> TurnRequest {
    TurnRequest {
        prompt: prompt.into(),
        model: CliModel {
            arg: None,
            thinking: None,
            display: "fake".into(),
        },
        cwd: PathBuf::from("."),
        resume: resume.map(String::from),
        settings: CliSettings {
            path: Some(PathBuf::from(FAKE)),
            extra_args: extra,
            allow_all_tools: false,
            default_model: None,
        },
    }
}

#[test]
fn fake_agent_end_to_end_through_every_adapter() {
    let cases = [
        (CliId::Pi, "pong.jsonl", "pong"),
        (CliId::Copilot, "pong.jsonl", "pong"),
        (CliId::Claude, "pong.jsonl", "pong"),
        (CliId::Codex, "pong.jsonl", "pong"),
        (CliId::Gemini, "pong.jsonl", "pong"),
        (CliId::Opencode, "tool.jsonl", "banana"),
    ];
    let dir = tempfile::tempdir().unwrap();
    // A long prompt: it goes through stdin, far past the Windows command-line limit.
    let prompt = format!("{}\nReply with the single word: pong", "x".repeat(40_000));
    for (id, name, answer) in cases {
        let record = dir.path().join(format!("{id}.txt"));
        let t = turn(
            &prompt,
            vec![
                format!(
                    "--fm-fake-fixture={}",
                    fixture_path(id.as_str(), name).display()
                ),
                format!("--fm-fake-record={}", record.display()),
            ],
            Some("sess-1"),
        );
        let plan = adapter(id).plan_turn(PathBuf::from(FAKE), &t);
        let (state, code, stderr) = run_plan(id, &plan, &prompt);
        let r = adapter(id).finish(&state, code, &stderr);
        assert_eq!(r.answer.as_deref(), Some(answer), "{id}");
        assert_eq!(r.error, None, "{id}");
        assert!(r.session_id.is_some(), "{id}");

        let rec = std::fs::read_to_string(&record).unwrap();
        let (argv, stdin) = rec.split_once(&format!("{RECORD_STDIN_MARKER}\n")).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        let planned: Vec<&str> = plan.args.iter().map(String::as_str).collect();
        assert_eq!(argv, planned, "{id}");
        assert!(
            argv.contains(&"sess-1") || argv.iter().any(|a| a.ends_with("sess-1")),
            "{id}"
        );
        assert_eq!(stdin, prompt, "{id}");
    }
}

#[test]
fn fake_agent_failure_becomes_an_error() {
    let t = turn(
        "hi",
        vec![
            "--fm-fake-exit=3".into(),
            "--fm-fake-stderr=Error: not signed in".into(),
        ],
        None,
    );
    let plan = adapter(CliId::Claude).plan_turn(PathBuf::from(FAKE), &t);
    let (state, code, stderr) = run_plan(CliId::Claude, &plan, "hi");
    let r = adapter(CliId::Claude).finish(&state, code, &stderr);
    assert_eq!(code, Some(3));
    assert_eq!(r.answer, None);
    assert_eq!(
        r.error.as_deref(),
        Some("claude exited with code 3: Error: not signed in")
    );
}

/// A real, signed-in CLI answering "pong" through its adapter. Costs a model call, so it is
/// ignored; run it with `FM_AGENTS_REAL=pi,copilot cargo test -p fm-agents -- --ignored real_`.
#[test]
#[ignore]
fn real_cli_smoke() {
    let wanted = std::env::var("FM_AGENTS_REAL").unwrap_or_default();
    let dir = tempfile::tempdir().unwrap();
    for name in wanted.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let id: CliId = name.parse().unwrap();
        let settings = CliSettings::default();
        let install = fm_agents::detect(id, &settings).unwrap();
        eprintln!("{id}: {} {:?}", install.path.display(), install.version);
        let t = TurnRequest {
            prompt: "Reply with the single word: pong".into(),
            model: adapter(id).resolve_model(None, &settings).unwrap(),
            cwd: dir.path().to_path_buf(),
            resume: None,
            settings,
        };
        let plan = adapter(id).plan_turn(install.path, &t);
        eprintln!("{}", plan.display(&t.prompt));
        let mut child = Command::new(&plan.program)
            .args(&plan.args)
            .current_dir(dir.path())
            // Keep pi's session files out of `~/.pi`.
            .env(
                "PI_CODING_AGENT_SESSION_DIR",
                dir.path().join("pi-sessions"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(t.prompt.as_bytes())
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let err_reader = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });
        let mut state = StreamState::default();
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let events = adapter(id).parse_line(&line.unwrap(), &mut state, 0);
            for e in events {
                eprintln!("  {e:?}");
            }
        }
        let code = child.wait().unwrap().code();
        let r = adapter(id).finish(&state, code, &err_reader.join().unwrap());
        eprintln!("{id}: {r:?}");
        assert_eq!(r.error, None, "{id}");
        assert!(r.answer.unwrap().to_lowercase().contains("pong"), "{id}");
        assert!(r.session_id.is_some(), "{id}");
    }
}

/// An npm-style `.cmd` shim in front of the agent: std spawns it through `cmd.exe` with
/// cmd-safe quoting, and every planned argument arrives intact.
#[cfg(windows)]
#[test]
fn windows_cmd_shim_passes_arguments_intact() {
    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("claude.cmd");
    std::fs::write(&shim, format!("@echo off\r\n\"{FAKE}\" %*\r\n")).unwrap();
    let record = dir.path().join("rec.txt");
    let mut t = turn(
        "hello",
        vec![
            format!("--fm-fake-record={}", record.display()),
            format!(
                "--fm-fake-fixture={}",
                fixture_path("claude", "pong.jsonl").display()
            ),
            "--append-system-prompt".into(),
            "two words & a \"quote\"".into(),
        ],
        Some("5d1c7a3e"),
    );
    t.model = adapter(CliId::Claude)
        .resolve_model(None, &CliSettings::default())
        .unwrap();
    let plan = adapter(CliId::Claude).plan_turn(shim.clone(), &t);
    assert_eq!(plan.program, shim);
    let (state, code, stderr) = run_plan(CliId::Claude, &plan, "hello");
    let r = adapter(CliId::Claude).finish(&state, code, &stderr);
    assert_eq!(r.answer.as_deref(), Some("pong"), "stderr: {stderr}");
    let rec = std::fs::read_to_string(&record).unwrap();
    let (argv, stdin) = rec.split_once(&format!("{RECORD_STDIN_MARKER}\n")).unwrap();
    let argv: Vec<&str> = argv.lines().collect();
    let planned: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    assert_eq!(argv, planned);
    assert_eq!(stdin, "hello");
}

//! Real processes, on the OS the tests run on. Each case picks its command per OS.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// `script` run by the platform shell.
fn shell(cwd: &Path, script: &str) -> ProcessSpec {
    let mut spec = if cfg!(windows) {
        ProcessSpec::new("cmd.exe", cwd).args(["/D", "/C", script])
    } else {
        ProcessSpec::new("/bin/sh", cwd).args(["-c", script])
    };
    spec.timeout = Duration::from_secs(30);
    spec
}

/// The same work, written for each shell.
fn per_os<'a>(windows: &'a str, unix: &'a str) -> &'a str {
    if cfg!(windows) { windows } else { unix }
}

fn run_quiet(spec: &ProcessSpec) -> ProcessOutcome {
    run(spec, |_| {})
}

fn exited(o: ProcessOutcome) -> (i32, Output) {
    match o {
        ProcessOutcome::Exited { exit, output } => (exit, output),
        other => panic!("expected an exit, got {other:?}"),
    }
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use std::process::Command;
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

/// Whether `pid` is gone within `within` (a killed orphan is reaped by init a little later).
fn gone_within(pid: u32, within: Duration) -> bool {
    let until = Instant::now() + within;
    loop {
        if !alive(pid) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn reports_exit_codes_as_facts() {
    let dir = tmp();
    let (exit, _) = exited(run_quiet(&shell(dir.path(), "exit 0")));
    assert_eq!(exit, 0);
    let o = run_quiet(&shell(
        dir.path(),
        per_os(
            "echo went wrong 1>&2& exit 3",
            "echo went wrong >&2; exit 3",
        ),
    ));
    let (exit, out) = exited(o);
    assert_eq!(exit, 3);
    assert_eq!(out.stderr.trim(), "went wrong");
}

#[test]
fn streams_stdout_lines_in_order_as_they_arrive() {
    let dir = tmp();
    let spec = shell(
        dir.path(),
        per_os(
            "echo first& ping -n 3 127.0.0.1 >NUL& echo second& echo third",
            "echo first; sleep 2; echo second; echo third",
        ),
    );
    let mut seen: Vec<(String, Instant)> = Vec::new();
    let o = run(&spec, |l| seen.push((l.to_string(), Instant::now())));
    let ended = Instant::now();
    let (exit, out) = exited(o);
    assert_eq!(exit, 0);
    let lines: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(lines, ["first", "second", "third"]);
    // The first line reached the callback while the child was still sleeping.
    assert!(ended - seen[0].1 >= Duration::from_secs(1));
    assert_eq!(out.stdout.lines().collect::<Vec<_>>(), lines);
    assert!(!out.truncated);
}

#[test]
fn caps_what_it_keeps_and_counts_everything() {
    let dir = tmp();
    let mut spec = shell(
        dir.path(),
        per_os(
            "for /L %i in (1,1,500) do @echo xxxxxxxx",
            "i=0; while [ $i -lt 500 ]; do echo xxxxxxxx; i=$((i+1)); done",
        ),
    );
    spec.max_output_bytes = 100;
    let mut lines = 0;
    let o = run(&spec, |_| lines += 1);
    let (_, out) = exited(o);
    assert_eq!(out.stdout.len(), 100);
    assert!(out.truncated);
    // What the child really wrote: "xxxxxxxx" plus CRLF on Windows, LF elsewhere.
    assert_eq!(out.output_bytes, if cfg!(windows) { 5000 } else { 4500 });
    // The live view still sees every line.
    assert_eq!(lines, 500);
}

#[test]
fn large_output_on_both_pipes_does_not_deadlock() {
    let dir = tmp();
    let mut spec = if cfg!(windows) {
        ProcessSpec::new("powershell.exe", dir.path()).args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Error.Write('y' * 1000000); [Console]::Out.Write('x' * 1000000)",
        ])
    } else {
        shell(
            dir.path(),
            "head -c 1000000 /dev/zero | tr '\\0' y >&2; head -c 1000000 /dev/zero | tr '\\0' x",
        )
    };
    spec.timeout = Duration::from_secs(60);
    spec.max_output_bytes = 10;
    let (exit, out) = exited(run_quiet(&spec));
    assert_eq!(exit, 0);
    assert_eq!(out.output_bytes, 1_000_000);
    assert_eq!(out.stdout, "xxxxxxxxxx");
    assert_eq!(out.stderr.len(), STDERR_MAX_BYTES);
    assert!(out.stderr.bytes().all(|b| b == b'y'));
}

#[test]
fn writes_stdin_and_closes_it() {
    let dir = tmp();
    let mut spec = if cfg!(windows) {
        ProcessSpec::new("findstr.exe", dir.path()).args(["^"])
    } else {
        ProcessSpec::new("cat", dir.path())
    };
    spec.timeout = Duration::from_secs(30);
    spec.stdin = Stdin::Bytes(b"the body said this\n".to_vec());
    let (exit, out) = exited(run_quiet(&spec));
    assert_eq!(exit, 0);
    assert_eq!(out.stdout.trim_end(), "the body said this");
}

#[test]
fn does_not_mind_a_child_that_never_reads_stdin() {
    let dir = tmp();
    let mut spec = shell(dir.path(), "exit 0");
    spec.stdin = Stdin::Bytes(vec![b'x'; 200_000]);
    assert_eq!(exited(run_quiet(&spec)).0, 0);
}

#[test]
fn runs_in_the_directory_it_was_given() {
    let dir = tmp();
    let (_, out) = exited(run_quiet(&shell(dir.path(), per_os("cd", "pwd"))));
    let got = fs::canonicalize(out.stdout.trim()).expect("printed a directory");
    assert_eq!(got, fs::canonicalize(dir.path()).unwrap());
}

#[test]
fn exact_gives_the_child_only_what_it_was_handed() {
    let dir = tmp();
    let mut spec = shell(
        dir.path(),
        per_os(
            "echo [%ONLY_THIS%][%CARGO_PKG_NAME%]",
            "echo \"[$ONLY_THIS][${CARGO_PKG_NAME-%CARGO_PKG_NAME%}]\"",
        ),
    );
    spec.env_mode = EnvMode::Exact(vec![("ONLY_THIS".into(), "yes".into())]);
    let (_, out) = exited(run_quiet(&spec));
    // Cargo sets CARGO_PKG_NAME for this test process; the child must not see it.
    assert_eq!(out.stdout.trim(), "[yes][%CARGO_PKG_NAME%]");
}

#[test]
fn scrubbed_and_inherit_keep_path_and_add_the_overlay() {
    let dir = tmp();
    for mode in [
        EnvMode::Scrubbed {
            overlay: vec![("AUTOMATION_NAME".into(), "hooked".into())],
        },
        EnvMode::Inherit {
            overlay: vec![("AUTOMATION_NAME".into(), "hooked".into())],
        },
    ] {
        let mut spec = shell(
            dir.path(),
            per_os(
                "echo [%AUTOMATION_NAME%]& echo [%PATH%]",
                "echo \"[$AUTOMATION_NAME]\"; echo \"[$PATH]\"",
            ),
        );
        spec.env_mode = mode;
        let (_, out) = exited(run_quiet(&spec));
        let lines: Vec<&str> = out.stdout.lines().collect();
        assert_eq!(lines[0], "[hooked]");
        assert!(lines[1].len() > 2 && !lines[1].contains("%PATH%"));
    }
}

/// Starts a grandchild that sleeps, prints `PID:<grandchild>`, then sleeps itself.
fn tree_spec(dir: &Path) -> ProcessSpec {
    if cfg!(windows) {
        let mut spec = ProcessSpec::new("powershell.exe", dir).args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p = Start-Process -FilePath powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep 100' -NoNewWindow -PassThru; Write-Output ('PID:' + $p.Id); Start-Sleep 100",
        ]);
        spec.timeout = Duration::from_secs(12);
        spec
    } else {
        let mut spec = shell(dir, "sleep 100 & echo \"PID:$!\"; sleep 100");
        spec.timeout = Duration::from_secs(3);
        spec
    }
}

fn grandchild(line: &str, pids: &mut Vec<u32>) {
    if let Some(pid) = line
        .trim()
        .strip_prefix("PID:")
        .and_then(|p| p.parse().ok())
    {
        assert!(alive(pid), "the grandchild runs before the kill");
        pids.push(pid);
    }
}

#[test]
fn a_timeout_kills_the_child_and_its_grandchild() {
    let dir = tmp();
    let spec = tree_spec(dir.path());
    let mut pids = Vec::new();
    let started = Instant::now();
    let o = run(&spec, |l| grandchild(l, &mut pids));
    assert!(
        matches!(o, ProcessOutcome::TimedOut { after, .. } if after == spec.timeout),
        "{o:?}"
    );
    assert!(started.elapsed() < spec.timeout + Duration::from_secs(8));
    assert_eq!(pids.len(), 1, "the grandchild reported its PID: {o:?}");
    assert!(gone_within(pids[0], Duration::from_secs(5)));
}

#[test]
fn a_stop_file_stops_the_tree_within_the_poll_interval() {
    let dir = tmp();
    let stop = dir.path().join("stop");
    let mut spec = tree_spec(dir.path());
    spec.timeout = Duration::from_secs(60);
    spec.stop_file = Some(stop.clone());
    spec.poll = Duration::from_millis(100);
    let mut pids = Vec::new();
    let mut stopped_at = None;
    let o = run(&spec, |l| {
        grandchild(l, &mut pids);
        if !pids.is_empty() && stopped_at.is_none() {
            fs::write(&stop, b"").unwrap();
            stopped_at = Some(Instant::now());
        }
    });
    assert!(matches!(o, ProcessOutcome::Stopped { .. }), "{o:?}");
    // A poll, then the kill (Unix: SIGTERM is enough for sh and sleep), then at most DRAIN while
    // the killed tree's pipes close: under a loaded test run that can take most of it.
    let took = stopped_at.expect("the stop file was written").elapsed();
    assert!(
        took < spec.poll + DRAIN + Duration::from_secs(1),
        "{took:?}"
    );
    assert!(gone_within(pids[0], Duration::from_secs(5)));
}

#[test]
fn a_stop_file_already_there_starts_nothing() {
    let dir = tmp();
    let stop = dir.path().join("stop");
    fs::write(&stop, b"").unwrap();
    let mut spec = ProcessSpec::new(dir.path().join("no-such-binary-anywhere"), dir.path());
    spec.stop_file = Some(stop);
    assert_eq!(
        run_quiet(&spec),
        ProcessOutcome::Stopped {
            output: Output::default()
        }
    );
}

#[test]
fn what_a_finished_child_leaves_running_does_not_hold_the_run() {
    let dir = tmp();
    let spec = shell(
        dir.path(),
        per_os(
            "start /B ping -n 60 127.0.0.1 >NUL& echo done",
            "sleep 60 & echo done",
        ),
    );
    let started = Instant::now();
    let (exit, out) = exited(run_quiet(&spec));
    assert_eq!(exit, 0);
    assert!(out.stdout.contains("done"));
    assert!(started.elapsed() < DRAIN + Duration::from_secs(5));
}

#[test]
fn reports_a_program_that_does_not_exist() {
    let dir = tmp();
    let o = run_quiet(&ProcessSpec::new(
        dir.path().join("no-such-binary-anywhere"),
        dir.path(),
    ));
    match o {
        ProcessOutcome::StartFailed { message } => {
            assert!(message.contains("no-such-binary-anywhere"), "{message}")
        }
        other => panic!("expected a start failure, got {other:?}"),
    }
}

#[test]
fn reports_a_working_directory_that_does_not_exist() {
    let dir = tmp();
    let spec = shell(&dir.path().join("missing"), "exit 0");
    assert!(matches!(
        run_quiet(&spec),
        ProcessOutcome::StartFailed { .. }
    ));
}

#[test]
fn a_hook_result_from_a_real_run() {
    let dir = tmp();
    let o = run_quiet(&shell(
        dir.path(),
        per_os(
            "echo nothing to do 1>&2& exit 3",
            "echo nothing to do >&2; exit 3",
        ),
    ));
    let r = to_hook_result(
        fm_model::Phase::Pre,
        0,
        fm_model::HookKind::Script,
        "s.sh",
        &o,
        Duration::from_millis(5),
    );
    assert_eq!(r.status, fm_model::HookStatus::Skipped);
    assert_eq!(
        r.reason.as_deref(),
        Some("pre[0] s.sh exited 3: nothing to do")
    );
}

#[cfg(windows)]
#[test]
fn windows_children_never_open_a_console_window() {
    assert_eq!(tree::creation_flags(), 0x0800_0000);
    assert_eq!(
        tree::creation_flags() & tree::CREATE_NO_WINDOW,
        tree::CREATE_NO_WINDOW
    );
}

#[cfg(windows)]
#[test]
fn windows_runs_a_cmd_shim_with_spaced_arguments() {
    let dir = tmp();
    let shim = dir.path().join("say it.cmd");
    fs::write(&shim, "@echo [%~1] [%~2]\r\n").unwrap();
    let spec = ProcessSpec::new(&shim, dir.path()).args(["hello world", "plain"]);
    let (exit, out) = exited(run_quiet(&spec));
    assert_eq!(exit, 0);
    assert_eq!(out.stdout.trim(), "[hello world] [plain]");
}

#[cfg(unix)]
#[test]
fn unix_reports_a_signal() {
    let dir = tmp();
    let o = run_quiet(&shell(dir.path(), "kill -9 $$"));
    assert!(
        matches!(&o, ProcessOutcome::Signaled { signal, .. } if signal == "SIGKILL"),
        "{o:?}"
    );
}

#[test]
fn spec_defaults_match_the_hook_defaults() {
    let spec = ProcessSpec::new("p", PathBuf::from("."));
    assert_eq!(spec.timeout, Duration::from_secs(300));
    assert_eq!(spec.max_output_bytes, 65_536);
    assert_eq!(spec.stdin, Stdin::Null);
    assert!(matches!(spec.env_mode, EnvMode::Scrubbed { ref overlay } if overlay.is_empty()));
    assert!(spec.args.is_empty());
    let spec = spec.args(["a", "b"]);
    assert_eq!(spec.args, vec![OsString::from("a"), OsString::from("b")]);
}

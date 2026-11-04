//! Starting processes: with a timeout (killing only the process started here), or detached.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// What a finished process left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn drain(mut r: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        // Charts are a few KB; a runaway process is cut off at 8 MB.
        let _ = r.by_ref().take(8 * 1024 * 1024).read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn wait_for(child: &mut Child, timeout: Duration) -> Result<Option<i32>, String> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code()),
            Ok(None) if start.elapsed() >= timeout => {
                // Only the process this function started.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "did not finish in {}s and was stopped",
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Run `cmd` with `stdin` written to it, capturing its output, killed after `timeout`.
pub fn run_captured(
    mut cmd: Command,
    stdin: Option<String>,
    timeout: Duration,
) -> Result<Finished, String> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(text), Some(mut pipe)) => Some(thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        })),
        _ => None,
    };
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let code = wait_for(&mut child, timeout)?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    let join =
        |h: Option<thread::JoinHandle<String>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(Finished {
        code,
        stdout: join(out),
        stderr: join(err),
    })
}

/// True when `program args…` exits 0 within `timeout`, output ignored.
pub fn succeeds(program: &Path, args: &[&str], timeout: Duration) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    matches!(wait_for(&mut child, timeout), Ok(Some(0)))
}

/// Start `cmd` on its own, outliving this process, its standard streams closed.
pub fn spawn_detached(mut cmd: Command) -> std::io::Result<u32> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn()?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> std::path::PathBuf {
        std::env::current_exe().unwrap()
    }

    #[test]
    fn captures_and_reports_exit() {
        // The test binary itself: `--list` prints the tests and exits 0.
        let mut cmd = Command::new(exe());
        cmd.arg("--list");
        let f = run_captured(cmd, None, Duration::from_secs(30)).unwrap();
        assert_eq!(f.code, Some(0));
        assert!(f.stdout.contains("captures_and_reports_exit"));
    }

    #[test]
    fn missing_program_is_an_error() {
        let cmd = Command::new("fm-no-such-program-here");
        assert!(run_captured(cmd, None, Duration::from_secs(1)).is_err());
        assert!(!succeeds(
            Path::new("fm-no-such-program-here"),
            &[],
            Duration::from_secs(1)
        ));
    }
}

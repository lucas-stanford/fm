//! Running a child process: hook and watch scripts, and agent CLI turns.
//!
//! [`run`] starts one process and waits for it, bounded three ways: a time limit, a cap on the
//! standard output it keeps, and a stop file polled every [`ProcessSpec::poll`] (Windows has no
//! signals, so a stop request is a file, see `adding-support.md`). Standard output is streamed line
//! by line to a callback on the caller's thread as it arrives, for the live view of an agent turn.
//!
//! A timeout or a stop kills the **whole tree** the process started, and only that tree:
//!
//! - **Windows**: the child is assigned to a Job Object with `KILL_ON_JOB_CLOSE` right after it is
//!   spawned; closing the job ends every process in it. If the job cannot be made, the fallback is
//!   `taskkill /T /F /PID <child>`, which walks only our child's tree. Children start with
//!   `CREATE_NO_WINDOW`, so a background run never flashes a console.
//! - **Unix**: the child leads a new process group (`process_group(0)`). The group gets `SIGTERM`,
//!   then `SIGKILL` after [`KILL_GRACE`].
//!
//! When the child exits on its own, what it left running dies too on Windows (the job closes). On
//! Unix, the group is killed only if something in it still holds the output pipes after [`DRAIN`].
//! Nothing is ever killed by name.
//!
//! **Programs.** `program` is already resolved (interpreter resolution lives in `fm-spec`). A
//! `.cmd` or `.bat` file (an npm shim, say) can be given directly: on Windows the standard library
//! runs it through `cmd.exe /c` with batch-safe quoting, and refuses an argument it cannot quote
//! safely, which becomes [`ProcessOutcome::StartFailed`]. A bare `npm` is not found on Windows:
//! resolve it to `npm.cmd` first. Other arguments are quoted for `CommandLineToArgvW`, so spaces
//! and quotes arrive intact.
//!
//! The outcome mirrors pi-automations' `ScriptExecution`: an exit, a start failure, a timeout, a
//! stop, or (Unix only) a signal. It never decides what an exit code means; [`to_hook_result`]
//! does that for hooks.

mod capture;
mod env;
mod hook;
mod tree;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

pub use env::{
    AutomationVars, ScriptContext, automation_env, hook_env, is_credential_name, is_scrubbed_name,
};
pub use hook::{format_duration, to_hook_result};

use capture::{Event, Head, Tail};

/// The most standard error kept: the last 64 KiB, where the error that matters usually is.
pub const STDERR_MAX_BYTES: usize = 64 * 1024;

/// A line of standard output longer than this is passed to the callback in pieces of this size.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// How long a tree gets between `SIGTERM` and `SIGKILL` on Unix.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

/// How long the output pipes may stay open after the child exits, or after it is killed.
pub const DRAIN: Duration = Duration::from_secs(2);

/// The environment a child gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvMode {
    /// This process's environment minus credential-shaped names (`*_API_KEY`, `*_TOKEN`,
    /// `*_SECRET`, `*_PASSWORD`, `*_CREDENTIAL(S)`) and inherited `AUTOMATION_*` names, plus
    /// `overlay`. For hook and watch scripts, with [`automation_env`] as the overlay.
    Scrubbed { overlay: Vec<(String, String)> },
    /// This process's environment untouched, plus `overlay`. For agent CLI turns: the CLI keeps
    /// its own login environment, credentials included.
    Inherit { overlay: Vec<(String, String)> },
    /// Exactly these variables; nothing is inherited. Pair with [`hook_env`].
    Exact(Vec<(String, String)>),
}

/// What the child reads on standard input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    /// Nothing: standard input is the null device. Watch scripts get this.
    Null,
    /// These bytes, then end of file. A child that never reads them is not an error.
    Bytes(Vec<u8>),
}

/// One process to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    /// The resolved executable (an interpreter, or the script itself).
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env_mode: EnvMode,
    pub stdin: Stdin,
    pub timeout: Duration,
    /// Standard output beyond this many bytes is dropped (the head is kept); `truncated` is set.
    pub max_output_bytes: u64,
    /// The process is stopped once this file exists.
    pub stop_file: Option<PathBuf>,
    /// How often the stop file is checked.
    pub poll: Duration,
}

impl ProcessSpec {
    /// A spec with the hook defaults: scrubbed environment with no overlay, no input, 5 minutes,
    /// 64 KiB of output, no stop file, polled every 250 ms.
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env_mode: EnvMode::Scrubbed {
                overlay: Vec::new(),
            },
            stdin: Stdin::Null,
            timeout: Duration::from_secs(5 * 60),
            max_output_bytes: 65_536,
            stop_file: None,
            poll: Duration::from_millis(250),
        }
    }

    /// Replace the arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

/// What a process printed, bounded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// The first `max_output_bytes` of standard output, decoded as UTF-8 (lossily).
    pub stdout: String,
    /// The last [`STDERR_MAX_BYTES`] of standard error, decoded as UTF-8 (lossily).
    pub stderr: String,
    /// Every byte written to standard output, kept or not.
    pub output_bytes: u64,
    /// Standard output was longer than `max_output_bytes`.
    pub truncated: bool,
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    /// It exited by itself with this code.
    Exited { exit: i32, output: Output },
    /// It never ran: missing program, bad working directory, an argument that cannot be quoted.
    StartFailed { message: String },
    /// It overran `timeout`; its tree was killed.
    TimedOut { after: Duration, output: Output },
    /// The stop file appeared; its tree was killed.
    Stopped { output: Output },
    /// Unix only: a signal from outside ended it, e.g. `SIGKILL`.
    Signaled { signal: String, output: Output },
}

impl ProcessOutcome {
    /// What it printed, if it ran.
    pub fn output(&self) -> Option<&Output> {
        match self {
            Self::Exited { output, .. }
            | Self::TimedOut { output, .. }
            | Self::Stopped { output }
            | Self::Signaled { output, .. } => Some(output),
            Self::StartFailed { .. } => None,
        }
    }
}

enum End {
    Timeout,
    Stop,
}

/// Output gathered on the caller's thread from the reader threads' events.
struct Collector {
    head: Head,
    tail: Option<Tail>,
    stdout_done: bool,
}

impl Collector {
    fn take(&mut self, ev: Event, on_line: &mut impl FnMut(&str)) {
        match ev {
            Event::Line(bytes) => {
                self.head.push(&bytes);
                on_line(&capture::line_text(&bytes));
            }
            Event::StdoutDone => self.stdout_done = true,
            Event::StderrDone(t) => self.tail = Some(t),
        }
    }

    fn closed(&self) -> bool {
        self.stdout_done && self.tail.is_some()
    }

    /// Take events until both pipes are closed or `until` passes.
    fn drain(&mut self, rx: &Receiver<Event>, until: Instant, on_line: &mut impl FnMut(&str)) {
        while !self.closed() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match rx.recv_timeout(left) {
                Ok(ev) => self.take(ev, on_line),
                Err(_) => return,
            }
        }
    }
}

/// Run `spec` to its end, passing each line of standard output (without its line ending) to
/// `on_stdout_line` as it arrives. Never returns an error: everything a child can do, including
/// not starting, is a [`ProcessOutcome`].
pub fn run(spec: &ProcessSpec, mut on_stdout_line: impl FnMut(&str)) -> ProcessOutcome {
    let deadline = Instant::now() + spec.timeout;

    // Already asked to stop: start nothing, so nothing is left behind.
    if spec.stop_file.as_ref().is_some_and(|f| f.exists()) {
        return ProcessOutcome::Stopped {
            output: Output::default(),
        };
    }

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(match spec.stdin {
            Stdin::Null => Stdio::null(),
            Stdin::Bytes(_) => Stdio::piped(),
        });
    env::apply(&mut cmd, &spec.env_mode);
    tree::prepare(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ProcessOutcome::StartFailed {
                message: format!("{}: {e}", spec.program.display()),
            };
        }
    };
    let mut tree = tree::Tree::attach(&child);

    if let (Some(mut pipe), Stdin::Bytes(bytes)) = (child.stdin.take(), &spec.stdin) {
        let bytes = bytes.clone();
        // A child that never reads its input closes the pipe; the failed write is not an error.
        thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }

    let (tx, rx) = mpsc::sync_channel::<Event>(256);
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || capture::read_lines(out, &tx));
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || capture::read_tail(err, &tx));
    }
    drop(tx);

    let mut got = Collector {
        head: Head::new(usize::try_from(spec.max_output_bytes).unwrap_or(usize::MAX)),
        tail: None,
        stdout_done: false,
    };
    let mut status: Option<ExitStatus> = None;
    let mut exited_at: Option<Instant> = None;
    let mut end: Option<End> = None;
    let tick = spec
        .poll
        .clamp(Duration::from_millis(1), Duration::from_millis(25));
    let mut next_stop_check = Instant::now();

    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(s)) => {
                    status = Some(s);
                    exited_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(_) => {
                    // The child cannot be observed; treat it as overrunning so its tree goes.
                    end = Some(End::Timeout);
                    break;
                }
            }
        }
        if status.is_some() && got.closed() {
            break;
        }
        if exited_at.is_some_and(|t| t.elapsed() >= DRAIN) {
            break;
        }
        if status.is_none() {
            let now = Instant::now();
            if now >= deadline {
                end = Some(End::Timeout);
                break;
            }
            let check = spec.stop_file.as_ref().filter(|_| now >= next_stop_check);
            if let Some(file) = check {
                next_stop_check = now + spec.poll;
                if file.exists() {
                    end = Some(End::Stop);
                    break;
                }
            }
        }
        match rx.recv_timeout(tick) {
            Ok(ev) => got.take(ev, &mut on_stdout_line),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // Both readers are gone; only the exit is left to wait for.
                got.stdout_done = true;
                got.tail.get_or_insert_with(|| Tail::new(STDERR_MAX_BYTES));
                thread::sleep(tick);
            }
        }
    }

    let drain_until = if end.is_some() {
        tree.kill(&mut child);
        status = child.wait().ok();
        Instant::now() + DRAIN
    } else {
        exited_at.map_or_else(Instant::now, |t| t + DRAIN)
    };
    got.drain(&rx, drain_until, &mut on_stdout_line);
    tree.finish(&mut child, !got.closed());

    let output = Output {
        stdout: got.head.text(),
        stderr: got.tail.map(|t| t.text()).unwrap_or_default(),
        output_bytes: got.head.total(),
        truncated: got.head.truncated(),
    };

    match end {
        Some(End::Timeout) => ProcessOutcome::TimedOut {
            after: spec.timeout,
            output,
        },
        Some(End::Stop) => ProcessOutcome::Stopped { output },
        None => match status {
            Some(s) => from_status(s, output),
            None => ProcessOutcome::Exited { exit: -1, output },
        },
    }
}

fn from_status(status: ExitStatus, output: Output) -> ProcessOutcome {
    if let Some(exit) = status.code() {
        return ProcessOutcome::Exited { exit, output };
    }
    match tree::signal_name(&status) {
        Some(signal) => ProcessOutcome::Signaled { signal, output },
        None => ProcessOutcome::Exited { exit: -1, output },
    }
}

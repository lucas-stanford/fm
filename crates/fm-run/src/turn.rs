//! One agent turn: spawn what the adapter planned, prompt on stdin, stream its standard output
//! through the adapter into the live file and the run's transcript, then let the adapter decide
//! the answer. Spawned with [`crate::process::run`], so a timeout or a stop ends the whole tree
//! (a `.cmd` shim and the node process under it).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fm_model::{AgentCli, PromptVia, StreamState, TurnRequest, TurnResult};

use crate::live::{LiveWriter, now_ms};
use crate::process::{self, EnvMode, ProcessOutcome, ProcessSpec, Stdin};

/// Standard output kept in memory per turn; every line is streamed regardless.
const KEEP_STDOUT_BYTES: u64 = 1024 * 1024;

/// The run's transcript: every line the agent wrote, every turn, in `sessions/<run id>.jsonl`.
/// Best effort, like the live file.
#[derive(Debug)]
pub struct Transcript {
    file: PathBuf,
    out: Option<File>,
}

impl Transcript {
    pub fn open(file: impl Into<PathBuf>) -> Transcript {
        let file = file.into();
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut o = OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let out = o.open(&file).ok();
        Transcript { file, out }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    fn line(&mut self, line: &str) {
        if let Some(out) = self.out.as_mut()
            && writeln!(out, "{line}").is_err()
        {
            self.out = None;
        }
    }
}

/// What to run for one turn.
pub struct Turn<'a> {
    pub adapter: &'a dyn AgentCli,
    /// The CLI as detected.
    pub program: PathBuf,
    pub request: TurnRequest,
    /// Added on top of the plan's own environment: the run's progress identity, `PATH`.
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    pub stop_file: Option<PathBuf>,
    pub poll: Duration,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// The CLI exited (or a signal ended it) and the adapter read what it wrote.
    Finished(TurnResult),
    /// The CLI never ran.
    StartFailed(String),
    TimedOut,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub end: TurnEnd,
    /// The session this turn ran in, if the CLI said.
    pub session_id: Option<String>,
    pub duration: Duration,
}

/// Run one turn to its end.
pub fn run_turn(
    turn: &Turn<'_>,
    live: &mut LiveWriter,
    transcript: &mut Transcript,
) -> TurnOutcome {
    let started = Instant::now();
    let plan = turn.adapter.plan_turn(turn.program.clone(), &turn.request);
    let mut overlay = plan.env.clone();
    overlay.extend(turn.env.iter().cloned());
    let spec = ProcessSpec {
        program: plan.program.clone(),
        args: plan.args.iter().map(Into::into).collect(),
        cwd: turn.request.cwd.clone(),
        env_mode: EnvMode::Inherit { overlay },
        stdin: match plan.prompt_via {
            PromptVia::Stdin => Stdin::Bytes(turn.request.prompt.clone().into_bytes()),
            PromptVia::Arg => Stdin::Null,
        },
        timeout: turn.timeout,
        max_output_bytes: KEEP_STDOUT_BYTES,
        stop_file: turn.stop_file.clone(),
        poll: turn.poll,
    };

    let mut state = StreamState::default();
    let outcome = process::run(&spec, |line| {
        transcript.line(line);
        for ev in turn.adapter.parse_line(line, &mut state, now_ms()) {
            live.write(ev);
        }
    });

    let finish = |exit: Option<i32>, stderr: &str, state: &StreamState| {
        TurnEnd::Finished(turn.adapter.finish(state, exit, stderr))
    };
    let end = match &outcome {
        ProcessOutcome::Exited { exit, output } => finish(Some(*exit), &output.stderr, &state),
        ProcessOutcome::Signaled { output, .. } => finish(None, &output.stderr, &state),
        ProcessOutcome::StartFailed { message } => TurnEnd::StartFailed(message.clone()),
        ProcessOutcome::TimedOut { .. } => TurnEnd::TimedOut,
        ProcessOutcome::Stopped { .. } => TurnEnd::Stopped,
    };
    let session_id = match &end {
        TurnEnd::Finished(r) => r.session_id.clone(),
        _ => None,
    }
    .or(state.session_id);
    TurnOutcome {
        end,
        session_id,
        duration: started.elapsed(),
    }
}

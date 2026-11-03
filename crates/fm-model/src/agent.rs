//! The seam between fm and an agent CLI.
//!
//! An adapter is **pure**: it builds the command for a turn and parses the CLI's output lines. It
//! never starts a process. `fm-run` spawns what an adapter plans, so every adapter is tested with
//! recorded output and no CLI installed.

use std::path::PathBuf;

use crate::{CliId, CliSettings, LiveEvent, ModelSpec};

/// The CLI found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliInstall {
    pub path: PathBuf,
    /// As the CLI reports it, e.g. `1.0.102`.
    pub version: Option<String>,
}

/// Why a model cannot be used through a CLI. Becomes a `skipped` record, never a substitution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRefusal {
    /// One line: `model: <what is wrong> — <what fixes it>`.
    pub message: String,
}

/// The model a CLI will be asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliModel {
    /// What is passed to the CLI, e.g. `claude-sonnet-5` or `anthropic/claude-sonnet-5`.
    pub arg: Option<String>,
    pub thinking: Option<String>,
    /// For records and the UI: `provider/id[:thinking]`, or `<cli> default`.
    pub display: String,
}

/// One turn of an agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRequest {
    pub prompt: String,
    pub model: CliModel,
    pub cwd: PathBuf,
    /// Resume this session (prompt hooks after the first turn). `None` starts a new one.
    pub resume: Option<String>,
    pub settings: CliSettings,
}

/// How the prompt reaches the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptVia {
    /// In the argument list (already included in `args`).
    Arg,
    /// Written to standard input, which is then closed.
    Stdin,
}

/// Exactly what will be run. Also what the consent preview shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Added to the environment. The CLI keeps its own login environment.
    pub env: Vec<(String, String)>,
    pub prompt_via: PromptVia,
}

impl CommandPlan {
    /// One line for the consent preview, with the prompt shown as `<prompt>`.
    pub fn display(&self, prompt: &str) -> String {
        let mut parts = vec![self.program.display().to_string()];
        for a in &self.args {
            let shown = if a == prompt {
                "<prompt>".to_string()
            } else {
                a.clone()
            };
            parts.push(if shown.contains(' ') {
                format!("\"{shown}\"")
            } else {
                shown
            });
        }
        if self.prompt_via == PromptVia::Stdin {
            parts.push("< <prompt>".into());
        }
        parts.join(" ")
    }
}

/// Per-turn parser state an adapter keeps between lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamState {
    pub session_id: Option<String>,
    /// The last assistant text seen, for CLIs that only stream partials.
    pub last_text: String,
    pub final_text: Option<String>,
    pub error: Option<String>,
    /// Adapter-specific scratch.
    pub scratch: String,
}

/// What a finished turn produced, after the process exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnResult {
    pub session_id: Option<String>,
    pub answer: Option<String>,
    pub error: Option<String>,
}

pub trait AgentCli: Send + Sync {
    fn id(&self) -> CliId;

    /// The executable name looked up on PATH when `settings.path` is unset.
    fn program_name(&self) -> &'static str;

    /// Arguments that print the version, e.g. `["--version"]`.
    fn version_args(&self) -> &'static [&'static str] {
        &["--version"]
    }

    /// Map `model:` (or none) to what this CLI is asked for. Refuses, never substitutes.
    fn resolve_model(
        &self,
        spec: Option<&ModelSpec>,
        settings: &CliSettings,
    ) -> Result<CliModel, ModelRefusal>;

    /// Whether a second turn can continue the same session (needed for prompt hooks).
    fn supports_resume(&self) -> bool;

    /// The command for one turn. `program` is the resolved executable.
    fn plan_turn(&self, program: PathBuf, turn: &TurnRequest) -> CommandPlan;

    /// Parse one line of standard output into live events, updating `state`.
    fn parse_line(&self, line: &str, state: &mut StreamState, ts: u64) -> Vec<LiveEvent>;

    /// Called once the process exits. Decides the answer from `state`, stdout tail and exit code.
    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult;

    /// The `session.format` tag for records, e.g. `claude/stream-json`.
    fn transcript_format(&self) -> &'static str;
}

//! Adapters for agent CLIs: pi, Copilot, Claude Code, Codex, Gemini, OpenCode.
//!
//! Each adapter implements [`fm_model::AgentCli`] and is pure: it plans a turn's command and
//! parses the CLI's output lines; `fm-run` spawns. [`detect()`] is the only code here that starts
//! a process (`<cli> --version`). See `docs/AGENTS.md` for the per-CLI table, and [`fake`] for the
//! test stand-in binary `fm-fake-agent`.
//!
//! Every adapter writes the prompt to stdin ([`fm_model::PromptVia::Stdin`]), so prompts never
//! hit the Windows command-line limit (~32K characters) or shell quoting.

use std::path::PathBuf;

use fm_model::{AgentCli, CliId, CommandPlan, PromptVia};

pub mod claude;
pub mod codex;
mod common;
pub mod copilot;
pub mod detect;
pub mod fake;
pub mod gemini;
pub mod opencode;
pub mod pi;

pub use detect::{detect, detect_with, find_in_path, launcher, parse_version};

static PI: pi::Pi = pi::Pi;
static COPILOT: copilot::Copilot = copilot::Copilot;
static CLAUDE: claude::Claude = claude::Claude;
static CODEX: codex::Codex = codex::Codex;
static GEMINI: gemini::Gemini = gemini::Gemini;
static OPENCODE: opencode::Opencode = opencode::Opencode;

/// The adapter for a CLI.
pub fn adapter(id: CliId) -> &'static dyn AgentCli {
    match id {
        CliId::Pi => &PI,
        CliId::Copilot => &COPILOT,
        CliId::Claude => &CLAUDE,
        CliId::Codex => &CODEX,
        CliId::Gemini => &GEMINI,
        CliId::Opencode => &OPENCODE,
    }
}

/// Every adapter, in [`CliId::ALL`] order.
pub fn all() -> [&'static dyn AgentCli; 6] {
    CliId::ALL.map(adapter)
}

/// The version each adapter's flags and output format were checked against.
pub fn checked_version(id: CliId) -> &'static str {
    match id {
        CliId::Pi => pi::CHECKED_VERSION,
        CliId::Copilot => copilot::CHECKED_VERSION,
        CliId::Claude => claude::CHECKED_VERSION,
        CliId::Codex => codex::CHECKED_VERSION,
        CliId::Gemini => gemini::CHECKED_VERSION,
        CliId::Opencode => opencode::CHECKED_VERSION,
    }
}

/// A plan that starts `program` (through [`launcher`] for `.ps1` shims) with `args`, the prompt
/// on stdin.
pub(crate) fn stdin_plan(program: PathBuf, args: Vec<String>) -> CommandPlan {
    let (program, mut full) = launcher(&program);
    full.extend(args);
    CommandPlan {
        program,
        args: full,
        env: Vec::new(),
        prompt_via: PromptVia::Stdin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_complete_and_ordered() {
        let ids: Vec<CliId> = all().iter().map(|a| a.id()).collect();
        assert_eq!(ids, CliId::ALL);
        for id in CliId::ALL {
            assert_eq!(adapter(id).id(), id);
            assert_eq!(adapter(id).program_name(), id.as_str());
            assert!(adapter(id).transcript_format().starts_with(id.as_str()));
            assert_eq!(adapter(id).version_args(), ["--version"]);
            assert!(!checked_version(id).is_empty());
        }
    }
}

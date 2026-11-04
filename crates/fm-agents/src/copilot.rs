//! GitHub Copilot CLI (`copilot`).
//!
//! Checked against the installed `copilot --help`, version [`CHECKED_VERSION`], on 2026-10-29, and
//! real recorded runs (`tests/fixtures/copilot/`). The ignored `real_cli_smoke` test also passed
//! on 1.0.105 (copilot updates itself).
//!
//! ```text
//! copilot --output-format json [--model <id>] [--reasoning-effort <level>] [--allow-all-tools]
//!         [--resume=<session id>] [extra…]  < prompt
//! ```
//!
//! - With stdin piped and no `-p`, copilot takes the prompt from stdin and exits when done
//!   (verified; `--help` mentions "piped stdin" as a prompt source).
//! - `--output-format json` is JSONL: `assistant.message_delta`, `assistant.reasoning_delta`,
//!   `assistant.message`, `tool.execution_start`, and a closing `result` carrying `sessionId`.
//!   The session id only arrives in `result`, at the end.
//! - `--resume=<id>` (with `=`: the value is optional) continues a session non-interactively.
//! - Without `--allow-all-tools`, tools that need approval are denied in non-interactive mode;
//!   reading files in the working directory works.
//!
//! Models: `github-copilot/<id>` (pi's name for this provider) and `copilot/<id>` pass `<id>`
//! through. So do `anthropic/`, `openai/`, `google/` and `xai/` ids, whose models Copilot serves
//! under their own ids (e.g. `anthropic/claude-sonnet-4.5` → `--model claude-sonnet-4.5`).
//! Copilot itself refuses an id it does not have (`Model "x" from --model flag is not
//! available.`), which [`AgentCli::finish`] reports from stderr.

use std::path::PathBuf;

use fm_model::{
    AgentCli, CliId, CliModel, CliSettings, CommandPlan, LiveEvent, ModelRefusal, ModelSpec,
    StreamState, TurnRequest, TurnResult,
};
use serde_json::Value;

use crate::common::{
    ERROR_MAX, MESSAGE_MAX, ModelArg, ModelRules, PARTIAL_MAX, ThinkingRule, finish_turn, head,
    json_object, one_line, resolve_with, session_event, str_at, tail, tool_subject,
};
use crate::stdin_plan;

/// The Copilot CLI version these flags and events were checked against.
pub const CHECKED_VERSION: &str = "1.0.104";

/// `--reasoning-effort` values from `copilot --help`.
pub const THINKING_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Providers whose model ids Copilot serves.
pub const PROVIDERS: &[&str] = &[
    "github-copilot",
    "copilot",
    "anthropic",
    "openai",
    "google",
    "xai",
];

pub struct Copilot;

const RULES: ModelRules = ModelRules {
    cli: CliId::Copilot,
    providers: PROVIDERS,
    provider_fix: "use a github-copilot/<id> model (or anthropic/, openai/, google/, xai/), or another agent",
    thinking: ThinkingRule::Levels(THINKING_LEVELS),
    arg: ModelArg::Id,
};

impl AgentCli for Copilot {
    fn id(&self) -> CliId {
        CliId::Copilot
    }

    fn program_name(&self) -> &'static str {
        "copilot"
    }

    fn resolve_model(
        &self,
        spec: Option<&ModelSpec>,
        settings: &CliSettings,
    ) -> Result<CliModel, ModelRefusal> {
        resolve_with(&RULES, spec, settings)
    }

    fn supports_resume(&self) -> bool {
        true
    }

    fn plan_turn(&self, program: PathBuf, turn: &TurnRequest) -> CommandPlan {
        let mut args: Vec<String> = vec!["--output-format".into(), "json".into()];
        if let Some(m) = &turn.model.arg {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(t) = &turn.model.thinking {
            args.extend(["--reasoning-effort".into(), t.clone()]);
        }
        if turn.settings.allow_all_tools {
            args.push("--allow-all-tools".into());
        }
        if let Some(id) = &turn.resume {
            args.push(format!("--resume={id}"));
        }
        args.extend(turn.settings.extra_args.iter().cloned());
        stdin_plan(program, args)
    }

    fn parse_line(&self, line: &str, state: &mut StreamState, ts: u64) -> Vec<LiveEvent> {
        let Some(obj) = json_object(line) else {
            return Vec::new();
        };
        let v = Value::Object(obj);
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        let kind = str_at(&v, "type").unwrap_or("");
        let mut out = Vec::new();
        match kind {
            "assistant.message_start" => {
                state.last_text.clear();
            }
            "assistant.message_delta" => {
                if let Some(d) = str_at(&data, "deltaContent").filter(|d| !d.is_empty()) {
                    state.last_text.push_str(d);
                    out.push(LiveEvent::Partial {
                        text: tail(&state.last_text, PARTIAL_MAX),
                        ts,
                    });
                }
            }
            "assistant.reasoning_delta" => {
                if let Some(d) = str_at(&data, "deltaContent").filter(|d| !d.is_empty()) {
                    state.scratch.push_str(d);
                    out.push(LiveEvent::Thinking {
                        text: tail(&state.scratch, PARTIAL_MAX),
                        ts,
                    });
                }
            }
            "assistant.reasoning" => {
                state.scratch.clear();
            }
            "assistant.message" => {
                let text = str_at(&data, "content").unwrap_or("");
                if !text.trim().is_empty() {
                    state.last_text = text.to_string();
                    out.push(LiveEvent::Message {
                        text: head(text, MESSAGE_MAX),
                        ts,
                    });
                }
            }
            "tool.execution_start" => {
                let name = str_at(&data, "toolName").unwrap_or("tool").to_string();
                let summary = data.get("arguments").map(tool_subject).unwrap_or_default();
                out.push(LiveEvent::Tool { name, summary, ts });
            }
            "result" => {
                out.extend(session_event(state, str_at(&v, "sessionId"), ts));
                let code = v.get("exitCode").and_then(Value::as_i64).unwrap_or(0);
                if code == 0 && !state.last_text.trim().is_empty() {
                    state.final_text = Some(state.last_text.clone());
                    out.push(LiveEvent::Final {
                        text: state.last_text.clone(),
                        ts,
                    });
                }
            }
            k if k == "error" || k.ends_with(".error") => {
                let msg = str_at(&data, "message")
                    .or_else(|| str_at(&v, "message"))
                    .unwrap_or(k);
                let msg = one_line(msg, ERROR_MAX);
                state.error = Some(msg.clone());
                out.push(LiveEvent::Error { message: msg, ts });
            }
            _ => {}
        }
        out
    }

    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult {
        finish_turn(CliId::Copilot, state, exit, stderr_tail)
    }

    fn transcript_format(&self) -> &'static str {
        "copilot/jsonl"
    }
}

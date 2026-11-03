//! Gemini CLI (`gemini`).
//!
//! Checked on 2026-10-29 against `gemini --help` of [`CHECKED_VERSION`] (installed into a
//! scratch folder, not signed in), the docs
//! <https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/headless.md> and
//! <https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/cli-reference.md>, and the
//! stream event types in `packages/core/src/output/types.ts`. `no-auth.stderr` is real (a piped
//! prompt with no auth exits 41); the stream fixtures are hand-written from those types.
//!
//! ```text
//! gemini --output-format stream-json [-m <id>] [--approval-mode yolo] [--resume <session id>]
//!        [extra…]  < prompt
//! ```
//!
//! - Run with stdin piped (not a TTY), gemini is headless and takes the piped text as the prompt
//!   ("Process piped content"), so the prompt never touches the command line.
//! - `stream-json` events: `init` (`session_id`, `model`), `message` (`role`, `content`,
//!   `delta`), `tool_use` (`tool_name`, `parameters`), `tool_result`, `error` (non-fatal), and a
//!   closing `result` (`status`, `error`).
//! - `--resume <session id>` continues a session ("Resume session by ID").
//! - `allow-all-tools` → `--approval-mode yolo` (`--yolo` is deprecated).
//! - Gemini CLI has no thinking-level flag, so `:level` is refused.

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

/// The Gemini CLI version whose help these flags were checked against (latest release: 0.66.0).
pub const CHECKED_VERSION: &str = "0.67.0-nightly.20261022";

pub struct Gemini;

const RULES: ModelRules = ModelRules {
    cli: CliId::Gemini,
    providers: &["google"],
    provider_fix: "use a google/* model, or another agent",
    thinking: ThinkingRule::Unsupported,
    arg: ModelArg::Id,
};

impl AgentCli for Gemini {
    fn id(&self) -> CliId {
        CliId::Gemini
    }

    fn program_name(&self) -> &'static str {
        "gemini"
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
        let mut args: Vec<String> = vec!["--output-format".into(), "stream-json".into()];
        if let Some(m) = &turn.model.arg {
            args.extend(["-m".into(), m.clone()]);
        }
        if turn.settings.allow_all_tools {
            args.extend(["--approval-mode".into(), "yolo".into()]);
        }
        if let Some(id) = &turn.resume {
            args.extend(["--resume".into(), id.clone()]);
        }
        args.extend(turn.settings.extra_args.iter().cloned());
        stdin_plan(program, args)
    }

    fn parse_line(&self, line: &str, state: &mut StreamState, ts: u64) -> Vec<LiveEvent> {
        let Some(obj) = json_object(line) else {
            return Vec::new();
        };
        let v = Value::Object(obj);
        let mut out = Vec::new();
        match str_at(&v, "type").unwrap_or("") {
            "init" => {
                out.extend(session_event(state, str_at(&v, "session_id"), ts));
            }
            "message" if str_at(&v, "role") == Some("assistant") => {
                let content = str_at(&v, "content").unwrap_or("");
                if v.get("delta").and_then(Value::as_bool).unwrap_or(false) {
                    state.last_text.push_str(content);
                } else {
                    state.last_text = content.to_string();
                }
                if !state.last_text.trim().is_empty() {
                    out.push(LiveEvent::Partial {
                        text: tail(&state.last_text, PARTIAL_MAX),
                        ts,
                    });
                }
            }
            "tool_use" => {
                // Text before a tool call is a finished message; the answer comes after.
                flush_message(state, ts, &mut out);
                out.push(LiveEvent::Tool {
                    name: str_at(&v, "tool_name").unwrap_or("tool").to_string(),
                    summary: v.get("parameters").map(tool_subject).unwrap_or_default(),
                    ts,
                });
            }
            "error" => {
                let msg = str_at(&v, "message").unwrap_or("error");
                out.push(LiveEvent::Error {
                    message: one_line(msg, ERROR_MAX),
                    ts,
                });
            }
            "result" => {
                if str_at(&v, "status") == Some("error") {
                    let msg = v
                        .get("error")
                        .and_then(|e| str_at(e, "message"))
                        .unwrap_or("the run failed");
                    let msg = one_line(msg, ERROR_MAX);
                    state.error = Some(msg.clone());
                    out.push(LiveEvent::Error { message: msg, ts });
                } else if !state.last_text.trim().is_empty() {
                    let text = state.last_text.clone();
                    out.push(LiveEvent::Message {
                        text: head(&text, MESSAGE_MAX),
                        ts,
                    });
                    state.final_text = Some(text.clone());
                    out.push(LiveEvent::Final { text, ts });
                }
            }
            _ => {}
        }
        out
    }

    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult {
        finish_turn(CliId::Gemini, state, exit, stderr_tail)
    }

    fn transcript_format(&self) -> &'static str {
        "gemini/stream-json"
    }
}

fn flush_message(state: &mut StreamState, ts: u64, out: &mut Vec<LiveEvent>) {
    if !state.last_text.trim().is_empty() {
        out.push(LiveEvent::Message {
            text: head(&state.last_text, MESSAGE_MAX),
            ts,
        });
    }
    state.last_text.clear();
}

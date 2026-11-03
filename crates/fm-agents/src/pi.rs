//! pi (`@earendil-works/pi-coding-agent`).
//!
//! Checked against the installed `pi --help`, version [`CHECKED_VERSION`], on 2026-10-29, and a
//! real recorded run (`tests/fixtures/pi/`).
//!
//! ```text
//! pi -p --mode json [--model <provider>/<id>] [--thinking <level>] [--session <id>] [extra…]  < prompt
//! ```
//!
//! - `-p` (`--print`) is non-interactive; with no message argument pi reads the prompt from stdin.
//! - `--mode json` prints pi's agent events as JSON lines: `session` (id), `message_update`
//!   (`text_delta` / `thinking_delta`), `message_end`, `tool_execution_start`, `agent_end`.
//! - `--session <id>` continues that session (looked up in pi's session folder for the cwd).
//! - pi never asks before using a tool, so `allow-all-tools` adds nothing.

use std::path::PathBuf;

use fm_model::{
    AgentCli, CliId, CliModel, CliSettings, CommandPlan, LiveEvent, ModelRefusal, ModelSpec,
    StreamState, TurnRequest, TurnResult,
};
use serde_json::Value;

use crate::common::{
    ERROR_MAX, MESSAGE_MAX, ModelArg, ModelRules, PARTIAL_MAX, ThinkingRule, blocks_text,
    finish_turn, head, json_object, one_line, resolve_with, session_event, str_at, tail,
    tool_subject,
};
use crate::stdin_plan;

/// The pi version these flags and events were checked against.
pub const CHECKED_VERSION: &str = "1.2.0";

/// `--thinking` levels from `pi --help`.
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub struct Pi;

const RULES: ModelRules = ModelRules {
    cli: CliId::Pi,
    providers: &[],
    provider_fix: "",
    thinking: ThinkingRule::Levels(THINKING_LEVELS),
    arg: ModelArg::ProviderSlashId,
};

impl AgentCli for Pi {
    fn id(&self) -> CliId {
        CliId::Pi
    }

    fn program_name(&self) -> &'static str {
        "pi"
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
        let mut args: Vec<String> = vec!["-p".into(), "--mode".into(), "json".into()];
        if let Some(m) = &turn.model.arg {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(t) = &turn.model.thinking {
            args.extend(["--thinking".into(), t.clone()]);
        }
        if let Some(id) = &turn.resume {
            args.extend(["--session".into(), id.clone()]);
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
            "session" => {
                out.extend(session_event(state, str_at(&v, "id"), ts));
            }
            "message_start" => {
                if is_assistant(v.get("message")) {
                    state.last_text.clear();
                    state.scratch.clear();
                }
            }
            "message_update" => {
                let Some(ev) = v.get("assistantMessageEvent") else {
                    return out;
                };
                let delta = str_at(ev, "delta").unwrap_or("");
                match str_at(ev, "type").unwrap_or("") {
                    "text_delta" if !delta.is_empty() => {
                        state.last_text.push_str(delta);
                        out.push(LiveEvent::Partial {
                            text: tail(&state.last_text, PARTIAL_MAX),
                            ts,
                        });
                    }
                    "thinking_delta" if !delta.is_empty() => {
                        state.scratch.push_str(delta);
                        out.push(LiveEvent::Thinking {
                            text: tail(&state.scratch, PARTIAL_MAX),
                            ts,
                        });
                    }
                    _ => {}
                }
            }
            "message_end" => {
                let Some(msg) = v.get("message").filter(|m| is_assistant(Some(m))) else {
                    return out;
                };
                if str_at(msg, "stopReason") == Some("error") {
                    let e = str_at(msg, "errorMessage").unwrap_or("the model call failed");
                    let e = one_line(e, ERROR_MAX);
                    state.error = Some(e.clone());
                    out.push(LiveEvent::Error { message: e, ts });
                }
                let text = blocks_text(msg.get("content"), "text", "text");
                if !text.trim().is_empty() {
                    state.last_text = text.clone();
                    out.push(LiveEvent::Message {
                        text: head(&text, MESSAGE_MAX),
                        ts,
                    });
                }
            }
            "tool_execution_start" => {
                let name = str_at(&v, "toolName").unwrap_or("tool").to_string();
                let summary = v.get("args").map(tool_subject).unwrap_or_default();
                out.push(LiveEvent::Tool { name, summary, ts });
            }
            "agent_end" => {
                let last = v.get("messages").and_then(Value::as_array).and_then(|ms| {
                    ms.iter()
                        .rev()
                        .filter(|m| is_assistant(Some(m)))
                        .map(|m| blocks_text(m.get("content"), "text", "text"))
                        .find(|t| !t.trim().is_empty())
                });
                if let Some(text) = last {
                    state.final_text = Some(text.clone());
                    out.push(LiveEvent::Final { text, ts });
                }
            }
            _ => {}
        }
        out
    }

    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult {
        finish_turn(CliId::Pi, state, exit, stderr_tail)
    }

    fn transcript_format(&self) -> &'static str {
        "pi/json"
    }
}

fn is_assistant(m: Option<&Value>) -> bool {
    m.and_then(|m| str_at(m, "role")) == Some("assistant")
}

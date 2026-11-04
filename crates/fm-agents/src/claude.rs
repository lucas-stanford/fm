//! Claude Code (`claude`).
//!
//! Checked against the installed `claude --help`, version [`CHECKED_VERSION`], on 2026-10-29. That
//! install is not signed in to a usable account, so the recorded fixture is a real *error* run
//! (`tests/fixtures/claude/auth-error.jsonl`); the success fixtures are written by hand from the
//! documented stream-json format (<https://code.claude.com/docs/en/headless>).
//!
//! ```text
//! claude -p --output-format stream-json --verbose [--model <id>] [--effort <level>]
//!        [--dangerously-skip-permissions] [--resume <session id>] [extra…]  < prompt
//! ```
//!
//! - `-p` with no prompt argument reads the prompt from stdin (verified).
//! - `stream-json` needs `--verbose` in print mode. Events: `system`/`init` (`session_id`),
//!   `assistant` (content blocks `text`, `thinking`, `tool_use`), `user` (tool results), and a
//!   closing `result` (`result` text, `is_error`, `session_id`). `stream_event` lines (only with
//!   `--include-partial-messages`) are read for partial text when present.
//! - `--resume <id>` continues a session in print mode.
//! - `allow-all-tools` → `--dangerously-skip-permissions`; without it, tools that need approval
//!   are denied and listed in `permission_denials`.

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

/// The Claude Code version these flags were checked against.
pub const CHECKED_VERSION: &str = "2.1.122";

/// `--effort` levels from `claude --help`.
pub const THINKING_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

pub struct Claude;

const RULES: ModelRules = ModelRules {
    cli: CliId::Claude,
    providers: &["anthropic"],
    provider_fix: "use an anthropic/* model, or another agent",
    thinking: ThinkingRule::Levels(THINKING_LEVELS),
    arg: ModelArg::Id,
};

impl AgentCli for Claude {
    fn id(&self) -> CliId {
        CliId::Claude
    }

    fn program_name(&self) -> &'static str {
        "claude"
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
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
        ];
        if let Some(m) = &turn.model.arg {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(t) = &turn.model.thinking {
            args.extend(["--effort".into(), t.clone()]);
        }
        if turn.settings.allow_all_tools {
            args.push("--dangerously-skip-permissions".into());
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
        out.extend(session_event(state, str_at(&v, "session_id"), ts));
        match str_at(&v, "type").unwrap_or("") {
            "assistant" => {
                let Some(msg) = v.get("message") else {
                    return out;
                };
                let content = msg.get("content");
                let text = blocks_text(content, "text", "text");
                // A synthetic message carrying an API error has a top-level `error`.
                if v.get("error").is_some_and(|e| !e.is_null()) {
                    let e = if text.trim().is_empty() {
                        str_at(&v, "error").unwrap_or("error").to_string()
                    } else {
                        text
                    };
                    let e = one_line(&e, ERROR_MAX);
                    state.error = Some(e.clone());
                    out.push(LiveEvent::Error { message: e, ts });
                    return out;
                }
                let thinking = blocks_text(content, "thinking", "thinking");
                if !thinking.trim().is_empty() {
                    out.push(LiveEvent::Thinking {
                        text: tail(&thinking, PARTIAL_MAX),
                        ts,
                    });
                }
                if !text.trim().is_empty() {
                    state.last_text = text.clone();
                    out.push(LiveEvent::Message {
                        text: head(&text, MESSAGE_MAX),
                        ts,
                    });
                }
                for block in content.and_then(Value::as_array).into_iter().flatten() {
                    if str_at(block, "type") == Some("tool_use") {
                        out.push(LiveEvent::Tool {
                            name: str_at(block, "name").unwrap_or("tool").to_string(),
                            summary: block.get("input").map(tool_subject).unwrap_or_default(),
                            ts,
                        });
                    }
                }
            }
            "stream_event" => {
                let Some(ev) = v.get("event") else {
                    return out;
                };
                match str_at(ev, "type") {
                    Some("message_start") => state.scratch.clear(),
                    Some("content_block_delta") => {
                        let Some(delta) = ev.get("delta") else {
                            return out;
                        };
                        match str_at(delta, "type") {
                            Some("text_delta") => {
                                state.scratch.push_str(str_at(delta, "text").unwrap_or(""));
                                out.push(LiveEvent::Partial {
                                    text: tail(&state.scratch, PARTIAL_MAX),
                                    ts,
                                });
                            }
                            Some("thinking_delta") => {
                                out.push(LiveEvent::Thinking {
                                    text: tail(
                                        str_at(delta, "thinking").unwrap_or(""),
                                        PARTIAL_MAX,
                                    ),
                                    ts,
                                });
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            "result" => {
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                    || str_at(&v, "subtype").is_some_and(|s| s.starts_with("error"));
                let text = str_at(&v, "result").unwrap_or("");
                if is_error {
                    let e = if !text.trim().is_empty() {
                        text.to_string()
                    } else if let Some(errs) = v.get("errors").and_then(Value::as_array) {
                        errs.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    } else {
                        str_at(&v, "subtype").unwrap_or("error").to_string()
                    };
                    let e = one_line(&e, ERROR_MAX);
                    // An API error arrives twice: as a synthetic assistant message, then as the
                    // result. Report it once.
                    if state.error.as_deref() != Some(e.as_str()) {
                        state.error = Some(e.clone());
                        out.push(LiveEvent::Error { message: e, ts });
                    }
                } else if !text.trim().is_empty() {
                    state.final_text = Some(text.to_string());
                    out.push(LiveEvent::Final {
                        text: text.to_string(),
                        ts,
                    });
                }
            }
            _ => {}
        }
        out
    }

    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult {
        finish_turn(CliId::Claude, state, exit, stderr_tail)
    }

    fn transcript_format(&self) -> &'static str {
        "claude/stream-json"
    }
}

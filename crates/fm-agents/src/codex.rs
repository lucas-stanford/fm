//! Codex CLI (`codex`).
//!
//! Checked on 2026-10-29 against `codex exec --help` and `codex exec resume --help` of
//! [`CHECKED_VERSION`] (installed into a scratch folder, not signed in), the docs
//! <https://developers.openai.com/codex/noninteractive> and
//! <https://developers.openai.com/codex/developer-commands>, and `codex-rs/exec/src/cli.rs` and
//! `exec_events.rs` on `main`. `tests/fixtures/codex/auth-error.jsonl` is a real unauthenticated
//! run; the success fixtures are hand-written from the documented JSONL events.
//!
//! ```text
//! codex exec --json --skip-git-repo-check [-m <id>] [-c model_reasoning_effort=<level>]
//!            [--sandbox workspace-write] [extra…] [resume <session id>] -   < prompt
//! ```
//!
//! - `-` reads the prompt from stdin, for a new session and for `exec resume <id> -`.
//! - `--json` prints `thread.started` (`thread_id`, the session id), `item.started` /
//!   `item.completed` (`agent_message`, `reasoning`, `command_execution`, `file_change`,
//!   `mcp_tool_call`, `web_search`, `error`), `turn.completed`, `turn.failed`, `error`.
//! - `--skip-git-repo-check` is always passed: fm picks the working directory itself, and a user
//!   automation's folder is often not a repository.
//! - `codex exec` never asks for approval; its default sandbox is read-only. `allow-all-tools` →
//!   `--sandbox workspace-write` (edits inside the workspace; the documented "allow edits"
//!   setting). Anything broader goes in `extra-args` with `allow-all-tools` off.
//! - `extra-args` go before `resume`/`-`, because options after the `resume` subcommand are
//!   parsed by it.

use std::path::PathBuf;

use fm_model::{
    AgentCli, CliId, CliModel, CliSettings, CommandPlan, LiveEvent, ModelRefusal, ModelSpec,
    StreamState, TurnRequest, TurnResult,
};
use serde_json::Value;

use crate::common::{
    ERROR_MAX, MESSAGE_MAX, ModelArg, ModelRules, PARTIAL_MAX, SUMMARY_MAX, ThinkingRule,
    finish_turn, head, json_object, one_line, resolve_with, session_event, str_at, tail,
    tool_subject,
};
use crate::stdin_plan;

/// The `codex-cli` version whose help these flags were checked against (docs: 0.165.0).
pub const CHECKED_VERSION: &str = "0.164.0-alpha.9";

pub struct Codex;

const RULES: ModelRules = ModelRules {
    cli: CliId::Codex,
    providers: &["openai"],
    provider_fix: "use an openai/* model, or another agent",
    // `model_reasoning_effort` is model-dependent (low … max); Codex validates it.
    thinking: ThinkingRule::AnyWord,
    arg: ModelArg::Id,
};

impl AgentCli for Codex {
    fn id(&self) -> CliId {
        CliId::Codex
    }

    fn program_name(&self) -> &'static str {
        "codex"
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
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
        ];
        if let Some(m) = &turn.model.arg {
            args.extend(["-m".into(), m.clone()]);
        }
        if let Some(t) = &turn.model.thinking {
            args.extend(["-c".into(), format!("model_reasoning_effort={t}")]);
        }
        if turn.settings.allow_all_tools {
            args.extend(["--sandbox".into(), "workspace-write".into()]);
        }
        args.extend(turn.settings.extra_args.iter().cloned());
        if let Some(id) = &turn.resume {
            args.extend(["resume".into(), id.clone()]);
        }
        args.push("-".into());
        stdin_plan(program, args)
    }

    fn parse_line(&self, line: &str, state: &mut StreamState, ts: u64) -> Vec<LiveEvent> {
        let Some(obj) = json_object(line) else {
            return Vec::new();
        };
        let v = Value::Object(obj);
        let mut out = Vec::new();
        match str_at(&v, "type").unwrap_or("") {
            "thread.started" => {
                out.extend(session_event(state, str_at(&v, "thread_id"), ts));
            }
            kind @ ("item.started" | "item.updated" | "item.completed") => {
                let Some(item) = v.get("item") else {
                    return out;
                };
                item_events(kind, item, state, ts, &mut out);
            }
            "turn.completed" => {
                // The turn succeeded; an earlier `error` (a retried stream) did not end it.
                state.error = None;
                if !state.last_text.trim().is_empty() {
                    state.final_text = Some(state.last_text.clone());
                    out.push(LiveEvent::Final {
                        text: state.last_text.clone(),
                        ts,
                    });
                }
            }
            "turn.failed" => {
                let msg = v
                    .get("error")
                    .and_then(|e| str_at(e, "message"))
                    .unwrap_or("the turn failed");
                push_error(state, msg, ts, &mut out);
            }
            "error" => {
                let msg = str_at(&v, "message").unwrap_or("error");
                push_error(state, msg, ts, &mut out);
            }
            _ => {}
        }
        out
    }

    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult {
        finish_turn(CliId::Codex, state, exit, stderr_tail)
    }

    fn transcript_format(&self) -> &'static str {
        "codex/exec-json"
    }
}

fn push_error(state: &mut StreamState, msg: &str, ts: u64, out: &mut Vec<LiveEvent>) {
    let msg = one_line(msg, ERROR_MAX);
    state.error = Some(msg.clone());
    out.push(LiveEvent::Error { message: msg, ts });
}

fn item_events(
    kind: &str,
    item: &Value,
    state: &mut StreamState,
    ts: u64,
    out: &mut Vec<LiveEvent>,
) {
    let id = str_at(item, "id").unwrap_or("");
    let completed = kind == "item.completed";
    // Tool items are reported once, at their first sighting; ids seen are kept in `scratch`.
    let first_sight = |state: &mut StreamState| {
        let key = format!("\n{id}\n");
        if id.is_empty() || state.scratch.contains(&key) {
            return id.is_empty();
        }
        state.scratch.push_str(&key);
        true
    };
    match str_at(item, "type").unwrap_or("") {
        "agent_message" => {
            let text = str_at(item, "text").unwrap_or("");
            if text.trim().is_empty() {
                return;
            }
            state.last_text = text.to_string();
            if completed {
                out.push(LiveEvent::Message {
                    text: head(text, MESSAGE_MAX),
                    ts,
                });
            } else {
                out.push(LiveEvent::Partial {
                    text: tail(text, PARTIAL_MAX),
                    ts,
                });
            }
        }
        "reasoning" => {
            let text = str_at(item, "text").unwrap_or("");
            if !text.trim().is_empty() {
                out.push(LiveEvent::Thinking {
                    text: tail(text, PARTIAL_MAX),
                    ts,
                });
            }
        }
        "command_execution" => {
            if first_sight(state) {
                out.push(LiveEvent::Tool {
                    name: "shell".into(),
                    summary: one_line(str_at(item, "command").unwrap_or(""), SUMMARY_MAX),
                    ts,
                });
            }
        }
        "file_change" => {
            if first_sight(state) {
                let paths: Vec<&str> = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| str_at(c, "path"))
                    .collect();
                out.push(LiveEvent::Tool {
                    name: "file_change".into(),
                    summary: one_line(&paths.join(", "), SUMMARY_MAX),
                    ts,
                });
            }
        }
        "mcp_tool_call" => {
            if first_sight(state) {
                let server = str_at(item, "server").unwrap_or("mcp");
                let tool = str_at(item, "tool").unwrap_or("tool");
                out.push(LiveEvent::Tool {
                    name: format!("{server}.{tool}"),
                    summary: item.get("arguments").map(tool_subject).unwrap_or_default(),
                    ts,
                });
            }
        }
        "web_search" => {
            if first_sight(state) {
                out.push(LiveEvent::Tool {
                    name: "web_search".into(),
                    summary: one_line(str_at(item, "query").unwrap_or(""), SUMMARY_MAX),
                    ts,
                });
            }
        }
        // A non-fatal error item: shown, but the turn goes on.
        "error" if completed => {
            let msg = str_at(item, "message").unwrap_or("error");
            out.push(LiveEvent::Error {
                message: one_line(msg, ERROR_MAX),
                ts,
            });
        }
        _ => {}
    }
}

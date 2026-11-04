//! OpenCode (`opencode`).
//!
//! Checked on 2026-10-29 against `opencode run --help` of [`CHECKED_VERSION`] (installed into a
//! scratch folder), <https://opencode.ai/docs/cli/>, and `packages/opencode/src/cli/cmd/run.ts`
//! at tag `v1.19.5`, with real recorded runs (`tests/fixtures/opencode/`).
//!
//! ```text
//! opencode run --format json [-m <provider>/<id>] [--variant <level>] [--auto]
//!              [--session <session id>] [extra…]  < prompt
//! ```
//!
//! - With no message argument and stdin not a TTY, `run` reads the prompt from stdin.
//! - `--format json` prints one object per line, `{type, timestamp, sessionID, …}`: `step_start`,
//!   `text` (a finished text part), `reasoning` (only with `--thinking`), `tool_use` (a tool part
//!   once it completed or failed), `step_finish` (`part.reason`), `error`.
//! - Tool calls are reported when they finish, not when they start.
//! - `--session <id>` continues a session.
//! - `allow-all-tools` → `--auto` ("auto-approve permissions that are not explicitly denied");
//!   without it `run` rejects every permission request.
//! - Models are `provider/id` as given; `:level` → `--variant <level>` (provider-specific
//!   reasoning effort, validated by OpenCode).

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

/// The OpenCode version these flags and events were checked against.
pub const CHECKED_VERSION: &str = "1.19.4";

pub struct Opencode;

const RULES: ModelRules = ModelRules {
    cli: CliId::Opencode,
    providers: &[],
    provider_fix: "",
    thinking: ThinkingRule::AnyWord,
    arg: ModelArg::ProviderSlashId,
};

impl AgentCli for Opencode {
    fn id(&self) -> CliId {
        CliId::Opencode
    }

    fn program_name(&self) -> &'static str {
        "opencode"
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
        let mut args: Vec<String> = vec!["run".into(), "--format".into(), "json".into()];
        if let Some(m) = &turn.model.arg {
            args.extend(["-m".into(), m.clone()]);
        }
        if let Some(t) = &turn.model.thinking {
            args.extend(["--variant".into(), t.clone()]);
        }
        if turn.settings.allow_all_tools {
            args.push("--auto".into());
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
        out.extend(session_event(state, str_at(&v, "sessionID"), ts));
        let part = v.get("part").cloned().unwrap_or(Value::Null);
        match str_at(&v, "type").unwrap_or("") {
            "text" => {
                let text = str_at(&part, "text").unwrap_or("");
                if !text.trim().is_empty() {
                    state.last_text = text.to_string();
                    out.push(LiveEvent::Message {
                        text: head(text, MESSAGE_MAX),
                        ts,
                    });
                }
            }
            "reasoning" => {
                let text = str_at(&part, "text").unwrap_or("");
                if !text.trim().is_empty() {
                    out.push(LiveEvent::Thinking {
                        text: tail(text, PARTIAL_MAX),
                        ts,
                    });
                }
            }
            "tool_use" => {
                let name = str_at(&part, "tool").unwrap_or("tool").to_string();
                let summary = part
                    .get("state")
                    .and_then(|s| s.get("input"))
                    .map(tool_subject)
                    .unwrap_or_default();
                out.push(LiveEvent::Tool { name, summary, ts });
            }
            "step_finish" => {
                // `tool-calls` means another step follows; anything else ends the turn.
                let reason = str_at(&part, "reason").unwrap_or("");
                if reason != "tool-calls" && !state.last_text.trim().is_empty() {
                    state.final_text = Some(state.last_text.clone());
                    out.push(LiveEvent::Final {
                        text: state.last_text.clone(),
                        ts,
                    });
                }
            }
            "error" => {
                let msg = error_message(v.get("error"));
                let msg = one_line(&msg, ERROR_MAX);
                state.error = Some(msg.clone());
                out.push(LiveEvent::Error { message: msg, ts });
            }
            _ => {}
        }
        out
    }

    fn finish(&self, state: &StreamState, exit: Option<i32>, stderr_tail: &str) -> TurnResult {
        finish_turn(CliId::Opencode, state, exit, stderr_tail)
    }

    fn transcript_format(&self) -> &'static str {
        "opencode/json"
    }
}

/// `{name, data: {message}}` (a session error) or `{type, message}` (a run error).
fn error_message(e: Option<&Value>) -> String {
    let Some(e) = e else {
        return "error".into();
    };
    if let Some(s) = e.as_str() {
        return s.to_string();
    }
    e.get("data")
        .and_then(|d| str_at(d, "message"))
        .or_else(|| str_at(e, "message"))
        .or_else(|| str_at(e, "name"))
        .unwrap_or("error")
        .to_string()
}

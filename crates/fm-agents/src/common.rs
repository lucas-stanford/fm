//! Helpers every adapter shares: bounded text, tool summaries, model rules, `finish`.

use fm_model::{
    CliId, CliModel, CliSettings, LiveEvent, ModelRefusal, ModelSpec, StreamState, TurnResult,
};
use serde_json::Value;

/// Longest `Partial` / `Thinking` text, keeping the tail (the newest words).
pub(crate) const PARTIAL_MAX: usize = 2000;
/// Longest `Message` text.
pub(crate) const MESSAGE_MAX: usize = 8000;
/// Longest tool summary.
pub(crate) const SUMMARY_MAX: usize = 120;
/// Longest error text taken from a stream or from stderr.
pub(crate) const ERROR_MAX: usize = 500;

/// Parse a line as a JSON object. Anything else (blank, torn, garbage, arrays) is `None`.
pub(crate) fn json_object(line: &str) -> Option<serde_json::Map<String, Value>> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

pub(crate) fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The first `max` characters, with `…` when cut.
pub(crate) fn head(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// The last `max` characters, with `…` when cut.
pub(crate) fn tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let skip = count - max;
    let start = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    format!("…{}", &s[start..])
}

/// Whitespace (newlines included) collapsed to single spaces, bounded.
pub(crate) fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    head(&flat, max)
}

/// The argument that says what a tool call acted on, as one bounded line. Mirrors pa's
/// `toolSubject`.
pub(crate) fn tool_subject(args: &Value) -> String {
    let Some(obj) = args.as_object() else {
        return match args {
            Value::String(s) => one_line(s, SUMMARY_MAX),
            _ => String::new(),
        };
    };
    const KEYS: [&str; 12] = [
        "path",
        "file_path",
        "filePath",
        "file",
        "command",
        "cmd",
        "url",
        "query",
        "pattern",
        "prompt",
        "description",
        "name",
    ];
    for key in KEYS {
        let found = obj
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        if let Some(s) = found {
            return one_line(s, SUMMARY_MAX);
        }
    }
    obj.values()
        .filter_map(Value::as_str)
        .find(|s| !s.trim().is_empty() && s.chars().count() <= SUMMARY_MAX)
        .map(|s| one_line(s, SUMMARY_MAX))
        .unwrap_or_default()
}

/// Concatenated `text` of the blocks of `content` whose `type` is `kind`.
pub(crate) fn blocks_text(content: Option<&Value>, kind: &str, field: &str) -> String {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|b| str_at(b, "type") == Some(kind))
        .filter_map(|b| str_at(b, field))
        .collect::<Vec<_>>()
        .join("")
}

/// Remember a session id; `true` when it is new (so the caller emits `LiveEvent::Session`).
pub(crate) fn note_session(state: &mut StreamState, id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || state.session_id.as_deref() == Some(id) {
        return false;
    }
    state.session_id = Some(id.to_string());
    true
}

/// A `Session` event for `id` when it is new.
pub(crate) fn session_event(
    state: &mut StreamState,
    id: Option<&str>,
    ts: u64,
) -> Option<LiveEvent> {
    let id = id?;
    note_session(state, id).then(|| LiveEvent::Session {
        id: id.trim().to_string(),
        ts,
    })
}

/// The shared end of a turn: the answer is the final text, else the last text seen. A non-zero
/// (or missing) exit with no answer is an error carrying a bounded excerpt of stderr.
pub(crate) fn finish_turn(
    cli: CliId,
    state: &StreamState,
    exit: Option<i32>,
    stderr_tail: &str,
) -> TurnResult {
    let answer = state
        .final_text
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| Some(state.last_text.as_str()).filter(|t| !t.trim().is_empty()))
        .map(str::to_string);
    let error = if let Some(e) = &state.error {
        Some(e.clone())
    } else if answer.is_none() && exit != Some(0) {
        let how = match exit {
            Some(code) => format!("{cli} exited with code {code}"),
            None => format!("{cli} was stopped before it answered"),
        };
        let excerpt = stderr_excerpt(stderr_tail);
        Some(if excerpt.is_empty() {
            format!("{how} and wrote nothing to stderr")
        } else {
            format!("{how}: {excerpt}")
        })
    } else {
        None
    };
    TurnResult {
        session_id: state.session_id.clone(),
        answer,
        error,
    }
}

/// The end of stderr as one bounded line: its last non-empty lines joined with ` | `.
pub(crate) fn stderr_excerpt(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let start = lines.len().saturating_sub(5);
    let joined = lines[start..].join(" | ");
    tail(&one_line(&joined, usize::MAX), ERROR_MAX)
}

/// One line, `model: <what is wrong> — <what fixes it>`.
pub(crate) fn refuse(what: &str, fix: &str) -> ModelRefusal {
    ModelRefusal {
        message: one_line(&format!("model: {what} — {fix}"), 400),
    }
}

/// How an adapter accepts thinking levels.
pub(crate) enum ThinkingRule {
    /// The CLI has no way to set one: a `:level` suffix is refused.
    Unsupported,
    /// Exactly these levels.
    Levels(&'static [&'static str]),
    /// Any simple word (`[A-Za-z0-9_-]+`); the CLI validates it against the model.
    AnyWord,
}

/// What goes in the CLI's model argument.
pub(crate) enum ModelArg {
    /// Just the id: `claude-sonnet-5`.
    Id,
    /// `provider/id`, as given.
    ProviderSlashId,
}

pub(crate) struct ModelRules {
    pub cli: CliId,
    /// Providers this CLI serves. Empty: any provider.
    pub providers: &'static [&'static str],
    /// The fix shown when the provider is refused, e.g. `use an anthropic/* model, or another agent`.
    pub provider_fix: &'static str,
    pub thinking: ThinkingRule,
    pub arg: ModelArg,
}

/// Parse `provider/id[:thinking]`. The thinking suffix is after the last `:` of the id.
pub(crate) fn parse_model_text(text: &str) -> Option<ModelSpec> {
    let (provider, rest) = text.trim().split_once('/')?;
    let (id, thinking) = match rest.rsplit_once(':') {
        Some((id, t)) if !t.is_empty() && !t.contains('/') => (id, Some(t.to_string())),
        _ => (rest, None),
    };
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some(ModelSpec {
        provider: provider.to_string(),
        id: id.to_string(),
        thinking,
    })
}

fn simple_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Map `model:` (or the configured default, or the CLI default) through `rules`. Never
/// substitutes: a model the CLI cannot serve is refused.
pub(crate) fn resolve_with(
    rules: &ModelRules,
    spec: Option<&ModelSpec>,
    settings: &CliSettings,
) -> Result<CliModel, ModelRefusal> {
    let parsed;
    let spec = match spec {
        Some(s) => s,
        None => match settings.default_model.as_deref().map(str::trim) {
            None | Some("") => {
                return Ok(CliModel {
                    arg: None,
                    thinking: None,
                    display: format!("{} default", rules.cli),
                });
            }
            Some(text) => match parse_model_text(text) {
                Some(s) => {
                    parsed = s;
                    &parsed
                }
                // A name without `/` is the CLI's own model name (an alias such as `sonnet`).
                None => {
                    return Ok(CliModel {
                        arg: Some(text.to_string()),
                        thinking: None,
                        display: text.to_string(),
                    });
                }
            },
        },
    };

    let shown = spec.display();
    if spec.provider.trim().is_empty() || spec.id.trim().is_empty() {
        return Err(refuse(
            &format!("\"{shown}\" is not provider/id"),
            "write it as <provider>/<model id>, e.g. anthropic/claude-sonnet-5",
        ));
    }
    if !rules.providers.is_empty() && !rules.providers.contains(&spec.provider.as_str()) {
        return Err(refuse(
            &format!("{shown} cannot run through {}", rules.cli.title()),
            rules.provider_fix,
        ));
    }
    let thinking = match (&spec.thinking, &rules.thinking) {
        (None, _) => None,
        (Some(t), ThinkingRule::Unsupported) => {
            return Err(refuse(
                &format!(
                    "{shown}: {} has no thinking-level option",
                    rules.cli.title()
                ),
                &format!("drop \":{t}\", or use another agent"),
            ));
        }
        (Some(t), ThinkingRule::Levels(levels)) => {
            if !levels.contains(&t.as_str()) {
                return Err(refuse(
                    &format!(
                        "{shown}: {} has no thinking level \"{t}\"",
                        rules.cli.title()
                    ),
                    &format!("use one of {}", levels.join(", ")),
                ));
            }
            Some(t.clone())
        }
        (Some(t), ThinkingRule::AnyWord) => {
            if !simple_word(t) {
                return Err(refuse(
                    &format!("{shown}: thinking level \"{t}\" is not a single word"),
                    "use a level such as low, medium or high",
                ));
            }
            Some(t.clone())
        }
    };
    let arg = match rules.arg {
        ModelArg::Id => spec.id.clone(),
        ModelArg::ProviderSlashId => format!("{}/{}", spec.provider, spec.id),
    };
    Ok(CliModel {
        arg: Some(arg),
        thinking,
        display: shown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_tail_respect_char_boundaries() {
        assert_eq!(head("héllo", 2), "hé…");
        assert_eq!(head("hi", 5), "hi");
        assert_eq!(tail("héllo", 3), "…llo");
        assert_eq!(tail("ab", 3), "ab");
        assert_eq!(one_line("a\n  b\tc", 10), "a b c");
    }

    #[test]
    fn tool_subject_prefers_known_keys() {
        let v = serde_json::json!({"view_range": [1, 2], "path": "src/a.rs"});
        assert_eq!(tool_subject(&v), "src/a.rs");
        let v = serde_json::json!({"x": "short\nvalue"});
        assert_eq!(tool_subject(&v), "short value");
        assert_eq!(tool_subject(&serde_json::json!(42)), "");
        let long = "x".repeat(500);
        assert_eq!(
            tool_subject(&serde_json::json!({"command": long}))
                .chars()
                .count(),
            SUMMARY_MAX + 1
        );
    }

    #[test]
    fn parse_model_text_splits_thinking() {
        let s = parse_model_text("anthropic/claude-sonnet-5:high").unwrap();
        assert_eq!(
            (s.provider.as_str(), s.id.as_str()),
            ("anthropic", "claude-sonnet-5")
        );
        assert_eq!(s.thinking.as_deref(), Some("high"));
        let s = parse_model_text("openrouter/meta/llama-3").unwrap();
        assert_eq!(s.id, "meta/llama-3");
        assert_eq!(s.thinking, None);
        assert!(parse_model_text("sonnet").is_none());
        assert!(parse_model_text("/x").is_none());
    }

    #[test]
    fn json_object_rejects_garbage() {
        assert!(json_object("").is_none());
        assert!(json_object("not json").is_none());
        assert!(json_object("{\"a\":").is_none());
        assert!(json_object("[1,2]").is_none());
        assert!(json_object(" {\"a\":1} ").is_some());
    }

    #[test]
    fn finish_reports_stderr_when_no_answer() {
        let state = StreamState::default();
        let r = finish_turn(CliId::Pi, &state, Some(2), "line one\n\nboom: bad thing\n");
        assert_eq!(
            r.error.as_deref(),
            Some("pi exited with code 2: line one | boom: bad thing")
        );
        let r = finish_turn(CliId::Pi, &state, Some(1), "");
        assert_eq!(
            r.error.as_deref(),
            Some("pi exited with code 1 and wrote nothing to stderr")
        );
        let r = finish_turn(CliId::Pi, &state, None, "");
        assert!(r.error.unwrap().starts_with("pi was stopped"));
        let r = finish_turn(CliId::Pi, &state, Some(0), "");
        assert_eq!((r.answer, r.error), (None, None));
        let long = "e".repeat(5000);
        let r = finish_turn(CliId::Pi, &state, Some(1), &long);
        assert!(r.error.unwrap().chars().count() < ERROR_MAX + 40);
    }

    #[test]
    fn finish_prefers_final_then_last_text() {
        let mut state = StreamState {
            last_text: "draft".into(),
            ..Default::default()
        };
        assert_eq!(
            finish_turn(CliId::Pi, &state, Some(1), "x")
                .answer
                .as_deref(),
            Some("draft")
        );
        state.final_text = Some("done".into());
        let r = finish_turn(CliId::Pi, &state, Some(0), "");
        assert_eq!(r.answer.as_deref(), Some("done"));
        assert_eq!(r.error, None);
    }
}

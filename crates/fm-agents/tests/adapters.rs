//! Adapter tests against recorded (and, where no CLI was available, hand-written) output.
//! See `tests/fixtures/README.md` for which is which.

use std::path::{Path, PathBuf};

use fm_agents::adapter;
use fm_model::{
    CliId, CliModel, CliSettings, CommandPlan, LiveEvent, ModelSpec, PromptVia, StreamState,
    TurnRequest, TurnResult,
};

const PROMPT: &str = "Reply with the single word: pong";

fn fixture_path(cli: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(cli)
        .join(name)
}

fn fixture(cli: &str, name: &str) -> String {
    std::fs::read_to_string(fixture_path(cli, name)).expect("fixture")
}

fn replay(id: CliId, text: &str) -> (Vec<LiveEvent>, StreamState) {
    let a = adapter(id);
    let mut state = StreamState::default();
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        events.extend(a.parse_line(line, &mut state, i as u64));
    }
    (events, state)
}

fn run_fixture(id: CliId, name: &str, exit: i32, stderr: &str) -> (Vec<LiveEvent>, TurnResult) {
    let (events, state) = replay(id, &fixture(id.as_str(), name));
    let result = adapter(id).finish(&state, Some(exit), stderr);
    (events, result)
}

fn sessions(events: &[LiveEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Session { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

fn finals(events: &[LiveEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Final { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn messages(events: &[LiveEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Message { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn tools(events: &[LiveEvent]) -> Vec<(&str, &str)> {
    events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Tool { name, summary, .. } => Some((name.as_str(), summary.as_str())),
            _ => None,
        })
        .collect()
}

fn errors(events: &[LiveEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Error { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect()
}

fn has_partial_or_thinking(events: &[LiveEvent]) -> (bool, bool) {
    (
        events
            .iter()
            .any(|e| matches!(e, LiveEvent::Partial { .. })),
        events
            .iter()
            .any(|e| matches!(e, LiveEvent::Thinking { .. })),
    )
}

fn spec(text: &str) -> ModelSpec {
    let (provider, rest) = text.split_once('/').unwrap();
    let (id, thinking) = match rest.rsplit_once(':') {
        Some((id, t)) => (id, Some(t.to_string())),
        None => (rest, None),
    };
    ModelSpec {
        provider: provider.into(),
        id: id.into(),
        thinking,
    }
}

fn model(id: CliId, text: Option<&str>) -> CliModel {
    let s = text.map(spec);
    adapter(id)
        .resolve_model(s.as_ref(), &CliSettings::default())
        .expect("model resolves")
}

fn refusal(id: CliId, text: &str) -> String {
    adapter(id)
        .resolve_model(Some(&spec(text)), &CliSettings::default())
        .expect_err("model refused")
        .message
}

fn exe(name: &str) -> PathBuf {
    Path::new("bin").join(name)
}

fn plan(id: CliId, model: CliModel, resume: Option<&str>, settings: CliSettings) -> CommandPlan {
    let turn = TurnRequest {
        prompt: PROMPT.into(),
        model,
        cwd: PathBuf::from("."),
        resume: resume.map(String::from),
        settings,
    };
    adapter(id).plan_turn(exe(id.as_str()), &turn)
}

fn full_settings() -> CliSettings {
    CliSettings {
        path: None,
        extra_args: vec!["--extra".into(), "two words".into()],
        allow_all_tools: true,
        default_model: None,
    }
}

fn args(plan: &CommandPlan) -> Vec<&str> {
    plan.args.iter().map(String::as_str).collect()
}

fn assert_stdin_plan(id: CliId, plan: &CommandPlan) {
    assert_eq!(plan.program, exe(id.as_str()));
    assert_eq!(plan.prompt_via, PromptVia::Stdin);
    assert!(plan.env.is_empty());
    assert!(!plan.args.iter().any(|a| a.contains(PROMPT)));
    assert!(plan.display(PROMPT).ends_with(" < <prompt>"));
}

// ------------------------------------------------------------------------------------------ pi

#[test]
fn pi_plan() {
    let id = CliId::Pi;
    let p = plan(id, model(id, None), None, CliSettings::default());
    assert_stdin_plan(id, &p);
    assert_eq!(args(&p), ["-p", "--mode", "json"]);

    let p = plan(
        id,
        model(id, Some("github-copilot/claude-opus-5.5:high")),
        Some("01a1886a"),
        full_settings(),
    );
    assert_eq!(
        args(&p),
        [
            "-p",
            "--mode",
            "json",
            "--model",
            "github-copilot/claude-opus-5.5",
            "--thinking",
            "high",
            "--session",
            "01a1886a",
            "--extra",
            "two words"
        ]
    );
    let shown = p.display(PROMPT);
    assert!(shown.ends_with(
        "-p --mode json --model github-copilot/claude-opus-5.5 --thinking high --session 01a1886a --extra \"two words\" < <prompt>"
    ));
}

#[test]
fn pi_pong() {
    let (events, r) = run_fixture(CliId::Pi, "pong.jsonl", 0, "");
    assert_eq!(sessions(&events), ["01a1886a-dfba-720c-864e-d4acfb04be9c"]);
    assert_eq!(messages(&events), ["pong"]);
    assert_eq!(finals(&events), ["pong"]);
    assert!(has_partial_or_thinking(&events).0);
    assert_eq!(r.answer.as_deref(), Some("pong"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("01a1886a-dfba-720c-864e-d4acfb04be9c")
    );
    assert_eq!(r.error, None);
}

#[test]
fn pi_tool_and_resume() {
    let (events, r) = run_fixture(CliId::Pi, "tool.jsonl", 0, "");
    assert_eq!(tools(&events), [("read", "hello.txt")]);
    assert_eq!(finals(&events), ["banana"]);
    assert_eq!(r.answer.as_deref(), Some("banana"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("01a1886b-3284-724d-86d5-6d1a973fc383")
    );

    let (events, r) = run_fixture(CliId::Pi, "resume.jsonl", 0, "");
    assert_eq!(sessions(&events), ["01a1886a-dfba-720c-864e-d4acfb04be9c"]);
    assert_eq!(r.answer.as_deref(), Some("pong"));
}

#[test]
fn pi_bad_model_reports_stderr() {
    let stderr = fixture("pi", "bad-model.stderr");
    let r = adapter(CliId::Pi).finish(&StreamState::default(), Some(1), &stderr);
    assert_eq!(r.answer, None);
    assert_eq!(
        r.error.as_deref(),
        Some(
            "pi exited with code 1: Error: Model \"nope/nomodel\" not found. Use --list-models to see available models."
        )
    );
}

#[test]
fn pi_models() {
    let id = CliId::Pi;
    let m = model(id, Some("openai/gpt-6.1-sol"));
    assert_eq!(m.arg.as_deref(), Some("openai/gpt-6.1-sol"));
    assert_eq!(m.thinking, None);
    assert_eq!(m.display, "openai/gpt-6.1-sol");
    assert_eq!(model(id, None).display, "pi default");
    assert_eq!(model(id, None).arg, None);
    let msg = refusal(id, "anthropic/claude-sonnet-5:turbo");
    assert!(msg.starts_with("model: anthropic/claude-sonnet-5:turbo: pi has no thinking level \"turbo\" — use one of off, minimal"), "{msg}");
}

// ------------------------------------------------------------------------------------- copilot

#[test]
fn copilot_plan() {
    let id = CliId::Copilot;
    let p = plan(id, model(id, None), None, CliSettings::default());
    assert_stdin_plan(id, &p);
    assert_eq!(args(&p), ["--output-format", "json"]);
    assert!(!p.args.iter().any(|a| a == "--allow-all-tools"));

    let p = plan(
        id,
        model(id, Some("github-copilot/gpt-6.1-sol:xhigh")),
        Some("27f8d8a7-0ee0-42f7-a391-a088314703c2"),
        full_settings(),
    );
    assert_eq!(
        args(&p),
        [
            "--output-format",
            "json",
            "--model",
            "gpt-6.1-sol",
            "--reasoning-effort",
            "xhigh",
            "--allow-all-tools",
            "--resume=27f8d8a7-0ee0-42f7-a391-a088314703c2",
            "--extra",
            "two words"
        ]
    );
}

#[test]
fn copilot_pong() {
    let (events, r) = run_fixture(CliId::Copilot, "pong.jsonl", 0, "");
    assert_eq!(sessions(&events), ["27f8d8a7-0ee0-42f7-a391-a088314703c2"]);
    assert_eq!(messages(&events), ["pong"]);
    assert_eq!(finals(&events), ["pong"]);
    assert_eq!(has_partial_or_thinking(&events), (true, true));
    assert_eq!(r.answer.as_deref(), Some("pong"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("27f8d8a7-0ee0-42f7-a391-a088314703c2")
    );
    assert_eq!(r.error, None);
}

#[test]
fn copilot_tool_and_resume() {
    let (events, r) = run_fixture(CliId::Copilot, "tool.jsonl", 0, "");
    assert_eq!(tools(&events), [("view", "C:\\work\\copilot\\hello.txt")]);
    assert_eq!(finals(&events), ["banana"]);
    assert_eq!(r.answer.as_deref(), Some("banana"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("96a05683-d37c-48bb-8afd-a3ce46273678")
    );

    let (events, r) = run_fixture(CliId::Copilot, "resume.jsonl", 0, "");
    assert_eq!(sessions(&events), ["27f8d8a7-0ee0-42f7-a391-a088314703c2"]);
    assert_eq!(r.answer.as_deref(), Some("pong"));
}

#[test]
fn copilot_streamed_tool_input_uses_the_sanitized_path() {
    let text = fixture("copilot", "tool.jsonl");
    let mut input = String::new();
    for line in text.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        if event["type"] == "assistant.tool_call_delta" {
            input.push_str(event["data"]["inputDelta"].as_str().unwrap());
        }
    }
    let arguments: serde_json::Value = serde_json::from_str(&input).unwrap();
    assert_eq!(arguments["path"], r"C:\work\copilot\hello.txt");
    assert_eq!(arguments["view_range"], serde_json::json!([1, 1]));
}

#[test]
fn copilot_bad_model_reports_stderr() {
    let stderr = fixture("copilot", "bad-model.stderr");
    let r = adapter(CliId::Copilot).finish(&StreamState::default(), Some(1), &stderr);
    assert_eq!(
        r.error.as_deref(),
        Some(
            "copilot exited with code 1: Error: Model \"nope-model\" from --model flag is not available."
        )
    );
}

#[test]
fn copilot_models() {
    let id = CliId::Copilot;
    assert_eq!(
        model(id, Some("github-copilot/claude-opus-5.5"))
            .arg
            .as_deref(),
        Some("claude-opus-5.5")
    );
    assert_eq!(
        model(id, Some("anthropic/claude-sonnet-4.5"))
            .arg
            .as_deref(),
        Some("claude-sonnet-4.5")
    );
    assert_eq!(
        model(id, Some("openai/gpt-6.1-sol:high"))
            .thinking
            .as_deref(),
        Some("high")
    );
    assert_eq!(
        refusal(id, "deepseek/deepseek-r2"),
        "model: deepseek/deepseek-r2 cannot run through GitHub Copilot CLI — use a github-copilot/<id> model (or anthropic/, openai/, google/, xai/), or another agent"
    );
    assert!(refusal(id, "openai/gpt-6.1-sol:ultra").contains("has no thinking level \"ultra\""));
}

// -------------------------------------------------------------------------------------- claude

#[test]
fn claude_plan() {
    let id = CliId::Claude;
    let p = plan(id, model(id, None), None, CliSettings::default());
    assert_stdin_plan(id, &p);
    assert_eq!(
        args(&p),
        ["-p", "--output-format", "stream-json", "--verbose"]
    );

    let p = plan(
        id,
        model(id, Some("anthropic/claude-sonnet-5:max")),
        Some("5d1c7a3e"),
        full_settings(),
    );
    assert_eq!(
        args(&p),
        [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "claude-sonnet-5",
            "--effort",
            "max",
            "--dangerously-skip-permissions",
            "--resume",
            "5d1c7a3e",
            "--extra",
            "two words"
        ]
    );
}

#[test]
fn claude_pong_and_tool() {
    let (events, r) = run_fixture(CliId::Claude, "pong.jsonl", 0, "");
    assert_eq!(sessions(&events), ["5d1c7a3e-2b9f-4e0a-9c1d-8f6b2a4e7c10"]);
    assert_eq!(messages(&events), ["pong"]);
    assert_eq!(finals(&events), ["pong"]);
    assert_eq!(r.answer.as_deref(), Some("pong"));
    assert_eq!(r.error, None);

    let (events, r) = run_fixture(CliId::Claude, "tool.jsonl", 0, "");
    assert_eq!(tools(&events), [("Read", "C:\\work\\claude\\hello.txt")]);
    assert!(has_partial_or_thinking(&events).1);
    assert_eq!(finals(&events), ["banana"]);
    assert_eq!(r.answer.as_deref(), Some("banana"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("7e2a9b4c-3d1f-4a6e-8b2c-9d0e1f2a3b4c")
    );
}

#[test]
fn claude_auth_error_is_an_error_not_an_answer() {
    let (events, r) = run_fixture(CliId::Claude, "auth-error.jsonl", 1, "");
    assert_eq!(sessions(&events), ["32423e92-ca3e-4182-958c-a2ca0aebbf01"]);
    assert!(finals(&events).is_empty());
    assert!(messages(&events).is_empty());
    // The API error arrives as a synthetic assistant message and again as the result: shown once.
    assert_eq!(errors(&events).len(), 1);
    assert_eq!(r.answer, None);
    let e = r.error.unwrap();
    assert!(e.starts_with("API Error: 400"), "{e}");
    assert!(!e.contains('\n'));
}

#[test]
fn claude_partial_messages_when_asked_for() {
    let lines = [
        r#"{"type":"stream_event","event":{"type":"message_start","message":{}},"session_id":"s1"}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"po"}},"session_id":"s1"}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ng"}},"session_id":"s1"}"#,
    ];
    let (events, _) = replay(CliId::Claude, &lines.join("\n"));
    let last = events.iter().rev().find_map(|e| match e {
        LiveEvent::Partial { text, .. } => Some(text.as_str()),
        _ => None,
    });
    assert_eq!(last, Some("pong"));
}

#[test]
fn claude_models() {
    let id = CliId::Claude;
    let m = model(id, Some("anthropic/claude-sonnet-5"));
    assert_eq!(
        (m.arg.as_deref(), m.display.as_str()),
        (Some("claude-sonnet-5"), "anthropic/claude-sonnet-5")
    );
    assert_eq!(
        refusal(id, "openai/gpt-6.1-sol"),
        "model: openai/gpt-6.1-sol cannot run through Claude Code — use an anthropic/* model, or another agent"
    );
    assert_eq!(
        refusal(id, "anthropic/claude-sonnet-5:minimal"),
        "model: anthropic/claude-sonnet-5:minimal: Claude Code has no thinking level \"minimal\" — use one of low, medium, high, xhigh, max"
    );
    let with_default = CliSettings {
        default_model: Some("sonnet".into()),
        ..Default::default()
    };
    let m = adapter(id).resolve_model(None, &with_default).unwrap();
    assert_eq!(
        (m.arg.as_deref(), m.display.as_str()),
        (Some("sonnet"), "sonnet")
    );
    let with_default = CliSettings {
        default_model: Some("anthropic/claude-opus-5:high".into()),
        ..Default::default()
    };
    let m = adapter(id).resolve_model(None, &with_default).unwrap();
    assert_eq!(m.arg.as_deref(), Some("claude-opus-5"));
    assert_eq!(m.thinking.as_deref(), Some("high"));
    let bad_default = CliSettings {
        default_model: Some("google/gemini-3-pro".into()),
        ..Default::default()
    };
    assert!(adapter(id).resolve_model(None, &bad_default).is_err());
}

// --------------------------------------------------------------------------------------- codex

#[test]
fn codex_plan() {
    let id = CliId::Codex;
    let p = plan(id, model(id, None), None, CliSettings::default());
    assert_stdin_plan(id, &p);
    assert_eq!(args(&p), ["exec", "--json", "--skip-git-repo-check", "-"]);

    let p = plan(
        id,
        model(id, Some("openai/gpt-6.1-sol:high")),
        Some("0199a213-81c0-7800-8aa1-bbab2a035a53"),
        full_settings(),
    );
    assert_eq!(
        args(&p),
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-m",
            "gpt-6.1-sol",
            "-c",
            "model_reasoning_effort=high",
            "--sandbox",
            "workspace-write",
            "--extra",
            "two words",
            "resume",
            "0199a213-81c0-7800-8aa1-bbab2a035a53",
            "-"
        ]
    );
}

#[test]
fn codex_pong_and_tool() {
    let (events, r) = run_fixture(CliId::Codex, "pong.jsonl", 0, "");
    assert_eq!(sessions(&events), ["0199a213-81c0-7800-8aa1-bbab2a035a53"]);
    assert_eq!(messages(&events), ["pong"]);
    assert_eq!(finals(&events), ["pong"]);
    assert!(has_partial_or_thinking(&events).1);
    assert_eq!(r.answer.as_deref(), Some("pong"));

    let (events, r) = run_fixture(CliId::Codex, "tool.jsonl", 0, "");
    assert_eq!(
        tools(&events),
        [("shell", "powershell.exe -Command 'Get-Content hello.txt'")]
    );
    assert_eq!(r.answer.as_deref(), Some("banana"));
    assert_eq!(r.error, None);
}

#[test]
fn codex_auth_error() {
    let (events, r) = run_fixture(CliId::Codex, "auth-error.jsonl", 1, "");
    assert_eq!(sessions(&events), ["01a18880-d4b2-70d1-82e1-c1c2d0875f80"]);
    assert!(errors(&events).len() > 5);
    assert_eq!(r.answer, None);
    assert!(
        r.error
            .unwrap()
            .starts_with("unexpected status 401 Unauthorized")
    );
}

#[test]
fn codex_retry_errors_do_not_fail_a_completed_turn() {
    let lines = [
        r#"{"type":"thread.started","thread_id":"t1"}"#,
        r#"{"type":"error","message":"Reconnecting... 1/5"}"#,
        r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"ok"}}"#,
        r#"{"type":"turn.completed","usage":{}}"#,
    ];
    let (_, state) = replay(CliId::Codex, &lines.join("\n"));
    let r = adapter(CliId::Codex).finish(&state, Some(0), "");
    assert_eq!((r.answer.as_deref(), r.error), (Some("ok"), None));
}

#[test]
fn codex_models() {
    let id = CliId::Codex;
    assert_eq!(
        model(id, Some("openai/gpt-6.1-sol")).arg.as_deref(),
        Some("gpt-6.1-sol")
    );
    assert!(
        refusal(id, "anthropic/claude-sonnet-5")
            .ends_with("— use an openai/* model, or another agent")
    );
    assert!(refusal(id, "openai/gpt-6.1-sol:high effort").contains("is not a single word"));
}

// -------------------------------------------------------------------------------------- gemini

#[test]
fn gemini_plan() {
    let id = CliId::Gemini;
    let p = plan(id, model(id, None), None, CliSettings::default());
    assert_stdin_plan(id, &p);
    assert_eq!(args(&p), ["--output-format", "stream-json"]);

    let p = plan(
        id,
        model(id, Some("google/gemini-2.5-pro")),
        Some("8a3f1c2e"),
        full_settings(),
    );
    assert_eq!(
        args(&p),
        [
            "--output-format",
            "stream-json",
            "-m",
            "gemini-2.5-pro",
            "--approval-mode",
            "yolo",
            "--resume",
            "8a3f1c2e",
            "--extra",
            "two words"
        ]
    );
}

#[test]
fn gemini_pong_tool_error() {
    let (events, r) = run_fixture(CliId::Gemini, "pong.jsonl", 0, "");
    assert_eq!(sessions(&events), ["8a3f1c2e-4b5d-4e6f-9a0b-1c2d3e4f5a6b"]);
    assert_eq!(finals(&events), ["pong"]);
    assert_eq!(r.answer.as_deref(), Some("pong"));

    let (events, r) = run_fixture(CliId::Gemini, "tool.jsonl", 0, "");
    assert_eq!(
        tools(&events),
        [("read_file", "C:\\work\\gemini\\hello.txt")]
    );
    assert_eq!(messages(&events), ["I will read the file.", "banana"]);
    assert_eq!(r.answer.as_deref(), Some("banana"));

    let (events, r) = run_fixture(CliId::Gemini, "error.jsonl", 1, "");
    assert_eq!(errors(&events).len(), 2);
    assert_eq!(r.answer, None);
    assert_eq!(
        r.error.as_deref(),
        Some("[API Error: 429 Resource exhausted. Please try again later.]")
    );
}

#[test]
fn gemini_no_auth_reports_stderr() {
    let stderr = fixture("gemini", "no-auth.stderr");
    let r = adapter(CliId::Gemini).finish(&StreamState::default(), Some(41), &stderr);
    assert!(
        r.error
            .unwrap()
            .starts_with("gemini exited with code 41: Please set an Auth method")
    );
}

#[test]
fn gemini_models() {
    let id = CliId::Gemini;
    assert_eq!(
        model(id, Some("google/gemini-2.5-flash")).arg.as_deref(),
        Some("gemini-2.5-flash")
    );
    assert_eq!(
        refusal(id, "google/gemini-2.5-pro:high"),
        "model: google/gemini-2.5-pro:high: Gemini CLI has no thinking-level option — drop \":high\", or use another agent"
    );
    assert!(refusal(id, "openai/gpt-6.1-sol").contains("use a google/* model"));
}

// ------------------------------------------------------------------------------------ opencode

#[test]
fn opencode_plan() {
    let id = CliId::Opencode;
    let p = plan(id, model(id, None), None, CliSettings::default());
    assert_stdin_plan(id, &p);
    assert_eq!(args(&p), ["run", "--format", "json"]);

    let p = plan(
        id,
        model(id, Some("anthropic/claude-sonnet-5:high")),
        Some("ses_e777d7951ffeU3MsCRLNcNPwVX"),
        full_settings(),
    );
    assert_eq!(
        args(&p),
        [
            "run",
            "--format",
            "json",
            "-m",
            "anthropic/claude-sonnet-5",
            "--variant",
            "high",
            "--auto",
            "--session",
            "ses_e777d7951ffeU3MsCRLNcNPwVX",
            "--extra",
            "two words"
        ]
    );
}

#[test]
fn opencode_pong_tool_resume() {
    let (events, r) = run_fixture(CliId::Opencode, "pong.jsonl", 0, "");
    assert_eq!(sessions(&events), ["ses_e777d7951ffeU3MsCRLNcNPwVX"]);
    assert_eq!(finals(&events), ["pong"]);
    assert_eq!(r.answer.as_deref(), Some("pong"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("ses_e777d7951ffeU3MsCRLNcNPwVX")
    );

    let (events, r) = run_fixture(CliId::Opencode, "tool.jsonl", 0, "");
    assert_eq!(tools(&events), [("read", "C:\\work\\opencode\\hello.txt")]);
    assert_eq!(finals(&events), ["banana"]);
    assert_eq!(r.answer.as_deref(), Some("banana"));

    let (events, r) = run_fixture(CliId::Opencode, "resume.jsonl", 0, "");
    assert_eq!(sessions(&events), ["ses_e777d7951ffeU3MsCRLNcNPwVX"]);
    assert_eq!(r.answer.as_deref(), Some("pong"));
}

#[test]
fn opencode_bad_model() {
    let (events, r) = run_fixture(CliId::Opencode, "bad-model.jsonl", 1, "");
    assert_eq!(
        errors(&events),
        ["Unexpected server error. Check server logs for details."]
    );
    assert_eq!(r.answer, None);
    assert_eq!(
        r.error.as_deref(),
        Some("Unexpected server error. Check server logs for details.")
    );
}

#[test]
fn opencode_models() {
    let id = CliId::Opencode;
    let m = model(id, Some("openrouter/meta/llama-4"));
    assert_eq!(m.arg.as_deref(), Some("openrouter/meta/llama-4"));
    assert!(refusal(id, "anthropic/claude-sonnet-5:very high").contains("is not a single word"));
}

// -------------------------------------------------------------------------------------- shared

#[test]
fn refusals_are_one_line_and_never_substitute() {
    let cases = [
        (CliId::Claude, "openai/gpt-6.1-sol"),
        (CliId::Codex, "google/gemini-2.5-pro"),
        (CliId::Gemini, "anthropic/claude-sonnet-5"),
        (CliId::Gemini, "google/gemini-2.5-pro:low"),
        (CliId::Copilot, "mistral/large"),
        (CliId::Pi, "openai/gpt-6.1-sol:warp"),
    ];
    for (id, m) in cases {
        let msg = refusal(id, m);
        assert!(msg.starts_with("model: "), "{msg}");
        assert!(msg.contains(" — "), "{msg}");
        assert!(!msg.contains('\n'), "{msg}");
    }
    let empty = ModelSpec {
        provider: "anthropic".into(),
        id: String::new(),
        thinking: None,
    };
    let msg = adapter(CliId::Claude)
        .resolve_model(Some(&empty), &CliSettings::default())
        .unwrap_err()
        .message;
    assert!(msg.contains("is not provider/id"), "{msg}");
}

#[test]
fn defaults_are_the_cli_default() {
    for id in CliId::ALL {
        let m = model(id, None);
        assert_eq!(m.arg, None);
        assert_eq!(m.thinking, None);
        assert_eq!(m.display, format!("{id} default"));
    }
}

#[test]
fn tool_flags_only_when_allowed() {
    let flags = [
        "--allow-all-tools",
        "--dangerously-skip-permissions",
        "--sandbox",
        "--approval-mode",
        "--auto",
        "--yolo",
    ];
    for id in CliId::ALL {
        let p = plan(id, model(id, None), None, CliSettings::default());
        for f in flags {
            assert!(!p.args.iter().any(|a| a == f), "{id} passes {f} by default");
        }
    }
}

#[test]
fn every_adapter_resumes() {
    for id in CliId::ALL {
        assert!(adapter(id).supports_resume(), "{id}");
    }
}

#[test]
fn garbage_never_panics_and_changes_nothing() {
    let garbage = [
        "",
        "   ",
        "not json at all",
        "{",
        "{\"type\":",
        "[1,2,3]",
        "null",
        "42",
        "\"a string\"",
        "{\"type\":5}",
        "{\"type\":null}",
        "{\"type\":\"unknown.event\"}",
        "\u{feff}{\"x\":1}",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":null}",
        "{\"type\":\"message_end\",\"message\":\"str\"}",
        "{\"type\":\"agent_end\",\"messages\":{}}",
        "{\"type\":\"assistant\",\"message\":[]}",
        "{\"type\":\"item.completed\",\"item\":7}",
        "{\"type\":\"tool_use\",\"part\":[]}",
        "{\"type\":\"tool.execution_start\",\"data\":[]}",
        "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\"}}",
        "\u{1b}[31mred\u{1b}[0m",
        "🦀🦀🦀",
    ];
    for id in CliId::ALL {
        let a = adapter(id);
        let mut state = StreamState::default();
        for g in garbage {
            let _ = a.parse_line(g, &mut state, 0);
        }
        assert_eq!(state.session_id, None, "{id}");
        assert_eq!(state.final_text, None, "{id}");
    }
}

#[test]
fn torn_fixture_lines_never_panic() {
    let all = [
        (CliId::Pi, "tool.jsonl"),
        (CliId::Copilot, "tool.jsonl"),
        (CliId::Claude, "tool.jsonl"),
        (CliId::Codex, "auth-error.jsonl"),
        (CliId::Gemini, "tool.jsonl"),
        (CliId::Opencode, "tool.jsonl"),
    ];
    for (id, name) in all {
        let text = fixture(id.as_str(), name);
        let a = adapter(id);
        let mut state = StreamState::default();
        for line in text.lines() {
            for (i, _) in line.char_indices().step_by(7) {
                let _ = a.parse_line(&line[..i], &mut state, 0);
            }
        }
    }
}

#[test]
fn live_events_are_bounded() {
    let long = "word ".repeat(5000);
    let line = serde_json::json!({
        "type": "item.completed",
        "item": {"id": "i", "type": "command_execution", "command": long, "aggregated_output": "", "exit_code": 0, "status": "completed"}
    })
    .to_string();
    let (events, _) = replay(CliId::Codex, &line);
    let (_, summary) = tools(&events)[0];
    assert!(summary.chars().count() <= 121);

    let delta =
        serde_json::json!({"type": "assistant.message_delta", "data": {"deltaContent": long}})
            .to_string();
    let (events, _) = replay(CliId::Copilot, &delta);
    match &events[0] {
        LiveEvent::Partial { text, .. } => assert!(text.chars().count() <= 2001),
        e => panic!("{e:?}"),
    }
}

#[test]
fn transcript_formats() {
    let formats: Vec<&str> = CliId::ALL
        .iter()
        .map(|id| adapter(*id).transcript_format())
        .collect();
    assert_eq!(
        formats,
        [
            "pi/json",
            "copilot/jsonl",
            "claude/stream-json",
            "codex/exec-json",
            "gemini/stream-json",
            "opencode/json"
        ]
    );
}

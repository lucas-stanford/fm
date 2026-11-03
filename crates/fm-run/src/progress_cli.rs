//! `fm step|next|say|note|message|status`: the verbs a run's agent, hooks and whatever they start
//! use to report progress. Found through `AUTOMATION_RUN`, `AUTOMATION_NODE` and `AUTOMATION_STATE_DIR`. A port of
//! pi-automations' `progress-cli.ts`: the same flags, output and exit codes (0 ok, 1 usage,
//! 2 refused). The `fm` binary forwards its arguments here.

use chrono::{DateTime, Utc};
use fm_model::StatePaths;
use fm_model::{HistoryEntry, RunLink, RunNote, StepStatus};
use fm_state::progress::{SETTABLE, strip_controls};
use fm_state::{
    Env, LinkedArgs, NextArgs, Os, ProgressCode, ProgressError, ProgressNode, ProgressResult,
    ProgressSnapshot, SayArgs, StepArgs, progress_message, progress_next, progress_note,
    progress_say, progress_status, progress_step, state_dir,
};
use serde::Serialize;

use crate::tracker::{ENV_NODE, ENV_RUN, ENV_STATE_DIR};

/// The verbs that work inside a run.
pub const IN_RUN_VERBS: [&str; 6] = ["step", "next", "say", "note", "message", "status"];

/// Exit codes.
pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 1;
pub const EXIT_REFUSED: i32 = 2;

/// What one verb printed, and how it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// True for a verb [`progress_cli`] handles.
pub fn is_in_run_verb(verb: &str) -> bool {
    IN_RUN_VERBS.contains(&verb)
}

/// How a node's status is drawn, as on the run page.
pub fn glyph(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Done => "✓",
        StepStatus::Active => "▶",
        StepStatus::Waiting => "⏸",
        StepStatus::Failed => "✗",
        StepStatus::Skipped => "↷",
        StepStatus::Interrupted => "‼",
        StepStatus::Pending => "·",
    }
}

fn status_name(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Pending => "pending",
        StepStatus::Active => "active",
        StepStatus::Waiting => "waiting",
        StepStatus::Done => "done",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "skipped",
        StepStatus::Interrupted => "interrupted",
    }
}

fn usage(message: impl Into<String>) -> ProgressError {
    ProgressError {
        code: ProgressCode::Usage,
        message: message.into(),
        suggestions: Vec::new(),
    }
}

#[derive(Debug, Default)]
struct Parsed {
    words: Vec<String>,
    json: bool,
    run: Option<String>,
    status: Option<String>,
    message: Option<String>,
    from: Option<String>,
    node: Option<String>,
    request_id: Option<String>,
    links: Vec<String>,
    format: Option<String>,
}

fn parse(argv: &[String]) -> Result<Parsed, ProgressError> {
    let mut out = Parsed::default();
    let mut i = 0;
    while i < argv.len() {
        let raw = &argv[i];
        if raw == "--" {
            out.words.extend(argv[i + 1..].iter().cloned());
            break;
        }
        let eq = if raw.starts_with("--") {
            raw.find('=').filter(|&e| e > 0)
        } else {
            None
        };
        let name = eq.map_or(raw.as_str(), |e| &raw[..e]);
        let inline = eq.map(|e| raw[e + 1..].to_string());
        let mut take = || -> Result<String, ProgressError> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            let v = argv
                .get(i + 1)
                .cloned()
                .ok_or_else(|| usage(format!("{name} needs a value")))?;
            i += 1;
            Ok(v)
        };
        match name {
            "--json" => out.json = true,
            "--link" => out.links.push(take()?),
            "--run" => out.run = Some(take()?),
            "--status" => out.status = Some(take()?),
            "-m" | "--message" => out.message = Some(take()?),
            "--from" => out.from = Some(take()?),
            "--node" => out.node = Some(take()?),
            "--request-id" => out.request_id = Some(take()?),
            "--format" => out.format = Some(take()?),
            n if n.starts_with('-') && n != "-" => {
                return Err(usage(format!("unknown option {n}")));
            }
            _ => out.words.push(raw.clone()),
        }
        i += 1;
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeJson<'a> {
    id: &'a str,
    label: &'a str,
    status: StepStatus,
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<&'a str>,
}

fn node_json(n: &ProgressNode) -> NodeJson<'_> {
    NodeJson {
        id: &n.id,
        label: &n.label,
        status: n.status,
        text: n.text.as_deref(),
        started_at: n.started_at.as_deref(),
        ended_at: n.ended_at.as_deref(),
    }
}

#[derive(Serialize)]
struct StatusJson<'a> {
    run: &'a str,
    name: &'a str,
    closed: bool,
    nodes: Vec<NodeJson<'a>>,
    taken: &'a [(String, String)],
    notes: &'a [RunNote],
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a RunNote>,
    history: &'a [HistoryEntry],
}

fn status_json(s: &ProgressSnapshot) -> StatusJson<'_> {
    StatusJson {
        run: &s.run,
        name: &s.name,
        closed: s.closed,
        nodes: s.nodes.iter().map(node_json).collect(),
        taken: &s.taken,
        notes: &s.notes,
        message: s.message.as_ref(),
        history: &s.history,
    }
}

#[derive(Serialize)]
struct WriteJson<'a> {
    run: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<NodeJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a RunNote>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Data<'a> {
    Status(StatusJson<'a>),
    Write(WriteJson<'a>),
}

#[derive(Serialize)]
struct OkJson<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
    warnings: &'a [String],
    data: Data<'a>,
}

#[derive(Serialize)]
struct ErrBody<'a> {
    code: ProgressCode,
    message: &'a str,
    suggestions: &'a [String],
}

#[derive(Serialize)]
struct ErrJson<'a> {
    ok: bool,
    error: ErrBody<'a>,
}

fn link_text(l: &RunLink) -> String {
    let t = match &l.label {
        Some(label) => format!("{label}: {}", l.url),
        None => l.url.clone(),
    };
    strip_controls(&t, false)
}

fn flat(s: &str) -> String {
    strip_controls(&s.split_whitespace().collect::<Vec<_>>().join(" "), false)
        .trim()
        .to_string()
}

fn pad(s: &str, width: usize) -> String {
    let n = s.chars().count();
    format!("{s}{}", " ".repeat(width.saturating_sub(n)))
}

/// `fm status` for a person. Nothing a chart or an agent wrote reaches the terminal as a control.
pub fn status_lines(s: &ProgressSnapshot) -> Vec<String> {
    let mut out = vec![format!(
        "{} · run {}{}",
        strip_controls(&s.name, false),
        s.run,
        if s.closed { " · ended" } else { "" }
    )];
    let width = s
        .nodes
        .iter()
        .map(|n| n.id.chars().count())
        .max()
        .unwrap_or(0)
        .max(2);
    for n in &s.nodes {
        let label = if n.label != n.id {
            format!("  {}", strip_controls(&n.label, false))
        } else {
            String::new()
        };
        let text = n
            .text
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| format!(" — {}", flat(t).chars().take(100).collect::<String>()))
            .unwrap_or_default();
        let line = format!(
            "  {} {}  {}{label}{text}",
            glyph(n.status),
            pad(&strip_controls(&n.id, false), width),
            pad(status_name(n.status), 8)
        );
        out.push(line.trim_end().to_string());
    }
    if !s.taken.is_empty() {
        let arrows: Vec<String> = s
            .taken
            .iter()
            .map(|(a, b)| strip_controls(&format!("{a}→{b}"), false))
            .collect();
        out.push(format!("taken  {}", arrows.join(", ")));
    }
    for note in &s.notes {
        out.push(format!("note   {}", flat(&note.text)));
        for l in &note.links {
            out.push(format!("       {}", link_text(l)));
        }
    }
    if let Some(m) = &s.message {
        out.push(format!("message {}", flat(&m.text)));
        for l in &m.links {
            out.push(format!("        {}", link_text(l)));
        }
    }
    out
}

enum Done {
    Write(Box<ProgressResult>, String),
    Status(Box<ProgressSnapshot>, String),
}

fn plural(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!(" with {n} link{}", if n == 1 { "" } else { "s" })
    }
}

fn run_verb(
    verb: &str,
    p: &Parsed,
    env: &dyn Env,
    paths: &StatePaths,
    now: DateTime<Utc>,
) -> Result<Done, ProgressError> {
    let run_id = p.run.clone().or_else(|| env.var(ENV_RUN));
    let run_id = run_id.as_deref();
    let env_node = env.var(ENV_NODE);
    let text_from = |from: usize| p.words.get(from..).unwrap_or_default().join(" ");
    let unchanged = |r: &ProgressResult, what: &str| {
        if r.changed {
            String::new()
        } else {
            format!(" ({what})")
        }
    };
    match verb {
        "step" => {
            let node = match (p.words.get(1), p.words.len()) {
                (Some(n), 2) => n.clone(),
                _ => {
                    return Err(usage(
                        "usage: fm step <node> [--status active|waiting|done|failed|skipped] [-m \"text\"]",
                    ));
                }
            };
            let status = match &p.status {
                None => None,
                Some(s) => Some(
                    SETTABLE
                        .iter()
                        .copied()
                        .find(|st| status_name(*st) == s)
                        .ok_or_else(|| {
                            let names: Vec<&str> =
                                SETTABLE.iter().map(|s| status_name(*s)).collect();
                            usage(format!("--status must be one of {}", names.join("|")))
                        })?,
                ),
            };
            let r = progress_step(
                paths,
                run_id,
                &StepArgs {
                    node,
                    status,
                    text: p.message.clone(),
                },
                now,
            )?;
            let human = match &r.node {
                Some(n) => format!(
                    "{} {}{}",
                    n.id,
                    status_name(n.status),
                    unchanged(&r, "unchanged")
                ),
                None => String::new(),
            };
            Ok(Done::Write(Box::new(r), human))
        }
        "next" => {
            let to = match (p.words.get(1), p.words.len()) {
                (Some(n), 2) => n.clone(),
                _ => return Err(usage("usage: fm next <to> [--from <node>] [-m \"text\"]")),
            };
            let r = progress_next(
                paths,
                run_id,
                &NextArgs {
                    to,
                    from: p.from.clone(),
                    env_node,
                    text: p.message.clone(),
                },
                now,
            )?;
            let human = format!(
                "{} active, {} {}{}",
                r.node.as_ref().map(|n| n.id.as_str()).unwrap_or_default(),
                r.from.as_ref().map(|n| n.id.as_str()).unwrap_or_default(),
                r.from
                    .as_ref()
                    .map(|n| status_name(n.status))
                    .unwrap_or_default(),
                unchanged(&r, "unchanged")
            );
            Ok(Done::Write(Box::new(r), human))
        }
        "say" => {
            let text = text_from(1);
            if text.trim().is_empty() {
                return Err(usage(
                    "usage: fm say \"text\" [--node <node>] [--request-id <key>]",
                ));
            }
            let r = progress_say(
                paths,
                run_id,
                &SayArgs {
                    text,
                    node: p.node.clone(),
                    env_node,
                    request_id: p.request_id.clone(),
                },
                now,
            )?;
            let human = format!(
                "said on {}{}",
                r.node.as_ref().map(|n| n.id.as_str()).unwrap_or_default(),
                unchanged(&r, "already said")
            );
            Ok(Done::Write(Box::new(r), human))
        }
        "note" | "message" => {
            let text = text_from(1);
            if text.trim().is_empty() {
                return Err(usage(format!(
                    "usage: fm {verb} \"text\" [--link \"label=https://…\"]… [--request-id <key>]"
                )));
            }
            let args = LinkedArgs {
                text,
                links: p.links.clone(),
                request_id: p.request_id.clone(),
            };
            let n = p.links.len();
            if verb == "note" {
                let r = progress_note(paths, run_id, &args, now)?;
                let human = format!("noted{}{}", plural(n), unchanged(&r, "already noted"));
                Ok(Done::Write(Box::new(r), human))
            } else {
                let r = progress_message(paths, run_id, &args, now)?;
                let human = format!("message left{}{}", plural(n), unchanged(&r, "already left"));
                Ok(Done::Write(Box::new(r), human))
            }
        }
        "status" => {
            if p.words.len() > 1 {
                return Err(usage("usage: fm status [--json | --format text|markdown]"));
            }
            let markdown = match p.format.as_deref() {
                None | Some("text") => false,
                Some("markdown") if !p.json => true,
                Some("markdown") => {
                    return Err(usage(
                        "--json and --format markdown are two answers; pick one",
                    ));
                }
                Some(other) => {
                    return Err(usage(format!(
                        "--format must be text or markdown, not {other}"
                    )));
                }
            };
            let s = progress_status(paths, run_id)?;
            let human = if markdown {
                crate::markdown::status_markdown(&s, now)
                    .trim_end()
                    .to_string()
            } else {
                status_lines(&s).join("\n")
            };
            Ok(Done::Status(Box::new(s), human))
        }
        other => Err(usage(format!(
            "unknown command {other} — use one of {}",
            IN_RUN_VERBS.join(", ")
        ))),
    }
}

fn state_paths_from(env: &dyn Env) -> Result<StatePaths, ProgressError> {
    if let Some(dir) = env.var(ENV_STATE_DIR) {
        return Ok(StatePaths::new(dir.trim()));
    }
    state_dir(env, Os::current())
        .map(StatePaths::new)
        .map_err(ProgressError::from)
}

/// Run one in-run verb: `args[0]` is the verb. Never panics, never exits the process.
pub fn progress_cli(args: &[String], env: &dyn Env, now: DateTime<Utc>) -> CliOutput {
    let verb = args.first().map(String::as_str).unwrap_or("");
    let mut json = args.iter().any(|a| a == "--json");
    let result = parse(args).and_then(|p| {
        json = p.json;
        let paths = state_paths_from(env)?;
        run_verb(verb, &p, env, &paths, now).map(|d| (p, d))
    });
    match result {
        Ok((_, done)) => {
            let (warnings, changed, human) = match &done {
                Done::Write(r, h) => (r.warnings.clone(), Some(r.changed), h.clone()),
                Done::Status(_, h) => (Vec::new(), None, h.clone()),
            };
            if json {
                let data = match &done {
                    Done::Status(s, _) => Data::Status(status_json(s)),
                    Done::Write(r, _) => Data::Write(WriteJson {
                        run: &r.snapshot.run,
                        node: r.node.as_ref().map(node_json),
                        notes: (verb == "note").then_some(r.snapshot.notes.len()),
                        message: if verb == "message" {
                            r.snapshot.message.as_ref()
                        } else {
                            None
                        },
                    }),
                };
                let body = OkJson {
                    ok: true,
                    changed,
                    warnings: &warnings,
                    data,
                };
                let line = serde_json::to_string(&body).unwrap_or_else(|_| "{\"ok\":true}".into());
                return CliOutput {
                    code: EXIT_OK,
                    stdout: format!("{line}\n"),
                    stderr: String::new(),
                };
            }
            CliOutput {
                code: EXIT_OK,
                stdout: format!("{human}\n"),
                stderr: warnings.iter().map(|w| format!("fm: {w}\n")).collect(),
            }
        }
        Err(err) => {
            let code = match err.code {
                ProgressCode::Usage | ProgressCode::BadLink => EXIT_USAGE,
                _ => EXIT_REFUSED,
            };
            if json {
                let body = ErrJson {
                    ok: false,
                    error: ErrBody {
                        code: err.code,
                        message: &err.message,
                        suggestions: &err.suggestions,
                    },
                };
                let line = serde_json::to_string(&body).unwrap_or_else(|_| "{\"ok\":false}".into());
                return CliOutput {
                    code,
                    stdout: format!("{line}\n"),
                    stderr: String::new(),
                };
            }
            let hint = if err.suggestions.is_empty() {
                String::new()
            } else {
                format!("\n  did you mean: {}", err.suggestions.join(", "))
            };
            CliOutput {
                code,
                stdout: String::new(),
                stderr: format!("fm: {}{hint}\n", err.message),
            }
        }
    }
}

/// The in-run half of `fm help`.
pub fn in_run_help_lines() -> Vec<String> {
    let pad = " ".repeat(22);
    vec![
        "Inside a run (AUTOMATION_RUN is set for the body, every hook and what they start):".into(),
        "fm step <node> [--status active|waiting|done|failed|skipped] [-m text]".into(),
        format!("{pad}set one node; active by default"),
        "fm next <to> [--from <node>] [-m text]".into(),
        format!(
            "{pad}finish <node> and start <to>; <node> defaults to AUTOMATION_NODE, else the one running node"
        ),
        "fm say <text> [--node <node>] [--request-id <key>]".into(),
        format!("{pad}add an update to a node; a request id makes a retry a no-op"),
        "fm note <text> [--link \"label=https://…\"]… [--request-id <key>]".into(),
        format!("{pad}a note on the run's record, with the links it published"),
        "fm message <text> [--link \"label=https://…\"]… [--request-id <key>]".into(),
        format!(
            "{pad}the run's one message for its reader: what happened, what needs them; the last one wins"
        ),
        "fm status             where the run is: every node, the arrows taken, the notes, the message"
            .into(),
        format!("{pad}every in-run command takes --json: one object on stdout, ok or not"),
    ]
}

#[cfg(test)]
mod tests;

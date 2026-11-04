//! The `AUTOMATION.md` parser: YAML 1.2 frontmatter, nine fields, everything else refused by name,
//! checked in the order the specification gives, first problem only (strict flow: all of them).

use std::time::Duration;

use fm_model::{
    Automation, CatchUp, Flow, Hook, Hooks, ModelSpec, NodeAgent, Schedule, Triggers, Watch,
};

use crate::chart::{is_reserved_node, parse_chart};
use crate::cron::{is_iana_zone, validate_cron};
use crate::duration::parse_duration;
use crate::flow_block::find_flow_block;
use crate::refusal::{Refusal, quote};
use crate::strict_flow::{check_strict_flow, flow_problem_line};
use crate::yaml::{Value, parse_yaml};

/// The closed field list. Adding one is a specification change, not a feature.
pub const FIELDS: [&str; 9] = [
    "name",
    "description",
    "enabled",
    "model",
    "timeout",
    "on",
    "hooks",
    "flow",
    "nodes",
];
/// The draft of the specification this parser implements, as its refusals name it.
pub const DRAFT: &str = "Draft 0.2.0";
/// What one entry of `nodes:` may say.
pub const NODE_KEYS: [&str; 3] = ["agent", "model", "timeout"];
/// The triggers this host implements.
pub const TRIGGERS: [&str; 3] = ["schedule", "manual", "watch"];
pub const SCHEDULE_KEYS: [&str; 3] = ["cron", "timezone", "catch-up"];
pub const WATCH_KEYS: [&str; 2] = ["script", "every"];
pub const NAME_MAX: usize = 64;
pub const DESCRIPTION_MAX: usize = 1024;
/// `on.watch.every` bounds: the clock ticks once a minute, and past a day a cron is better.
pub const WATCH_EVERY_MIN: Duration = Duration::from_secs(60);
pub const WATCH_EVERY_MAX: Duration = Duration::from_secs(24 * 3600);
/// The thinking levels a trailing `:<level>` on `model` can name; any other suffix is part of the id.
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `AUTOMATION.md` split at its frontmatter fences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub yaml: String,
    pub body: String,
    /// 1-based line of the file the body starts on.
    pub body_line: usize,
}

/// Split `---\n<yaml>\n---\n<body>`. `None` without an opening and a closing `---` line.
pub fn split_frontmatter(text: &str) -> Option<Frontmatter> {
    // A byte-order mark before the opening fence is invisible in an editor: ignore it.
    let src = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = src
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return None;
    }
    let close = (1..lines.len()).find(|&i| lines[i].trim() == "---")?;
    Some(Frontmatter {
        yaml: lines[1..close].join("\n"),
        body: lines[close + 1..].join("\n"),
        body_line: close + 2,
    })
}

/// True for `^[a-z0-9]+(-[a-z0-9]+)*$`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse `<provider>/<id>[:<thinking>]`. The id may hold colons; a trailing one is a thinking level
/// only when it names one of [`THINKING_LEVELS`].
pub fn parse_model_spec(spec: &str) -> Option<ModelSpec> {
    let (provider, rest) = spec.split_once('/')?;
    let mut pc = provider.chars();
    if !pc.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !pc.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    if rest.is_empty() || rest.contains('/') || rest.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(colon) = rest.rfind(':') {
        let maybe = &rest[colon + 1..];
        if colon > 0 && THINKING_LEVELS.contains(&maybe) {
            return Some(ModelSpec {
                provider: provider.to_string(),
                id: rest[..colon].to_string(),
                thinking: Some(maybe.to_string()),
            });
        }
    }
    Some(ModelSpec {
        provider: provider.to_string(),
        id: rest.to_string(),
        thinking: None,
    })
}

fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn sorted_unknown<'a>(map: &'a [(String, Value)], known: &[&str]) -> Vec<&'a str> {
    let mut unknown: Vec<&str> = map
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !known.contains(k))
        .collect();
    unknown.sort_unstable();
    unknown.dedup();
    unknown
}

/// Absolute on any OS: `/x`, `\x`, `C:\x`, `C:/x`.
fn is_absolute_anywhere(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/')
        || p.starts_with('\\')
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}

/// True when the relative path, normalized with either separator, climbs above its start.
fn leaves_directory(p: &str) -> bool {
    let mut depth: i64 = 0;
    for part in p.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

/// A script path is text, relative and inside the folder. Symbolic links are checked at discovery.
pub fn check_script_path(script: Option<&Value>, at: &str) -> Result<String, Refusal> {
    let text = match script {
        Some(Value::Str(s)) if !s.trim().is_empty() => s,
        _ => {
            return Err(Refusal::new(
                at,
                "is empty — give a path relative to the automation directory",
            ));
        }
    };
    if is_absolute_anywhere(text) {
        return Err(Refusal::new(
            at,
            format!(
                "{} is absolute — use a path relative to the automation directory",
                quote(text)
            ),
        ));
    }
    if leaves_directory(text) {
        return Err(Refusal::new(
            at,
            format!(
                "{} leaves the automation directory — scripts must live inside it",
                quote(text)
            ),
        ));
    }
    Ok(text.clone())
}

fn parse_frontmatter_yaml(yaml: &str) -> Result<Vec<(String, Value)>, Refusal> {
    let value = parse_yaml(yaml).map_err(|e| {
        let mut e = e;
        // An unquoted cron starting with `*` reads as a YAML alias; name the real cause.
        let cron_line = yaml.lines().position(|l| {
            let t = l.trim_start();
            ["cron", "schedule"].iter().any(|k| {
                t.strip_prefix(k)
                    .map(|r| r.trim_start())
                    .and_then(|r| r.strip_prefix(':'))
                    .is_some_and(|r| r.trim_start().starts_with('*'))
            })
        });
        let lower = e.message.to_lowercase();
        if let Some(i) = cron_line
            && (lower.contains("alias") || lower.contains("anchor") || e.line == i + 1)
        {
            e.fix = Some("quote the cron expression".to_string());
        }
        Refusal::new("frontmatter", e.to_string())
    })?;
    match value {
        Value::Null => Err(Refusal::new(
            "frontmatter",
            "is empty — Draft 0.2.0 needs at least name, description and on",
        )),
        Value::Map(m) => Ok(m),
        _ => Err(Refusal::new(
            "frontmatter",
            "must be a YAML mapping of fields",
        )),
    }
}

fn get<'a>(map: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .filter(|v| !v.is_null())
}

fn parse_name(front: &[(String, Value)], dir_name: &str) -> Result<String, Refusal> {
    let name = match get(front, "name") {
        None => {
            return Err(Refusal::new(
                "name",
                format!(
                    "required — ≤{NAME_MAX} chars, lowercase letters, digits and hyphens, same as the directory name"
                ),
            ));
        }
        Some(Value::Str(s)) => s,
        Some(_) => {
            return Err(Refusal::new(
                "name",
                "must be a string — lowercase letters, digits and hyphens",
            ));
        }
    };
    let len = js_len(name);
    if len > NAME_MAX {
        return Err(Refusal::new(
            "name",
            format!("is {len} characters — the maximum is {NAME_MAX}"),
        ));
    }
    if !is_valid_name(name) {
        return Err(Refusal::new(
            "name",
            format!(
                "{} is not a valid name — use lowercase letters, digits and single hyphens",
                quote(name)
            ),
        ));
    }
    if name != dir_name {
        return Err(Refusal::new(
            "name",
            format!(
                "{} does not match the directory name {} — they must be the same",
                quote(name),
                quote(dir_name)
            ),
        ));
    }
    Ok(name.clone())
}

fn parse_description(front: &[(String, Value)]) -> Result<String, Refusal> {
    let d = match get(front, "description") {
        None => {
            return Err(Refusal::new(
                "description",
                format!("required — ≤{DESCRIPTION_MAX} chars saying what this automation does"),
            ));
        }
        Some(Value::Str(s)) => s,
        Some(_) => {
            return Err(Refusal::new(
                "description",
                format!(
                    "must be a string — ≤{DESCRIPTION_MAX} chars saying what this automation does"
                ),
            ));
        }
    };
    let len = js_len(d);
    if len > DESCRIPTION_MAX {
        return Err(Refusal::new(
            "description",
            format!("is {len} characters — the maximum is {DESCRIPTION_MAX}"),
        ));
    }
    Ok(d.clone())
}

/// `on.schedule`: a cron string, or a mapping of `cron`, `timezone`, `catch-up`.
pub fn parse_schedule(v: &Value) -> Result<Schedule, Refusal> {
    let cron_problem = |e: String| Refusal::new("on.schedule.cron", e);
    if let Value::Str(s) = v {
        validate_cron(s).map_err(cron_problem)?;
        return Ok(Schedule {
            cron: s.trim().to_string(),
            timezone: None,
            catch_up: CatchUp::Skip,
        });
    }
    let Value::Map(map) = v else {
        return Err(Refusal::new(
            "on.schedule",
            format!(
                "must be a cron string or a mapping of {}",
                SCHEDULE_KEYS.join(", ")
            ),
        ));
    };
    if let Some(k) = sorted_unknown(map, &SCHEDULE_KEYS).first() {
        return Err(Refusal::new(
            format!("on.schedule.{k}"),
            format!("unknown key — on.schedule has {}", SCHEDULE_KEYS.join(", ")),
        ));
    }
    let cron = match get(map, "cron") {
        None => {
            return Err(Refusal::new(
                "on.schedule.cron",
                "required — 5 fields, for example \"0 7 * * MON-FRI\"",
            ));
        }
        Some(Value::Str(s)) => {
            validate_cron(s).map_err(cron_problem)?;
            s.trim().to_string()
        }
        Some(other) => {
            return Err(Refusal::new(
                "on.schedule.cron",
                format!("{other} is not a 5-field cron expression"),
            ));
        }
    };
    let timezone = match get(map, "timezone") {
        None => None,
        Some(Value::Str(tz)) if is_iana_zone(tz) => Some(tz.clone()),
        Some(other) => {
            return Err(Refusal::new(
                "on.schedule.timezone",
                format!("{other} is not an IANA zone — try America/Los_Angeles"),
            ));
        }
    };
    let catch_up = match get(map, "catch-up") {
        None => CatchUp::Skip,
        Some(Value::Str(s)) if s == "skip" => CatchUp::Skip,
        Some(Value::Str(s)) if s == "once" => CatchUp::Once,
        Some(other) => {
            return Err(Refusal::new(
                "on.schedule.catch-up",
                format!("{other} is not skip or once"),
            ));
        }
    };
    Ok(Schedule {
        cron,
        timezone,
        catch_up,
    })
}

/// `on.watch`: always `{ script, every }`.
pub fn parse_watch(v: &Value) -> Result<Watch, Refusal> {
    let Value::Map(map) = v else {
        return Err(Refusal::new(
            "on.watch",
            "must be { script: <path>, every: <duration> } — for example script: scripts/watch.js and every: 5m",
        ));
    };
    if let Some(k) = sorted_unknown(map, &WATCH_KEYS).first() {
        return Err(Refusal::new(
            format!("on.watch.{k}"),
            format!("unknown key — on.watch has {}", WATCH_KEYS.join(", ")),
        ));
    }
    let Some(script) = get(map, "script") else {
        return Err(Refusal::new(
            "on.watch.script",
            "required — the script that looks for work, relative to the automation directory",
        ));
    };
    let script = check_script_path(Some(script), "on.watch.script")?;
    let Some(every) = get(map, "every") else {
        return Err(Refusal::new(
            "on.watch.every",
            "required — how often to run the script, for example 5m",
        ));
    };
    let (text, duration) = match every {
        Value::Str(s) => (s, parse_duration(s)),
        _ => (&String::new(), None),
    };
    let Some(duration) = duration else {
        return Err(Refusal::new(
            "on.watch.every",
            format!("{every} is not a duration — use 1m, 5m, 1h"),
        ));
    };
    if duration < WATCH_EVERY_MIN {
        return Err(Refusal::new(
            "on.watch.every",
            format!("{text} is shorter than a minute — use 1m or more"),
        ));
    }
    if duration > WATCH_EVERY_MAX {
        return Err(Refusal::new(
            "on.watch.every",
            format!("{text} is longer than 24h — use on.schedule for anything rarer"),
        ));
    }
    Ok(Watch {
        script,
        every: text.clone(),
    })
}

fn parse_on(raw: Option<&Value>) -> Result<Triggers, Refusal> {
    let list = TRIGGERS.join(", ");
    let Some(raw) = raw else {
        return Err(Refusal::new(
            "on",
            format!("required — at least one trigger ({list})"),
        ));
    };
    let Value::Map(map) = raw else {
        return Err(Refusal::new(
            "on",
            format!("must be a mapping of trigger types ({list})"),
        ));
    };
    let none = || Refusal::new("on", format!("needs at least one trigger — {list}"));
    if map.is_empty() {
        return Err(none());
    }
    if let Some(k) = sorted_unknown(map, &TRIGGERS).first() {
        return Err(Refusal::new(
            format!("on.{k}"),
            format!("this host implements {list}"),
        ));
    }
    let mut on = Triggers::default();
    if let Some(manual) = get(map, "manual") {
        if *manual != Value::Bool(true) {
            return Err(Refusal::new(
                "on.manual",
                format!(
                    "{manual} is not true — write \"manual: true\", or remove it to disallow manual runs"
                ),
            ));
        }
        on.manual = true;
    }
    if let Some(s) = get(map, "schedule") {
        on.schedule = Some(parse_schedule(s)?);
    }
    if let Some(w) = get(map, "watch") {
        on.watch = Some(parse_watch(w)?);
    }
    if on.schedule.is_none() && on.watch.is_none() && !on.manual {
        return Err(none());
    }
    Ok(on)
}

fn parse_hook_list(raw: Option<&Value>, phase: &str) -> Result<Vec<Hook>, Refusal> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Value::Seq(items) = raw else {
        return Err(Refusal::new(
            format!("hooks.{phase}"),
            "must be a list of { script: <path> } or { prompt: <text> }",
        ));
    };
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let at = format!("hooks.{phase}[{i}]");
            let shape = || Refusal::new(&at, "must be { script: <path> } or { prompt: <text> }");
            let Value::Map(m) = item else {
                return Err(shape());
            };
            match m.as_slice() {
                [(k, v)] if k == "prompt" => match v {
                    Value::Str(p) if !p.trim().is_empty() => Ok(Hook::Prompt(p.clone())),
                    _ => Err(Refusal::new(
                        format!("{at}.prompt"),
                        "must be text — an empty prompt asks nothing",
                    )),
                },
                [(k, v)] if k == "script" => {
                    let script = if v.is_null() { None } else { Some(v) };
                    Ok(Hook::Script(check_script_path(
                        script,
                        &format!("{at}.script"),
                    )?))
                }
                _ => Err(shape()),
            }
        })
        .collect()
}

fn parse_hooks(raw: Option<&Value>) -> Result<Hooks, Refusal> {
    let Some(raw) = raw else {
        return Ok(Hooks::default());
    };
    let Value::Map(map) = raw else {
        return Err(Refusal::new(
            "hooks",
            "must be a mapping with pre and post lists",
        ));
    };
    if let Some(k) = sorted_unknown(map, &["pre", "post"]).first() {
        return Err(Refusal::new(
            format!("hooks.{k}"),
            "unknown key — hooks has pre, post",
        ));
    }
    Ok(Hooks {
        pre: parse_hook_list(get(map, "pre"), "pre")?,
        post: parse_hook_list(get(map, "post"), "post")?,
    })
}

fn parse_flow(raw: Option<&Value>) -> Result<Flow, Refusal> {
    match raw {
        None => Ok(Flow::Loose),
        Some(Value::Str(s)) if s == "loose" => Ok(Flow::Loose),
        Some(Value::Str(s)) if s == "strict" => Ok(Flow::Strict),
        Some(other) => Err(Refusal::new(
            "flow",
            format!("{other} is not loose or strict"),
        )),
    }
}

/// Parse and validate one `AUTOMATION.md` whose folder is `dir_name`. Returns the automation and
/// its chart (the first Mermaid flowchart in the body), or the first refusal.
///
/// Only the file is checked; the machine checks (scripts runnable, model, host maximum timeout) are
/// [`crate::discover::check_machine`]'s.
pub fn parse_automation(
    text: &str,
    dir_name: &str,
) -> Result<(Automation, Option<String>), Refusal> {
    let Some(split) = split_frontmatter(text) else {
        return Err(Refusal::new(
            "frontmatter",
            "missing — AUTOMATION.md starts with a --- line, then YAML, then another --- line",
        ));
    };
    let front = parse_frontmatter_yaml(&split.yaml)?;

    let body = split.body.trim();
    if body.is_empty() {
        return Err(Refusal::new(
            "body",
            "missing — the Markdown body after the frontmatter is the prompt for each run",
        ));
    }

    let unknown = sorted_unknown(&front, &FIELDS);
    if !unknown.is_empty() {
        let label = if unknown.len() == 1 {
            "unknown field"
        } else {
            "unknown fields"
        };
        return Err(Refusal::new(
            unknown.join(", "),
            format!("{label} — {DRAFT} has {}", FIELDS.join(", ")),
        ));
    }

    let name = parse_name(&front, dir_name)?;
    let description = parse_description(&front)?;

    let enabled = match get(&front, "enabled") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(other) => {
            return Err(Refusal::new(
                "enabled",
                format!("{other} is not true or false — enabled is a boolean"),
            ));
        }
    };

    let model = match get(&front, "model") {
        None => None,
        Some(v) => match v.as_str().and_then(parse_model_spec) {
            Some(m) => Some(m),
            None => {
                return Err(Refusal::new(
                    "model",
                    format!(
                        "{v} is not <provider>/<id>[:<thinking>] — for example anthropic/claude-sonnet-4-5:low"
                    ),
                ));
            }
        },
    };

    let timeout = match get(&front, "timeout") {
        None => None,
        Some(Value::Str(s)) if parse_duration(s).is_some() => Some(s.clone()),
        Some(other) => {
            return Err(Refusal::new(
                "timeout",
                format!("{other} is not a duration — use 90s, 10m, 1h"),
            ));
        }
    };

    let on = parse_on(get(&front, "on"))?;
    let hooks = parse_hooks(get(&front, "hooks"))?;
    let flow = parse_flow(get(&front, "flow"))?;

    if flow == Flow::Strict {
        let problems: Vec<String> = check_strict_flow(&split.body, split.body_line)
            .iter()
            .map(flow_problem_line)
            .collect();
        if !problems.is_empty() {
            return Err(Refusal::with_problems("flow", problems));
        }
    }

    let chart = find_flow_block(&split.body).map(|b| b.text);
    let nodes = parse_nodes(get(&front, "nodes"), flow, chart.as_deref())?;
    let automation = Automation {
        name,
        description,
        enabled,
        model,
        timeout,
        on,
        hooks,
        flow,
        nodes,
        body: body.to_string(),
    };
    Ok((automation, chart))
}

/// `nodes:`: steps of a strict chart that run through their own agent CLI or model.
fn parse_nodes(
    raw: Option<&Value>,
    flow: Flow,
    chart: Option<&str>,
) -> Result<Vec<NodeAgent>, Refusal> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Value::Map(map) = raw else {
        return Err(Refusal::new(
            "nodes",
            "must be a mapping from a step id to its agent — for example build: claude",
        ));
    };
    if flow != Flow::Strict {
        return Err(Refusal::new(
            "nodes",
            "needs flow: strict — a node runs its own section of the body, so every step needs one",
        ));
    }
    // `flow: strict` already guaranteed a chart whose steps each have a section.
    let parsed = parse_chart(chart.unwrap_or_default());
    let steps: Vec<&str> = parsed
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|id| !is_reserved_node(id) && !parsed.subgraphs.iter().any(|g| g == id))
        .collect();
    let mut out = Vec::new();
    for (id, value) in map {
        let at = format!("nodes.{id}");
        if !steps.contains(&id.as_str()) {
            let what = if is_reserved_node(id) {
                "is a hook or the body, not a step"
            } else {
                "is not a step on the flowchart"
            };
            return Err(Refusal::new(
                at,
                format!("{what} — the steps are {}", steps.join(", ")),
            ));
        }
        let mut node = NodeAgent {
            id: id.clone(),
            agent: None,
            model: None,
            timeout: None,
        };
        match value {
            Value::Str(agent) => node.agent = Some(parse_agent_name(agent, &at)?),
            Value::Map(entry) => {
                if let Some(k) = sorted_unknown(entry, &NODE_KEYS).first() {
                    return Err(Refusal::new(
                        format!("{at}.{k}"),
                        format!("unknown key — a node has {}", NODE_KEYS.join(", ")),
                    ));
                }
                if let Some(v) = get(entry, "agent") {
                    let Some(name) = v.as_str() else {
                        return Err(Refusal::new(
                            format!("{at}.agent"),
                            format!("{v} is not an agent name — for example claude or codex"),
                        ));
                    };
                    node.agent = Some(parse_agent_name(name, &format!("{at}.agent"))?);
                }
                if let Some(v) = get(entry, "model") {
                    node.model = Some(v.as_str().and_then(parse_model_spec).ok_or_else(|| {
                        Refusal::new(
                            format!("{at}.model"),
                            format!(
                                "{v} is not <provider>/<id>[:<thinking>] — for example anthropic/claude-sonnet-4-5:low"
                            ),
                        )
                    })?);
                }
                if let Some(v) = get(entry, "timeout") {
                    match v {
                        Value::Str(s) if parse_duration(s).is_some() => {
                            node.timeout = Some(s.clone());
                        }
                        other => {
                            return Err(Refusal::new(
                                format!("{at}.timeout"),
                                format!("{other} is not a duration — use 90s, 10m, 1h"),
                            ));
                        }
                    }
                }
                if node.agent.is_none() && node.model.is_none() {
                    return Err(Refusal::new(
                        at,
                        "names neither an agent nor a model — add agent: <cli> or model: <provider>/<id>",
                    ));
                }
            }
            other => {
                return Err(Refusal::new(
                    at,
                    format!("{other} is not an agent name or a mapping with agent, model, timeout"),
                ));
            }
        }
        out.push(node);
    }
    Ok(out)
}

/// An agent CLI's name: lowercase letters, digits and hyphens. Which ones run is the host's check.
fn parse_agent_name(name: &str, at: &str) -> Result<String, Refusal> {
    let ok = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(name.to_string())
    } else {
        Err(Refusal::new(
            at,
            format!(
                "{} is not an agent name — for example claude or codex",
                quote(name)
            ),
        ))
    }
}

/// Set `enabled:` with a one-line edit, keeping formatting, comments and line endings. Adds the
/// line after `name:` only when disabling, since `true` is the default.
pub fn set_enabled_in_place(text: &str, enabled: bool) -> String {
    if split_frontmatter(text).is_none() {
        return text.to_string();
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    let Some(close) = (1..lines.len()).find(|&i| lines[i].trim() == "---") else {
        return text.to_string();
    };
    let value = if enabled { "true" } else { "false" };

    for line in lines.iter_mut().take(close).skip(1) {
        let Some(rest) = line.strip_prefix("enabled") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix(':') else {
            continue;
        };
        let comment = after
            .find('#')
            .map(|i| format!(" {}", &after[i..]))
            .unwrap_or_default();
        *line = format!("enabled: {value}{comment}");
        return lines.join(eol);
    }

    if enabled {
        return text.to_string();
    }
    let at = (1..close)
        .find(|&i| {
            lines[i]
                .strip_prefix("name")
                .is_some_and(|r| r.trim_start().starts_with(':'))
        })
        .map_or(1, |i| i + 1);
    lines.insert(at, format!("enabled: {value}"));
    lines.join(eol)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(fields: &str) -> String {
        format!("---\n{fields}on:\n  manual: true\n---\n\nBody.\n")
    }

    fn refusal(text: &str, dir: &str) -> String {
        parse_automation(text, dir).unwrap_err().to_string()
    }

    #[test]
    fn splits_frontmatter() {
        let s = split_frontmatter("---\nname: x\n---\nbody\n---\nmore\n").unwrap();
        assert_eq!(s.yaml, "name: x");
        assert_eq!(s.body, "body\n---\nmore\n");
        assert_eq!(s.body_line, 4);
        let crlf = split_frontmatter("---\r\nname: x\r\n---\r\nbody\r\n").unwrap();
        assert_eq!(crlf.yaml, "name: x");
        assert_eq!(crlf.body.trim(), "body");
        assert_eq!(
            split_frontmatter("\u{feff}---\nname: x\n---\nbody\n")
                .unwrap()
                .yaml,
            "name: x"
        );
        assert!(split_frontmatter("# Just markdown\n").is_none());
        assert!(split_frontmatter("---\nname: x\nbody\n").is_none());
    }

    #[test]
    fn parses_a_full_automation() {
        let text = "---\nname: daily-briefing\ndescription: Summarise.\nmodel: anthropic/claude-sonnet-4-5:low\ntimeout: 10m\non:\n  schedule:\n    cron: \"0 7 * * MON-FRI\"\n    timezone: America/Los_Angeles\n    catch-up: once\n  manual: true\nhooks:\n  pre:\n    - script: scripts/overnight_counts.js\n    - prompt: Skip if empty.\n  post:\n    - script: scripts/mark_run.js\n---\n\n# Daily briefing\n\nWrite it.\n";
        let (a, chart) = parse_automation(text, "daily-briefing").unwrap();
        assert_eq!(a.name, "daily-briefing");
        assert!(a.enabled);
        assert_eq!(
            a.model,
            Some(ModelSpec {
                provider: "anthropic".into(),
                id: "claude-sonnet-4-5".into(),
                thinking: Some("low".into())
            })
        );
        assert_eq!(a.timeout.as_deref(), Some("10m"));
        assert!(a.on.manual);
        assert_eq!(
            a.on.schedule,
            Some(Schedule {
                cron: "0 7 * * MON-FRI".into(),
                timezone: Some("America/Los_Angeles".into()),
                catch_up: CatchUp::Once
            })
        );
        assert_eq!(
            a.hooks.pre,
            [
                Hook::Script("scripts/overnight_counts.js".into()),
                Hook::Prompt("Skip if empty.".into())
            ]
        );
        assert_eq!(a.hooks.post, [Hook::Script("scripts/mark_run.js".into())]);
        assert_eq!(a.flow, Flow::Loose);
        assert!(a.body.starts_with("# Daily briefing"));
        assert_eq!(chart, None);
    }

    #[test]
    fn a_bare_cron_is_the_mapping_with_only_cron() {
        let bare = parse_schedule(&Value::Str("0 9 * * MON".into())).unwrap();
        let mapped = parse_schedule(&Value::Map(vec![(
            "cron".into(),
            Value::Str("0 9 * * MON".into()),
        )]))
        .unwrap();
        assert_eq!(bare, mapped);
        assert_eq!(bare.catch_up, CatchUp::Skip);
        assert_eq!(bare.timezone, None);
    }

    #[test]
    fn defaults_and_yaml_1_2() {
        let (a, _) = parse_automation(&file("name: x\ndescription: d\n"), "x").unwrap();
        assert!(a.enabled);
        assert!(a.on.manual);
        assert_eq!(a.hooks, Hooks::default());
        let (off, _) =
            parse_automation(&file("name: x\ndescription: d\nenabled: false\n"), "x").unwrap();
        assert!(!off.enabled);
        let (nulls, _) = parse_automation(
            &file("name: x\ndescription: d\nmodel: null\ntimeout:\n"),
            "x",
        )
        .unwrap();
        assert_eq!(nulls.model, None);
        assert_eq!(nulls.timeout, None);
    }

    #[test]
    fn field_limits() {
        let long = "a".repeat(65);
        assert_eq!(
            refusal(&file(&format!("name: {long}\ndescription: d\n")), &long),
            "name: is 65 characters — the maximum is 64"
        );
        let d = "d".repeat(1025);
        assert_eq!(
            refusal(&file(&format!("name: x\ndescription: {d}\n")), "x"),
            "description: is 1025 characters — the maximum is 1024"
        );
        let at = "d".repeat(1024);
        let (a, _) =
            parse_automation(&file(&format!("name: x\ndescription: {at}\n")), "x").unwrap();
        assert_eq!(a.description.len(), 1024);
    }

    #[test]
    fn duplicate_keys_and_unquoted_crons_are_frontmatter_errors() {
        let dup = refusal(
            "---\nname: x\ndescription: one\ndescription: two\non:\n  manual: true\n---\n\nBody.\n",
            "x",
        );
        assert!(
            dup.starts_with("frontmatter: Map keys must be unique"),
            "{dup}"
        );
        assert!(dup.contains("line 3, column 1"), "{dup}");
        let cron = refusal(
            "---\nname: x\ndescription: d\non:\n  schedule:\n    cron: */15 * * * *\n---\n\nBody.\n",
            "x",
        );
        assert!(cron.starts_with("frontmatter: "), "{cron}");
        assert!(cron.ends_with(" — quote the cron expression"), "{cron}");
        assert!(!cron.contains('\n'));
    }

    #[test]
    fn flow_field() {
        const CHART: &str = "```mermaid\nflowchart TD\n  read[\"Read the change\"] --> post[\"Post the comment\"]\n```";
        let strict_body = format!(
            "# Review\n\n{CHART}\n\n## Read the change {{#read}}\n\nRead it.\n\n## Post the comment {{#post}}\n\nPost it."
        );
        let with = |flow: &str, body: &str| {
            format!("---\nname: x\ndescription: d\non:\n  manual: true\n{flow}---\n\n{body}\n")
        };
        let (loose, chart) = parse_automation(&with("", &strict_body), "x").unwrap();
        assert_eq!(loose.flow, Flow::Loose);
        assert_eq!(
            chart.as_deref(),
            Some("flowchart TD\n  read[\"Read the change\"] --> post[\"Post the comment\"]\n")
        );
        assert_eq!(
            parse_automation(&with("flow: loose\n", "## Anything goes\n\nNo chart."), "x")
                .unwrap()
                .0
                .body,
            "## Anything goes\n\nNo chart."
        );
        assert_eq!(
            parse_automation(&with("flow: strict\n", &strict_body), "x")
                .unwrap()
                .0
                .flow,
            Flow::Strict
        );
        assert_eq!(
            refusal(&with("flow: tight\n", &strict_body), "x"),
            "flow: \"tight\" is not loose or strict"
        );
        assert_eq!(
            refusal(&with("flow: true\n", &strict_body), "x"),
            "flow: true is not loose or strict"
        );
        let bad = format!("# Review\n\n{CHART}\n\n## Read it {{#read}}\n\n## Notes");
        let e = parse_automation(&with("flow: strict\n", &bad), "x").unwrap_err();
        assert_eq!(e.field, "flow");
        assert_eq!(
            e.problems,
            [
                "line 16: heading \"Read it\" does not match node `read`'s label \"Read the change\" — make them the same text",
                "line 18: \"## Notes\" is not a step — end it with {#id} for its node on the chart, or make it a ### under a step",
                "node `post` (\"Post the comment\") has no section — add `## Post the comment {#post}`",
            ]
        );
        assert_eq!(e.to_string(), format!("flow: {}", e.problems.join("; ")));
        assert_eq!(
            refusal(&with("flow: strict\n", "Just do it."), "x"),
            "flow: the body has no flowchart — add a ```mermaid block that starts with flowchart TD and has a node per step, or remove flow: strict"
        );
        assert_eq!(
            refusal(&with("mode: strict\n", &strict_body), "x"),
            "mode: unknown field — Draft 0.2.0 has name, description, enabled, model, timeout, on, hooks, flow, nodes"
        );
    }

    #[test]
    fn names() {
        for ok in ["a", "daily-briefing", "x1", "a-b-c", "123"] {
            assert!(is_valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "a--b", "A", "a_b", "a b", "a.b"] {
            assert!(!is_valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn model_specs() {
        for ok in [
            "anthropic/claude-sonnet-4-5",
            "anthropic/claude-sonnet-4-5:low",
            "openai/gpt-5.2",
            "google/gemini-2.5-pro:high",
            "amazon-bedrock/amazon.nova-2-lite-v1:0",
            "openrouter/some-model:exacto",
        ] {
            assert!(parse_model_spec(ok).is_some(), "{ok}");
        }
        for bad in [
            "claude-sonnet-4-5",
            "anthropic/",
            "/model",
            "anthropic/a/b",
            "a b/c",
        ] {
            assert!(parse_model_spec(bad).is_none(), "{bad}");
        }
        let exacto = parse_model_spec("openrouter/some-model:exacto").unwrap();
        assert_eq!(exacto.id, "some-model:exacto");
        assert_eq!(exacto.thinking, None);
        let nova = parse_model_spec("amazon-bedrock/amazon.nova-2-lite-v1:0").unwrap();
        assert_eq!(nova.id, "amazon.nova-2-lite-v1:0");
        let low = parse_model_spec("anthropic/claude-sonnet-4-5:low").unwrap();
        assert_eq!(low.display(), "anthropic/claude-sonnet-4-5:low");
    }

    #[test]
    fn script_paths() {
        let s =
            |p: &str| check_script_path(Some(&Value::Str(p.into())), "x").map_err(|e| e.message);
        assert!(s("scripts/a.sh").is_ok());
        assert!(s("./a.sh").is_ok());
        assert!(s("a/../b.sh").is_ok());
        assert!(s("/etc/a.sh").unwrap_err().contains("is absolute"));
        assert!(s("C:\\a.ps1").unwrap_err().contains("is absolute"));
        assert!(s("c:/a.ps1").unwrap_err().contains("is absolute"));
        assert!(s("\\\\server\\a.ps1").unwrap_err().contains("is absolute"));
        assert!(s("../a.sh").unwrap_err().contains("leaves"));
        assert!(s("a/../../b.sh").unwrap_err().contains("leaves"));
        assert!(s("..\\a.sh").unwrap_err().contains("leaves"));
        assert!(s("  ").unwrap_err().contains("is empty"));
    }

    #[test]
    fn set_enabled_in_place_edits_one_line() {
        let with =
            "---\nname: x\nenabled: true\ndescription: d\non:\n  manual: true\n---\n\nBody.\n";
        let without = "---\nname: x\ndescription: d\non:\n  manual: true\n---\n\nBody.\n";
        assert_eq!(
            set_enabled_in_place(with, false),
            with.replace("enabled: true", "enabled: false")
        );
        let commented = "---\nname: x\nenabled: true  # paused\ndescription: d\non:\n  manual: true\n---\n\nBody.\n";
        assert!(set_enabled_in_place(commented, false).contains("enabled: false # paused"));
        let added = set_enabled_in_place(without, false);
        assert_eq!(
            added.split('\n').take(3).collect::<Vec<_>>(),
            ["---", "name: x", "enabled: false"]
        );
        assert_eq!(set_enabled_in_place(without, true), without);
        let crlf = with.replace('\n', "\r\n");
        let out = set_enabled_in_place(&crlf, false);
        assert!(out.contains("enabled: false\r\n"));
        assert!(!out.replace("\r\n", "").contains('\n'));
        let in_body =
            "---\nname: x\ndescription: d\non:\n  manual: true\n---\n\nenabled: true in the body\n";
        let out = set_enabled_in_place(in_body, false);
        assert!(out.contains("enabled: true in the body"));
        assert_eq!(out.split('\n').nth(2), Some("enabled: false"));
        assert_eq!(set_enabled_in_place("# none\n", false), "# none\n");
        let off = set_enabled_in_place(with, false);
        assert!(!parse_automation(&off, "x").unwrap().0.enabled);
        assert!(
            parse_automation(&set_enabled_in_place(&off, true), "x")
                .unwrap()
                .0
                .enabled
        );
    }
}

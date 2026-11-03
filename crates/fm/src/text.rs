//! The plain-text output of the CLI: the listing, the consent preview, `check`, `log`, the live
//! view of `fm run`.

use std::path::Path;

use chrono::{DateTime, Local, Utc};
use fm_model::{
    AutomationState, ConsentPreview, HookStatus, LiveEvent, Outcome, RunRecord, Status, Trigger,
};
use fm_state::time::parse_iso;

/// `~/…` for a path under the home directory.
pub fn tildify(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => Path::new("~").join(rest).display().to_string(),
        Err(_) => p.display().to_string(),
    }
}

pub fn state_word(s: AutomationState) -> &'static str {
    match s {
        AutomationState::Invalid => "invalid",
        AutomationState::Disabled => "disabled",
        AutomationState::Off => "off",
        AutomationState::Armed => "armed",
    }
}

pub fn outcome_word(o: Outcome) -> &'static str {
    match o {
        Outcome::Ok => "ok",
        Outcome::Skipped => "skipped",
        Outcome::Failed => "failed",
        Outcome::Missed => "missed",
        Outcome::Aborted => "aborted",
    }
}

pub fn trigger_word(t: Trigger) -> &'static str {
    match t {
        Trigger::Schedule => "schedule",
        Trigger::Manual => "manual",
        Trigger::Watch => "watch",
    }
}

/// `45s`, `9m12s`, `1h`.
pub fn span(ms: i64) -> String {
    fm_spec::format_duration(std::time::Duration::from_millis(ms.max(0).unsigned_abs()))
}

fn local_clock(iso: &str, fmt: &str) -> String {
    parse_iso(iso)
        .map(|t| t.with_timezone(&Local).format(fmt).to_string())
        .unwrap_or_else(|| iso.to_string())
}

/// The SCHEDULE column: the cron, the watch interval, or both.
fn schedule_text(s: &Status) -> String {
    let Some(a) = &s.automation else {
        return "—".into();
    };
    let mut parts = Vec::new();
    if let Some(sc) = &a.on.schedule {
        parts.push(sc.cron.clone());
    }
    if let Some(w) = &a.on.watch {
        parts.push(format!("watch {}", w.every));
    }
    if parts.is_empty() {
        if a.on.manual {
            return "manual".into();
        }
        return "—".into();
    }
    parts.join(" + ")
}

fn next_text(s: &Status, now: DateTime<Utc>) -> String {
    if let Some(r) = &s.running {
        let started = parse_iso(&r.started_at).unwrap_or(now);
        return format!("running {}", span((now - started).num_milliseconds()));
    }
    if s.state != AutomationState::Armed {
        return "—".into();
    }
    if let Some(n) = &s.next_fire {
        return local_clock(n, "%a %H:%M");
    }
    if s.watch_paused_at.is_some() {
        return "watch paused".into();
    }
    if let Some(c) = &s.next_check {
        return match parse_iso(c) {
            Some(t) if t <= now => "check now".into(),
            _ => format!("check {}", local_clock(c, "%H:%M")),
        };
    }
    "—".into()
}

const STATE_ORDER: [AutomationState; 4] = [
    AutomationState::Armed,
    AutomationState::Off,
    AutomationState::Disabled,
    AutomationState::Invalid,
];

fn order(s: &Status) -> usize {
    STATE_ORDER.iter().position(|x| *x == s.state).unwrap_or(4)
}

/// The listing, grouped by scope root: `USER · ~/.agents/automations`, then each repository.
pub fn listing(statuses: &[Status], home: &Path, now: DateTime<Utc>) -> Vec<String> {
    struct Row {
        name: String,
        state: String,
        schedule: String,
        next: String,
        agent: String,
        notes: Vec<String>,
    }
    let mut groups: Vec<(String, Vec<&Status>)> = Vec::new();
    let user_title = format!(
        "USER · {}",
        tildify(&home.join(".agents").join("automations"), home)
    );
    groups.push((user_title, Vec::new()));
    for s in statuses {
        let title = match s.scope {
            fm_model::Scope::User => groups[0].0.clone(),
            fm_model::Scope::Repository => {
                let root = s.dir.parent().unwrap_or(&s.dir);
                format!("REPOSITORY · {}", tildify(root, home))
            }
        };
        match groups.iter_mut().find(|(t, _)| *t == title) {
            Some((_, list)) => list.push(s),
            None => groups.push((title, vec![s])),
        }
    }
    let mut rendered: Vec<(String, Vec<Row>)> = Vec::new();
    for (title, mut list) in groups {
        list.sort_by(|a, b| order(a).cmp(&order(b)).then(a.name.cmp(&b.name)));
        let rows = list
            .into_iter()
            .map(|s| {
                let mut notes = Vec::new();
                if matches!(s.state, AutomationState::Invalid | AutomationState::Off)
                    && let Some(r) = &s.reason
                {
                    notes.push(format!("↳ {r}"));
                }
                if s.also_armed_in_pa {
                    notes.push(format!(
                        "! also armed in pa — it would run twice; pa disarm {}",
                        s.name
                    ));
                }
                Row {
                    name: format!("{}{}", if s.flow { "◇ " } else { "" }, s.name),
                    state: state_word(s.state).into(),
                    schedule: schedule_text(s),
                    next: next_text(s, now),
                    agent: s.agent.clone().unwrap_or_else(|| "—".into()),
                    notes,
                }
            })
            .collect();
        rendered.push((title, rows));
    }

    let all = rendered.iter().flat_map(|(_, r)| r.iter());
    let w = |f: &dyn Fn(&Row) -> usize, min: usize| all.clone().map(f).max().unwrap_or(0).max(min);
    let wn = w(&|r| r.name.chars().count(), 16);
    let ws = w(&|r| r.state.len(), 8);
    let wc = w(&|r| r.schedule.chars().count(), 12);
    let wx = w(&|r| r.next.chars().count(), 10);
    let pad = |s: &str, n: usize| format!("{s}{}", " ".repeat(n.saturating_sub(s.chars().count())));

    let mut out = vec![
        format!(
            "  {}  {}  {}  {}  AGENT",
            pad("NAME", wn),
            pad("STATE", ws),
            pad("SCHEDULE", wc),
            pad("NEXT", wx)
        )
        .trim_end()
        .to_string(),
    ];
    for (title, rows) in rendered {
        out.push(format!("  {title}"));
        if rows.is_empty() {
            out.push("  none".into());
            continue;
        }
        for r in rows {
            out.push(
                format!(
                    "  {}  {}  {}  {}  {}",
                    pad(&r.name, wn),
                    pad(&r.state, ws),
                    pad(&r.schedule, wc),
                    pad(&r.next, wx),
                    r.agent
                )
                .trim_end()
                .to_string(),
            );
            for n in r.notes {
                out.push(format!("    {n}"));
            }
        }
    }
    out
}

/// The consent preview as lines: warnings first, then what would be allowed.
pub fn preview_lines(
    p: &ConsentPreview,
    description: &str,
    title: &str,
    root: &str,
) -> Vec<String> {
    let mut out = vec![format!("{title} {}  from {root}", p.name)];
    for w in &p.warnings {
        out.push(format!("  ! {w}"));
    }
    let row = |label: &str, value: &str| format!("  {label:<12}  {value}");
    out.push(row("description", description));
    for t in &p.triggers {
        let (k, v) = t.split_once(": ").unwrap_or(("trigger", t));
        out.push(row(k, v));
    }
    out.push(row(
        "agent",
        &p.agent
            .map(|c| format!("{c} ({})", c.title()))
            .unwrap_or_else(|| "none chosen".into()),
    ));
    if let Some(cmd) = &p.agent_command {
        out.push(row("command", cmd));
    }
    out.push(row("model", &p.model));
    out.push(row("timeout", &p.timeout));
    for (phase, what) in &p.hooks {
        out.push(row(phase, what));
    }
    if let Some(w) = &p.watch {
        out.push(row("watch runs", w));
    }
    out.push(row("pinned to", &p.files.join(", ")));
    if let Some(e) = &p.expires {
        out.push(row("expires", e));
    }
    out.push(String::new());
    out.push(
        "Arming lets it run on this machine with your credentials. Scripts run with your permissions."
            .into(),
    );
    out
}

/// What a run did, in one line: its reason, error, or the first line of its answer.
fn summary(rec: &RunRecord) -> String {
    let text = rec
        .reason
        .clone()
        .or_else(|| rec.error.as_ref().map(|e| e.message.clone()))
        .or_else(|| rec.output.as_ref().map(|o| o.text.clone()))
        .unwrap_or_default();
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line: String = line.chars().take(96).collect();
    let mut bits = vec![trigger_word(rec.trigger).to_string()];
    if let Some(a) = &rec.agent {
        bits.push(a.clone());
    }
    if rec.late {
        bits.push("late".into());
    }
    if line.is_empty() {
        bits.join(" · ")
    } else {
        format!("{} · {line}", bits.join(" · "))
    }
}

/// One record: when, outcome, how long, what happened; then its message and notes with links.
pub fn log_lines(rec: &RunRecord) -> Vec<String> {
    let at = rec
        .ended_at
        .as_deref()
        .or(rec.started_at.as_deref())
        .unwrap_or(&rec.recorded_at);
    let took = match (
        rec.started_at.as_deref().and_then(parse_iso),
        rec.ended_at.as_deref().and_then(parse_iso),
    ) {
        (Some(a), Some(b)) => span((b - a).num_milliseconds()),
        _ => "—".into(),
    };
    let mut out = vec![
        format!(
            "{:<16}  {:<8}  {:<7}  {}",
            local_clock(at, "%Y-%m-%d %H:%M"),
            outcome_word(rec.outcome),
            took,
            summary(rec)
        )
        .trim_end()
        .to_string(),
    ];
    let pad = " ".repeat(18);
    let items = rec
        .message
        .iter()
        .map(|m| ("message", m))
        .chain(rec.notes.iter().map(|n| ("note", n)));
    for (word, item) in items {
        let text = item.text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push(format!("{pad}{word}: {text}"));
        for l in &item.links {
            match &l.label {
                Some(label) => out.push(format!("{pad}      {label}: {}", l.url)),
                None => out.push(format!("{pad}      {}", l.url)),
            }
        }
    }
    out
}

/// The lines `fm run` prints for `events[from..]`, the answer once: a `Final` that repeats the
/// message before it was already printed as that message.
pub fn live_lines(events: &[LiveEvent], from: usize) -> Vec<String> {
    (from..events.len())
        .filter(|&i| fm_model::repeated_message(events, i).is_none())
        .filter_map(|i| live_line(&events[i]))
        .collect()
}

/// A live event as `fm run` prints it, or nothing for text still arriving.
pub fn live_line(e: &LiveEvent) -> Option<String> {
    match e {
        LiveEvent::Session { id, .. } => Some(format!("· session {id}")),
        LiveEvent::Partial { .. } | LiveEvent::Thinking { .. } => None,
        LiveEvent::Tool { name, summary, .. } => Some(format!("· {name} {summary}")),
        LiveEvent::Message { text, .. } => Some(text.clone()),
        LiveEvent::Final { text, .. } => Some(text.clone()),
        LiveEvent::Error { message, .. } => Some(format!("! {message}")),
        LiveEvent::Hook {
            phase,
            index,
            source,
            status,
            ..
        } => {
            let phase = match phase {
                fm_model::Phase::Pre => "pre",
                fm_model::Phase::Post => "post",
            };
            let how = match status {
                None => "started",
                Some(HookStatus::Ok) => "ok",
                Some(HookStatus::Skipped) => "skipped",
                Some(HookStatus::Failed) => "failed",
            };
            Some(format!("· {phase}[{index}] {source} {how}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn tildify_home() {
        let home = PathBuf::from("home").join("me");
        assert_eq!(tildify(&home, &home), "~");
        let p = home.join(".agents");
        assert_eq!(
            tildify(&p, &home),
            Path::new("~").join(".agents").display().to_string()
        );
        assert_eq!(tildify(Path::new("elsewhere"), &home), "elsewhere");
    }

    #[test]
    fn live_lines_skip_partials() {
        assert!(
            live_line(&LiveEvent::Partial {
                text: "x".into(),
                ts: 0
            })
            .is_none()
        );
        assert_eq!(
            live_line(&LiveEvent::Tool {
                name: "bash".into(),
                summary: "ls".into(),
                ts: 0
            })
            .as_deref(),
            Some("· bash ls")
        );
    }

    #[test]
    fn live_lines_print_the_answer_once() {
        let message = |text: &str| LiveEvent::Message {
            text: text.into(),
            ts: 0,
        };
        let answer = |text: &str| LiveEvent::Final {
            text: text.into(),
            ts: 0,
        };
        let events = [
            message("Paris."),
            LiveEvent::Session {
                id: "s".into(),
                ts: 0,
            },
            answer("Paris."),
        ];
        assert_eq!(live_lines(&events, 0), ["Paris.", "· session s"]);
        // Printed a poll at a time, the same.
        assert_eq!(live_lines(&events, 2), Vec::<String>::new());
        // An answer that is not the last message is printed.
        let events = [message("Looking."), answer("Paris.")];
        assert_eq!(live_lines(&events, 0), ["Looking.", "Paris."]);
    }

    #[test]
    fn empty_listing_has_a_user_group() {
        let lines = listing(&[], Path::new("h"), Utc::now());
        assert!(lines[0].starts_with("  NAME"));
        assert!(lines[1].contains("USER · "));
        assert_eq!(lines[2], "  none");
    }
}

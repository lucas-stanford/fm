//! `fm status --format markdown`: where a run is, for a page that reads Markdown (a GitHub issue or
//! pull request). Its flowchart as a ```mermaid block coloured by status, a table of its steps
//! with how long each took, and everything it reported in order: each update, each decision (an
//! arrow taken, with why), what it published and its message.

use chrono::{DateTime, Utc};
use fm_model::{HistoryEntry, RunLink, StepStatus};
use fm_state::progress::{ProgressNode, ProgressSnapshot};

/// One timeline entry's text is cut here; a page is long enough without an essay per line.
const ENTRY_MAX: usize = 1500;
/// GitHub refuses a comment over 65536 characters; the oldest timeline entries go first.
const PAGE_MAX: usize = 60_000;

fn icon(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Pending => "⬜",
        StepStatus::Active => "⏳",
        StepStatus::Waiting => "⏸️",
        StepStatus::Done => "✅",
        StepStatus::Failed => "❌",
        StepStatus::Skipped => "➖",
        StepStatus::Interrupted => "⚠️",
    }
}

fn parse_at(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// `14:05 UTC` from an RFC 3339 time; the text as it is when it is not one.
fn clock(at: &str) -> String {
    parse_at(at).map_or_else(|| at.to_string(), |t| t.format("%H:%M UTC").to_string())
}

fn took(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        return format!("{s}s");
    }
    let m = (s + 30) / 60;
    if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h {}m", m / 60, m % 60)
    }
}

/// Text from an agent, on one line and bounded. It stays Markdown: links and code are the point.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// A label in a table cell: a `|` would end the cell.
fn cell(text: &str) -> String {
    one_line(text, 200).replace('|', "\\|")
}

fn links(list: &[RunLink]) -> String {
    list.iter()
        .map(|l| format!("[{}]({})", l.label.as_deref().unwrap_or("link"), l.url))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn with_links(text: &str, list: &[RunLink]) -> String {
    let l = links(list);
    if l.is_empty() {
        one_line(text, ENTRY_MAX)
    } else {
        format!("{} — {l}", one_line(text, ENTRY_MAX))
    }
}

fn time_cell(n: &ProgressNode, passes: usize, now: DateTime<Utc>) -> String {
    let start = n.started_at.as_deref().and_then(parse_at);
    let end = n.ended_at.as_deref().and_then(parse_at);
    let mut out = match (n.status, start, end) {
        (StepStatus::Skipped, ..) => "skipped".to_string(),
        (StepStatus::Active | StepStatus::Waiting, Some(s), _) => {
            format!("{} so far", took((now - s).num_seconds()))
        }
        (_, Some(s), Some(e)) => took((e - s).num_seconds()),
        _ => String::new(),
    };
    if passes > 1 {
        out.push_str(&format!(" ({passes} passes)"));
    }
    out
}

/// The whole page.
pub fn status_markdown(s: &ProgressSnapshot, now: DateTime<Utc>) -> String {
    let label = |id: &str| {
        s.node(id)
            .map_or_else(|| id.to_string(), |n| n.label.clone())
    };
    let state = if s.closed {
        let outcome = s.history.iter().rev().find_map(|e| match e {
            HistoryEntry::Close { outcome, .. } => Some(outcome.as_str()),
            _ => None,
        });
        format!(
            "ended{}",
            outcome.map(|o| format!(": {o}")).unwrap_or_default()
        )
    } else {
        match s.nodes.iter().find(|n| n.status == StepStatus::Active) {
            Some(n) => format!("now: **{}**", cell(&n.label)),
            None => "running".to_string(),
        }
    };

    let mut head = vec![
        format!("**{}** · run `{}` · {state}", cell(&s.name), s.run),
        String::new(),
    ];

    // The same state `fm render --state` reads. A chart fm cannot draw leaves the block out.
    let nodes: serde_json::Map<String, serde_json::Value> = s
        .nodes
        .iter()
        .map(|n| {
            (
                n.id.clone(),
                serde_json::to_value(n.status).unwrap_or_default(),
            )
        })
        .collect();
    let chart_state = serde_json::json!({ "nodes": nodes, "taken": s.taken });
    if let Ok(block) = fm_render::render_markdown(&s.chart, &chart_state.to_string()) {
        head.push(block.trim_end().to_string());
        head.push(String::new());
    }

    head.push("| | Step | Time |".into());
    head.push("|:-:|---|--:|".into());
    for n in &s.nodes {
        let passes = s
            .history
            .iter()
            .filter(|e| matches!(e, HistoryEntry::Next { to, .. } if *to == n.id))
            .count();
        let name = if n.status == StepStatus::Active {
            format!("**{}**", cell(&n.label))
        } else {
            cell(&n.label)
        };
        head.push(format!(
            "| {} | {name} | {} |",
            icon(n.status),
            time_cell(n, passes, now)
        ));
    }

    let timeline: Vec<String> = s
        .history
        .iter()
        .map(|e| match e {
            HistoryEntry::Next { at, from, to, text } => {
                let why = text
                    .as_deref()
                    .map(|t| format!(": {}", one_line(t, ENTRY_MAX)))
                    .unwrap_or_default();
                format!(
                    "- {} · ➜ **{} → {}**{why}",
                    clock(at),
                    cell(&label(from)),
                    cell(&label(to))
                )
            }
            HistoryEntry::Step {
                at,
                node,
                status,
                text,
            } => {
                let t = text
                    .as_deref()
                    .map(|t| format!(" — {}", one_line(t, ENTRY_MAX)))
                    .unwrap_or_default();
                format!(
                    "- {} · {} {}: {}{t}",
                    clock(at),
                    icon(*status),
                    cell(&label(node)),
                    status_word(*status)
                )
            }
            HistoryEntry::Say { at, node, text } => {
                format!(
                    "- {} · {}: {}",
                    clock(at),
                    cell(&label(node)),
                    one_line(text, ENTRY_MAX)
                )
            }
            HistoryEntry::Note { at, text, links } => {
                format!("- {} · 📝 {}", clock(at), with_links(text, links))
            }
            HistoryEntry::Message { at, text, links } => {
                format!("- {} · 💬 {}", clock(at), with_links(text, links))
            }
            HistoryEntry::Close { at, outcome } => format!("- {} · 🏁 ended: {outcome}", clock(at)),
        })
        .collect();

    let mut tail = Vec::new();
    if let Some(m) = &s.message {
        tail.push(String::new());
        tail.push(format!("**Message:** {}", with_links(&m.text, &m.links)));
    }

    let page = |dropped: usize| {
        let mut lines = head.clone();
        if !timeline.is_empty() {
            lines.push(String::new());
            lines.push("**Steps and decisions**".into());
            lines.push(String::new());
            if dropped > 0 {
                lines.push(format!(
                    "- … {dropped} earlier entries are in `fm status --json`"
                ));
            }
            lines.extend(timeline[dropped..].iter().cloned());
        }
        lines.extend(tail.iter().cloned());
        lines.join("\n") + "\n"
    };
    let mut dropped = 0;
    let mut out = page(0);
    while out.len() > PAGE_MAX && dropped < timeline.len() {
        dropped += 1;
        out = page(dropped);
    }
    out
}

fn status_word(s: StepStatus) -> &'static str {
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

//! Pages that ask something: the consent preview, stop's question, the agent picker, the links.

use fm_model::{ConsentPreview, RunLink};
use ratatui::text::{Line, Span};

use super::{Page, colored, hint, muted, row, scroll_block, window_block, wrapped};
use crate::app::{AgentChoice, App, Confirm};
use crate::text::{ellipsize, one_line, pad, wrap, wrap_command};
use crate::theme::UiColor;
use crate::words::{parse, span_long};

const LABEL_W: usize = 10;

/// Everything arming allows, warnings first, the agent command whole. Only `y` arms.
pub fn consent(
    app: &App,
    key: &str,
    preview: &Result<ConsentPreview, String>,
    scroll: usize,
    w: usize,
    rows: usize,
) -> Page {
    let t = &app.theme;
    let name = app.status(key).map(|s| s.name.clone()).unwrap_or_default();
    let p = match preview {
        Ok(p) => p,
        Err(e) => {
            return Page {
                title: Line::from(Span::styled(format!("Arm {name}"), t.bold())),
                right: None,
                body: wrapped(
                    &format!("It cannot be armed: {}", one_line(e)),
                    2,
                    w.saturating_sub(2),
                    t.fg(UiColor::Error),
                ),
                hints: vec![],
                pinned: vec!["esc back".into()],
            };
        }
    };
    let value_w = w.saturating_sub(2 + LABEL_W).max(10);
    let mut lines: Vec<Line<'static>> = vec![];
    for warning in &p.warnings {
        let text = format!("{} {}", app.glyphs.problem, one_line(warning));
        for (i, l) in wrap(&text, w.saturating_sub(4)).into_iter().enumerate() {
            let indent = if i == 0 { "  " } else { "    " };
            lines.push(Line::from(Span::styled(
                format!("{indent}{l}"),
                t.fg(UiColor::Warning).patch(t.bold()),
            )));
        }
    }
    if !p.warnings.is_empty() {
        lines.push(Line::default());
    }
    let mut add = |label: &str, text: &str, hard: bool, color: UiColor| {
        let text = one_line(text);
        let parts = if hard {
            wrap_command(&text, value_w)
        } else {
            let v = wrap(&text, value_w);
            if v.is_empty() { vec![String::new()] } else { v }
        };
        for (i, l) in parts.into_iter().enumerate() {
            lines.push(row(
                if i == 0 { label } else { "" },
                LABEL_W,
                vec![colored(l, color, app)],
                app,
            ));
        }
    };
    for trig in &p.triggers {
        match trig.split_once(": ") {
            Some((label, value)) => add(label, value, false, UiColor::Text),
            None => add("trigger", trig, false, UiColor::Text),
        }
    }
    if let Some(watch) = &p.watch {
        add("watch", watch, true, UiColor::Text);
    }
    let agent = match p.agent {
        Some(cli) if cli.title() == cli.as_str() => cli.as_str().to_string(),
        Some(cli) => format!("{} ({})", cli.as_str(), cli.title()),
        None => "no agent CLI found — choose one with a".into(),
    };
    add(
        "agent",
        &agent,
        false,
        if p.agent.is_some() {
            UiColor::Text
        } else {
            UiColor::Warning
        },
    );
    if let Some(cmd) = &p.agent_command {
        add("command", cmd, true, UiColor::Accent);
    }
    add("model", &p.model, false, UiColor::Text);
    add("timeout", &p.timeout, false, UiColor::Text);
    for (phase, what) in &p.hooks {
        add(phase, what, true, UiColor::Text);
    }
    for (i, f) in p.files.iter().enumerate() {
        add(if i == 0 { "files" } else { "" }, f, true, UiColor::Text);
    }
    if let Some(e) = &p.expires {
        add("expires", e, false, UiColor::Text);
    }
    lines.push(Line::default());
    lines.extend(wrapped(
        "Arming lets it run on this machine, with your credentials. Scripts run with your permissions.",
        2,
        w.saturating_sub(2),
        t.fg(UiColor::Muted),
    ));
    let overflow = lines.len() > rows;
    let body = scroll_block(lines, scroll, rows, app);
    let mut hints = vec![hint("y arm it", 1)];
    if overflow {
        hints.push(hint("↑↓ scroll", 2));
    }
    Page {
        title: Line::from(Span::styled(format!("Arm {}", p.name), t.bold())),
        right: Some(Line::from(muted("consent preview", app))),
        body,
        hints,
        pinned: vec!["esc leave it off".into()],
    }
}

pub fn confirm(app: &App, key: &str, what: Confirm, w: usize) -> Page {
    let t = &app.theme;
    let s = app.status(key);
    let name = s.map(|s| s.name.clone()).unwrap_or_default();
    match what {
        Confirm::Stop => {
            let elapsed = s
                .and_then(|s| s.running.as_ref())
                .and_then(|r| parse(&r.started_at))
                .map(|at| span_long((app.now - at).num_milliseconds()));
            let mut text = String::new();
            if let Some(e) = elapsed {
                text.push_str(&format!("It has been running for {e}. "));
            }
            text.push_str("Its post hooks still run, and the run is recorded as aborted.");
            Page {
                title: Line::from(Span::styled(format!("Stop {name}'s run?"), t.bold())),
                right: None,
                body: wrapped(&text, 2, w.saturating_sub(2), t.fg(UiColor::Text)),
                hints: vec![hint("y stop it", 1)],
                pinned: vec!["esc keep it running".into()],
            }
        }
    }
}

pub fn agents(
    app: &App,
    key: &str,
    items: &[AgentChoice],
    index: usize,
    w: usize,
    rows: usize,
) -> Page {
    let t = &app.theme;
    let s = app.status(key);
    let name = s.map(|s| s.name.clone()).unwrap_or_default();
    let current = s.and_then(|s| s.agent.clone());
    let ell = app.glyphs.ellipsis;
    let mut lines = vec![];
    for (i, item) in items.iter().enumerate() {
        let on = i == index;
        let cursor = if on { app.glyphs.cursor } else { " " };
        let style = if on { t.bold() } else { t.fg(UiColor::Text) };
        let mut spans = vec![Span::styled(format!("  {cursor} "), t.bold())];
        match item.cli {
            None => spans.push(Span::styled("use default", style)),
            Some(cli) => {
                spans.push(Span::styled(pad(cli.as_str(), 10), style));
                let title = if cli.title() == cli.as_str() {
                    ""
                } else {
                    cli.title()
                };
                spans.push(Span::styled(pad(title, 20), style));
                spans.push(muted(
                    item.version
                        .clone()
                        .unwrap_or_else(|| "version unknown".into()),
                    app,
                ));
                if current.as_deref() == Some(cli.as_str()) {
                    spans.push(colored("  now", UiColor::Accent, app));
                }
            }
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());
    lines.push(Line::from(muted(
        ellipsize(
            "  Only the agent CLIs found on this machine are listed.",
            w,
            ell,
        ),
        app,
    )));
    Page {
        title: Line::from(Span::styled(format!("Agent for {name}"), t.bold())),
        right: current.map(|c| Line::from(muted(format!("now {c}"), app))),
        body: window_block(lines, index, index, rows, app),
        hints: vec![hint("enter use it", 1), hint("↑↓ choose", 2)],
        pinned: vec!["esc cancel".into()],
    }
}

pub fn links(app: &App, links: &[RunLink], index: usize, w: usize, rows: usize) -> Page {
    let t = &app.theme;
    let ell = app.glyphs.ellipsis;
    let room = w.saturating_sub(8);
    let label_w = links
        .iter()
        .map(|l| crate::text::width(&l.label.as_deref().map(one_line).unwrap_or_default()))
        .max()
        .unwrap_or(0)
        .min(room / 2);
    let lines = links
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let on = i == index;
            let cursor = if on { app.glyphs.cursor } else { " " };
            let label = l.label.as_deref().map(one_line).unwrap_or_default();
            let url = one_line(&l.url);
            let mut spans = vec![
                Span::styled(format!("  {cursor} "), t.bold()),
                Span::raw(format!("{}  ", i + 1)),
            ];
            if label_w > 0 {
                spans.push(Span::styled(
                    format!("{}  ", pad(&ellipsize(&label, label_w, ell), label_w)),
                    if on { t.bold() } else { t.fg(UiColor::Text) },
                ));
            }
            let rest = room.saturating_sub(if label_w == 0 { 0 } else { label_w + 2 });
            spans.push(colored(ellipsize(&url, rest, ell), UiColor::Accent, app));
            Line::from(spans)
        })
        .collect();
    Page {
        title: Line::from(Span::styled("Links", t.bold())),
        right: Some(Line::from(muted(format!("{} links", links.len()), app))),
        body: window_block(lines, index, index, rows, app),
        hints: vec![
            hint("enter open it", 1),
            hint("↑↓ choose", 2),
            hint("1-9 open that one", 3),
        ],
        pinned: vec!["esc back".into()],
    }
}

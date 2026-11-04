//! The list page: one answer at the top, then the sections in pa's order, one row each.

use ratatui::text::{Line, Span};

use super::{Hint, Page, colored, hint, muted, window_block};
use crate::app::App;
use crate::text::{ellipsize, pad, width};
use crate::theme::UiColor;
use crate::triage::{
    Action, Entry, Kind, Section, actions_for, phrase, section_title, status_answer,
};

const BADGE: &str = "also armed in pa";

pub fn glyph(app: &App, kind: Kind) -> (&'static str, UiColor) {
    let g = &app.glyphs;
    match kind {
        Kind::Invalid => (g.invalid, UiColor::Error),
        Kind::Failed => (g.failed, UiColor::Error),
        Kind::WatchProblem | Kind::Consent => (g.problem, UiColor::Warning),
        Kind::Running => (g.active, UiColor::Accent),
        Kind::Coming | Kind::Manual | Kind::Watched => (g.armed, UiColor::Success),
        Kind::Paused => (g.paused, UiColor::Warning),
        Kind::Disabled => (g.disabled, UiColor::Muted),
        Kind::NotArmed => (g.off, UiColor::Muted),
    }
}

/// The words a key's hint uses on the list: shorter than the automation page's.
pub fn short_label(action: Action, kind: Kind) -> &'static str {
    match action {
        Action::Run if kind == Kind::Failed => "run it again",
        Action::Run => "run now",
        Action::ShowRun => "show it",
        Action::Stop => "stop it",
        Action::PauseWatch => "pause watch",
        Action::ResumeWatch => "resume watch",
        Action::Arm if kind == Kind::Consent => "look, then arm",
        Action::Arm => "arm it",
        Action::Disarm => "disarm",
        Action::Enable => "enable",
        Action::Disable => "disable",
        Action::Edit if kind == Kind::Invalid => "fix it",
        Action::Edit => "edit",
        Action::Agent => "agent",
        Action::Check => "check",
        Action::Runs => "runs",
    }
}

fn row_line(
    app: &App,
    entry: &Entry,
    on: bool,
    name_w: usize,
    agent_w: usize,
    w: usize,
) -> Line<'static> {
    let Some(s) = app.status(&entry.key) else {
        return Line::default();
    };
    let t = &app.theme;
    let ell = app.glyphs.ellipsis;
    let (g, gc) = glyph(app, entry.kind);
    let cursor = if on { app.glyphs.cursor } else { " " };
    let (text, tone) = phrase(
        s,
        entry.kind,
        app.live.get(&s.key),
        &app.now,
        app.glyphs.dash,
    );
    let name_style = if on {
        t.bold()
    } else if entry.section == Section::Off {
        t.fg(UiColor::Muted)
    } else {
        t.fg(UiColor::Text)
    };

    // Lead (4) + glyph (2) + name column + two spaces, then the phrase, then the tags on the right.
    let lead = 6;
    let name_col = name_w + 2;
    let agent = s.agent.clone().unwrap_or_default();
    let mut show_agent = agent_w > 0 && !agent.is_empty();
    let mut show_badge = s.also_armed_in_pa;
    let right_w = |agent: bool, badge: bool| {
        let a = if agent { agent_w } else { 0 };
        let b = if badge { width(BADGE) } else { 0 };
        let between = if agent && badge { 2 } else { 0 };
        let total = a + b + between;
        if total > 0 { total + 2 } else { 0 }
    };
    let room = |agent: bool, badge: bool| {
        w as isize - (lead + name_col) as isize - right_w(agent, badge) as isize
    };
    if room(show_agent, show_badge) < 14 {
        show_agent = false;
    }
    if room(show_agent, show_badge) < 14 {
        show_badge = false;
    }
    let mut spans = vec![
        Span::styled(format!("  {cursor} "), t.bold()),
        Span::styled(format!("{g} "), t.fg(gc)),
    ];
    let phrase_room = room(show_agent, show_badge);
    if phrase_room < 4 {
        // No room for a phrase: the name alone.
        let name_room = w.saturating_sub(lead);
        spans.push(Span::styled(ellipsize(&s.name, name_room, ell), name_style));
        return Line::from(spans);
    }
    let phrase_room = phrase_room as usize;
    spans.push(Span::styled(
        pad(&ellipsize(&s.name, name_w, ell), name_col),
        name_style,
    ));
    let shown = ellipsize(&text, phrase_room, ell);
    let has_right = show_agent || show_badge;
    let fill = if has_right {
        pad(&shown, phrase_room)
    } else {
        shown
    };
    spans.push(Span::styled(fill, t.fg(tone)));
    if has_right {
        spans.push(Span::raw("  "));
        if show_badge {
            spans.push(colored(BADGE, UiColor::Warning, app));
            if show_agent {
                spans.push(Span::raw("  "));
            }
        }
        if show_agent {
            spans.push(muted(pad(&agent, agent_w), app));
        }
    }
    Line::from(spans)
}

pub fn page(app: &App, w: usize, rows: usize) -> Page {
    let t = &app.theme;
    let title = Line::from(Span::styled("Automations", t.bold()));
    let watcher = format!("watcher {}", app.watcher);
    let off = app.watcher.starts_with("off") || app.watcher.contains("late");
    let right = (!app.watcher.is_empty()).then(|| {
        Line::from(colored(
            watcher,
            if off {
                UiColor::Warning
            } else {
                UiColor::Muted
            },
            app,
        ))
    });

    let (answer, tone) = status_answer(&app.statuses, &app.triage, &app.now, app.glyphs.dash);
    let mut body = vec![
        Line::from(Span::styled(
            format!("  {answer}"),
            t.fg(tone).patch(t.bold()),
        )),
        Line::default(),
    ];

    let entries: Vec<&Entry> = app.triage.order().collect();
    let name_w = entries
        .iter()
        .filter_map(|e| app.status(&e.key))
        .map(|s| width(&s.name))
        .max()
        .unwrap_or(8)
        .clamp(8, 24);
    let agent_w = entries
        .iter()
        .filter_map(|e| app.status(&e.key))
        .filter_map(|s| s.agent.as_deref().map(width))
        .max()
        .unwrap_or(0);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut at = 0;
    let mut header_at = 0;
    for (section, items) in &app.triage.sections {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        let this_header = lines.len();
        lines.push(Line::from(Span::styled(
            format!("  {}", section_title(*section)),
            t.section(*section),
        )));
        for e in items {
            let on = app.selected.as_deref() == Some(e.key.as_str());
            if on {
                at = lines.len();
                header_at = this_header;
            }
            lines.push(row_line(app, e, on, name_w, agent_w, w));
        }
    }
    if entries.is_empty() {
        lines.push(Line::from(muted(
            ellipsize(
                "  Put an AUTOMATION.md in ~/.agents/automations/<name>/, then press R",
                w,
                app.glyphs.ellipsis,
            ),
            app,
        )));
    }
    let room = rows.saturating_sub(body.len());
    // Keep the selected row's section heading in view with it, when both fit.
    let from = if at - header_at + 3 <= room {
        header_at
    } else {
        at
    };
    body.extend(window_block(lines, from, at, room, app));

    Page {
        title,
        right,
        body,
        hints: row_hints(app),
        pinned: vec!["? help".into(), "q quit".into()],
    }
}

/// Only keys valid for the selected row, its most useful first.
fn row_hints(app: &App) -> Vec<Hint> {
    let Some(s) = app.selected_status() else {
        return vec![hint("R read again", 1)];
    };
    let kind = app
        .triage
        .order()
        .find(|e| e.key == s.key)
        .map_or(Kind::Manual, |e| e.kind);
    let mut out = Vec::new();
    let keyed: Vec<_> = actions_for(s)
        .into_iter()
        .filter(|a| a.key.is_some())
        .collect();
    for (i, a) in keyed.iter().enumerate() {
        let rank = if i == 0 { 1 } else { 3 + i as u8 };
        out.push(hint(
            format!(
                "{} {}",
                a.key.unwrap_or_default(),
                short_label(a.action, kind)
            ),
            rank,
        ));
        if i == 0 {
            out.push(hint("enter open", 2));
        }
    }
    out.push(hint("↑↓ move", 3));
    out
}

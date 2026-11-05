//! The run page: one run's progress, steps or flowchart, live events, message, notes and answer,
//! with the automation's runs underneath to choose from.

use fm_model::{LiveEvent, LiveRun, Outcome, RunNote, RunRecord, RunStep, Status, StepStatus};
use ratatui::text::{Line, Span};

use super::pages::{link_spans, outcome_color, outcome_name};
use super::{Page, colored, heading, hint, muted, scroll_block, window_block, wrapped};
use crate::app::{App, RunItem, RunPage};
use crate::text::{ellipsize, one_line, pad, width, wrap};
use crate::theme::UiColor;
use crate::triage::step_of;
use crate::words::{
    ago_long, at_words, day_time_words, moment_words, parse, running_words, span_long, span_words,
    took_ms, trigger_words, usual_ms,
};

fn step_color(s: StepStatus) -> UiColor {
    match s {
        StepStatus::Done => UiColor::Success,
        StepStatus::Active => UiColor::Accent,
        StepStatus::Waiting | StepStatus::Interrupted => UiColor::Warning,
        StepStatus::Failed => UiColor::Error,
        StepStatus::Pending | StepStatus::Skipped => UiColor::Muted,
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

/// Steps in order, each with its glyph and latest update.
/// A step's tasks, one line each under it, numbered as planned.
fn task_lines(app: &App, step: &RunStep, w: usize) -> Vec<Line<'static>> {
    let ell = app.glyphs.ellipsis;
    step.tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let label_style = if t.status == StepStatus::Pending {
                app.theme.fg(UiColor::Muted)
            } else {
                app.theme.fg(UiColor::Text)
            };
            Line::from(vec![
                Span::raw("        "),
                colored(app.glyphs.step_tree(t.status), step_color(t.status), app),
                Span::raw(" "),
                Span::styled(
                    ellipsize(
                        &format!("{}. {}", i + 1, t.title),
                        w.saturating_sub(10),
                        ell,
                    ),
                    label_style,
                ),
            ])
        })
        .collect()
}

fn step_tree(app: &App, steps: &[RunStep], w: usize) -> Vec<Line<'static>> {
    let ell = app.glyphs.ellipsis;
    let label_w = steps
        .iter()
        .map(|s| width(&s.label))
        .max()
        .unwrap_or(0)
        .min(w / 2);
    steps
        .iter()
        .flat_map(|s| {
            let mut spans = vec![
                Span::raw("    "),
                colored(app.glyphs.step_tree(s.status), step_color(s.status), app),
                Span::raw(" "),
            ];
            let label_style = if s.status == StepStatus::Pending {
                app.theme.fg(UiColor::Muted)
            } else {
                app.theme.fg(UiColor::Text)
            };
            let note = s.text.as_deref().map(one_line).filter(|t| !t.is_empty());
            let room = w.saturating_sub(6 + label_w + 3);
            match note {
                Some(n) if room > 8 => {
                    spans.push(Span::styled(
                        pad(&ellipsize(&s.label, label_w, ell), label_w),
                        label_style,
                    ));
                    spans.push(muted(format!("   {}", ellipsize(&n, room, ell)), app));
                }
                _ => spans.push(Span::styled(
                    ellipsize(&s.label, w.saturating_sub(6), ell),
                    label_style,
                )),
            }
            let mut lines = vec![Line::from(spans)];
            lines.extend(task_lines(app, s, w));
            lines
        })
        .collect()
}

/// The step list that stands in for a flowchart: glyph, label, status name, update.
fn step_list(app: &App, steps: &[RunStep], w: usize) -> Vec<Line<'static>> {
    let ell = app.glyphs.ellipsis;
    let label_w = steps
        .iter()
        .map(|s| width(&s.label))
        .max()
        .unwrap_or(4)
        .clamp(4, 36);
    steps
        .iter()
        .flat_map(|s| {
            let c = step_color(s.status);
            let mut spans = vec![
                Span::raw("    "),
                colored(app.glyphs.step_list(s.status), c, app),
                Span::raw(" "),
                Span::raw(pad(&ellipsize(&s.label, label_w, ell), label_w)),
                Span::raw("  "),
                colored(pad(status_name(s.status), 11), c, app),
            ];
            let used = 4 + 2 + label_w + 2 + 11;
            if let Some(t) = s.text.as_deref().map(one_line).filter(|t| !t.is_empty()) {
                let room = w.saturating_sub(used + 3);
                if room > 4 {
                    spans.push(muted(format!(" · {}", ellipsize(&t, room, ell)), app));
                }
            }
            let mut lines = vec![Line::from(spans)];
            lines.extend(task_lines(app, s, w));
            lines
        })
        .collect()
}

/// fm's text graph, each status glyph in its colour and nothing else coloured.
fn graph_lines(app: &App, text: &str) -> Vec<Line<'static>> {
    let all = [
        StepStatus::Pending,
        StepStatus::Active,
        StepStatus::Waiting,
        StepStatus::Done,
        StepStatus::Failed,
        StepStatus::Skipped,
        StepStatus::Interrupted,
    ];
    let unicode = crate::glyphs::Glyphs::UNICODE;
    text.lines()
        .map(|l| {
            let mut spans = vec![Span::raw("    ")];
            let mut plain = String::new();
            for c in one_line_keep_spaces(l).chars() {
                let mut buf = [0u8; 4];
                let g = &*c.encode_utf8(&mut buf);
                match all.iter().find(|s| unicode.step_list(**s) == g) {
                    Some(s)
                        if *s != StepStatus::Pending || !plain.ends_with(char::is_alphanumeric) =>
                    {
                        if !plain.is_empty() {
                            spans.push(Span::raw(std::mem::take(&mut plain)));
                        }
                        spans.push(colored(g.to_string(), step_color(*s), app));
                    }
                    _ => plain.push(c),
                }
            }
            if !plain.is_empty() {
                spans.push(Span::raw(plain.trim_end().to_string()));
            }
            Line::from(spans)
        })
        .collect()
}

/// Control characters out, spaces kept: a chart's layout is its spaces.
fn one_line_keep_spaces(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// One live event as one line.
fn event_line(app: &App, ev: &LiveEvent, w: usize) -> Option<Line<'static>> {
    let ell = app.glyphs.ellipsis;
    let room = w.saturating_sub(4);
    let fit = |s: String| ellipsize(&one_line(&s), room, ell);
    let spans = match ev {
        LiveEvent::Session { id, .. } => vec![muted(fit(format!("session {id}")), app)],
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
            let st = match status {
                Some(fm_model::HookStatus::Ok) => "ok",
                Some(fm_model::HookStatus::Failed) => "failed",
                Some(fm_model::HookStatus::Skipped) => "skipped",
                None => "running",
            };
            vec![muted(
                fit(format!("{phase} hook {} · {source} · {st}", index + 1)),
                app,
            )]
        }
        LiveEvent::Tool { name, summary, .. } => {
            let name = one_line(name);
            let rest = room.saturating_sub(width(&name) + 3);
            vec![
                colored(name, UiColor::Accent, app),
                muted(
                    format!(" · {}", ellipsize(&one_line(summary), rest, ell)),
                    app,
                ),
            ]
        }
        LiveEvent::Thinking { text, .. } => vec![muted(fit(format!("thinking · {text}")), app)],
        LiveEvent::Message { text, .. } | LiveEvent::Partial { text, .. } => {
            vec![Span::raw(fit(text.clone()))]
        }
        LiveEvent::Final { text, .. } => vec![Span::raw(fit(format!("answer · {text}")))],
        LiveEvent::Error { message, .. } => vec![colored(
            fit(format!("error · {message}")),
            UiColor::Error,
            app,
        )],
    };
    let mut all = vec![Span::raw("    ")];
    all.extend(spans);
    Some(Line::from(all))
}

fn message_lines(app: &App, title: &str, m: &RunNote, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![heading(title, app)];
    let text = one_line(&m.text);
    for l in wrap(&text, w.saturating_sub(4)).into_iter().take(6) {
        out.push(Line::from(format!("    {l}")));
    }
    for link in &m.links {
        let mut spans = vec![Span::raw("    ")];
        spans.extend(link_spans(app, link, w.saturating_sub(4)));
        out.push(Line::from(spans));
    }
    out.push(Line::default());
    out
}

fn notes_lines(app: &App, title: &str, notes: &[RunNote], w: usize) -> Vec<Line<'static>> {
    if notes.is_empty() {
        return vec![];
    }
    let ell = app.glyphs.ellipsis;
    let mut out = vec![heading(title, app)];
    for n in notes {
        out.push(Line::from(format!(
            "    {}",
            ellipsize(&one_line(&n.text), w.saturating_sub(4), ell)
        )));
        for link in &n.links {
            let mut spans = vec![Span::raw("      ")];
            spans.extend(link_spans(app, link, w.saturating_sub(6)));
            out.push(Line::from(spans));
        }
    }
    out.push(Line::default());
    out
}

/// Steps, or with `g` the flowchart, falling back to the step list when it cannot be drawn.
fn progress_lines(app: &App, page: &RunPage, steps: &[RunStep], w: usize) -> Vec<Line<'static>> {
    let mut out = vec![];
    if page.graph {
        out.push(heading("Flowchart", app));
        match page.chart.as_ref().map(|c| &c.drawn) {
            Some(Ok(text)) => out.extend(graph_lines(app, text)),
            Some(Err(e)) => {
                out.push(Line::from(muted(
                    ellipsize(
                        &format!(
                            "    {} {} {} showing the steps instead",
                            app.glyphs.reason,
                            one_line(e),
                            app.glyphs.dash
                        ),
                        w,
                        app.glyphs.ellipsis,
                    ),
                    app,
                )));
                out.extend(step_list(app, steps, w));
            }
            None => out.push(Line::from(muted("    drawing the flowchart", app))),
        }
        out.push(Line::default());
    } else if !steps.is_empty() {
        out.push(heading("Steps", app));
        out.extend(step_tree(app, steps, w));
        out.push(Line::default());
    }
    out
}

fn live_lines(
    app: &App,
    page: &RunPage,
    s: &Status,
    live: Option<&LiveRun>,
    w: usize,
    records: &[RunRecord],
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut out = vec![];
    let r = s.running.as_ref();
    let elapsed = r
        .and_then(|r| parse(&r.started_at))
        .map_or(0, |at| (app.now - at).num_milliseconds());
    let lead = format!(
        "{} is running: {}",
        s.name,
        running_words(elapsed, usual_ms(records), s.effective_timeout_ms)
    );
    out.extend(wrapped(
        &lead,
        2,
        w.saturating_sub(2),
        t.fg(UiColor::Accent),
    ));
    if let Some(r) = r {
        out.extend(wrapped(
            &format!(
                "Started {} {}.",
                trigger_words(r.trigger),
                at_words(&r.started_at, &app.now)
            ),
            2,
            w.saturating_sub(2),
            t.fg(UiColor::Muted),
        ));
    }
    out.push(Line::default());
    let progress = live.and_then(|l| l.progress.as_ref());
    out.extend(progress_lines(
        app,
        page,
        progress.map(|p| p.steps.as_slice()).unwrap_or(&[]),
        w,
    ));
    if let Some(l) = live {
        // The answer once: a message the final answer repeats is shown as the answer alone.
        let repeated: Vec<usize> = (0..l.events.len())
            .filter_map(|i| fm_model::repeated_message(&l.events, i))
            .collect();
        let shown: Vec<Line<'static>> = l
            .events
            .iter()
            .enumerate()
            .rev()
            .filter(|(i, _)| !repeated.contains(i))
            .filter_map(|(_, e)| event_line(app, e, w))
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if !shown.is_empty() {
            out.push(heading("Live", app));
            out.extend(shown);
            out.push(Line::default());
        }
    }
    if let Some(p) = progress {
        if let Some(m) = &p.message {
            out.extend(message_lines(app, "Message so far", m, w));
        }
        out.extend(notes_lines(app, "Notes so far", &p.notes, w));
    }
    out.push(Line::from(muted(
        ellipsize(
            "  Nothing needed from you. It moves back to Coming up when it finishes.",
            w,
            app.glyphs.ellipsis,
        ),
        app,
    )));
    out
}

/// What a finished run said, as one line: its message, its error, or `skipped: weekend`.
fn says(rec: &RunRecord) -> String {
    let reason = rec
        .reason
        .as_deref()
        .map(one_line)
        .filter(|r| !r.is_empty());
    let error = rec.error.as_ref().map(|e| one_line(&e.message));
    let message = rec.message.as_ref().map(|m| one_line(&m.text));
    match rec.outcome {
        Outcome::Skipped => reason.map_or("skipped".into(), |r| format!("skipped: {r}")),
        Outcome::Missed => reason.map_or("missed".into(), |r| format!("missed: {r}")),
        Outcome::Aborted => reason.map_or("stopped".into(), |r| format!("stopped: {r}")),
        Outcome::Failed => error.or(reason).unwrap_or_else(|| "failed".into()),
        Outcome::Ok => message
            .or_else(|| rec.output.as_ref().map(|o| one_line(&o.text)))
            .unwrap_or_default(),
    }
}

fn record_lines(
    app: &App,
    page: &RunPage,
    s: &Status,
    rec: &RunRecord,
    w: usize,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut out = vec![];
    let took = took_ms(rec);
    let after = took
        .map(|ms| format!(" after {}", span_long(ms)))
        .unwrap_or_default();
    let why = rec
        .error
        .as_ref()
        .map(|e| one_line(&e.message))
        .or_else(|| rec.reason.as_deref().map(one_line));
    let name = &s.name;
    let when = rec.ended_at.as_deref().unwrap_or(&rec.recorded_at);
    let lead = match rec.outcome {
        Outcome::Ok => format!(
            "{name} finished {}{}.",
            ago_long(when, &app.now),
            took.map(|ms| format!("; it took {}", span_long(ms)))
                .unwrap_or_default()
        ),
        Outcome::Failed => format!(
            "{name} failed{after}{}.",
            why.map(|w| format!(": {w}")).unwrap_or_default()
        ),
        Outcome::Aborted => format!("{name} was stopped{after}."),
        Outcome::Skipped => format!(
            "{name} skipped this run{}.",
            why.map(|w| format!(": {w}")).unwrap_or_default()
        ),
        Outcome::Missed => format!(
            "{name} missed this run{}.",
            why.map(|w| format!(": {w}")).unwrap_or_default()
        ),
    };
    out.extend(wrapped(
        &lead,
        2,
        w.saturating_sub(2),
        t.fg(outcome_color(rec.outcome)),
    ));
    let started = match rec.started_at.as_deref() {
        Some(at) => format!(
            "Started {} {}.",
            trigger_words(rec.trigger),
            at_words(at, &app.now)
        ),
        None => format!(
            "Scheduled for {}.",
            day_time_words(
                rec.scheduled_for.as_deref().unwrap_or(&rec.recorded_at),
                &app.now
            )
        ),
    };
    out.extend(wrapped(
        &started,
        2,
        w.saturating_sub(2),
        t.fg(UiColor::Muted),
    ));
    out.push(Line::default());
    out.extend(progress_lines(app, page, &rec.steps, w));
    if let Some(m) = &rec.message {
        out.extend(message_lines(app, "Message", m, w));
    }
    out.extend(notes_lines(app, "Notes", &rec.notes, w));
    if let Some(o) = rec.output.as_ref().filter(|o| !o.text.trim().is_empty()) {
        out.push(heading("Answer", app));
        let lines = wrap(&one_line(&o.text), w.saturating_sub(4));
        let n = lines.len();
        for l in lines.into_iter().take(8) {
            out.push(Line::from(format!("    {l}")));
        }
        if n > 8 || o.truncated == Some(true) {
            out.push(Line::from(muted(
                format!("    {} full text in the session file", app.glyphs.ellipsis),
                app,
            )));
        }
        out.push(Line::default());
    }
    let agent = rec
        .agent
        .as_deref()
        .map(|a| format!(" · {a}"))
        .unwrap_or_default();
    out.push(Line::from(muted(
        ellipsize(&format!("  run {}{agent}", rec.id), w, app.glyphs.ellipsis),
        app,
    )));
    out
}

/// The automation's runs, newest first, the live one on top.
fn runs_panel(
    app: &App,
    items: &[RunItem<'_>],
    selected: usize,
    w: usize,
    room: usize,
) -> Vec<Line<'static>> {
    let ell = app.glyphs.ellipsis;
    let t = &app.theme;
    let when = |i: &RunItem<'_>| match i {
        RunItem::Live(..) => "now".to_string(),
        RunItem::Record(r) => {
            moment_words(r.started_at.as_deref().unwrap_or(&r.recorded_at), &app.now)
        }
    };
    let took = |i: &RunItem<'_>| match i {
        RunItem::Live(s, _) => s
            .running
            .as_ref()
            .and_then(|r| parse(&r.started_at))
            .map(|at| span_words((app.now - at).num_milliseconds()))
            .unwrap_or_default(),
        RunItem::Record(r) => took_ms(r)
            .filter(|_| !matches!(r.outcome, Outcome::Skipped | Outcome::Missed))
            .map(span_words)
            .unwrap_or_default(),
    };
    let when_w = items.iter().map(|i| width(&when(i))).max().unwrap_or(3) + 2;
    let took_w = items
        .iter()
        .map(|i| width(&took(i)))
        .max()
        .unwrap_or(2)
        .max(2)
        + 3;
    let mut lines = vec![];
    for (i, item) in items.iter().enumerate() {
        let on = i == selected;
        let cursor = if on { app.glyphs.cursor } else { " " };
        let (g, c, text) = match item {
            RunItem::Live(_, live) => {
                let step = step_of(*live)
                    .map(|(a, b)| format!(" {} step {a}/{b}", app.glyphs.dash))
                    .unwrap_or_default();
                (app.glyphs.active, UiColor::Accent, format!("running{step}"))
            }
            RunItem::Record(r) => (
                app.glyphs.outcome(r.outcome),
                outcome_color(r.outcome),
                says(r),
            ),
        };
        let lead = 4 + 2 + when_w + took_w;
        let room = w.saturating_sub(lead);
        let quiet = matches!(item, RunItem::Record(r) if matches!(r.outcome, Outcome::Skipped | Outcome::Missed));
        let style = if on {
            t.bold()
        } else if quiet {
            t.fg(UiColor::Muted)
        } else {
            t.fg(UiColor::Text)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {cursor} "), t.bold()),
            colored(format!("{g} "), c, app),
            Span::styled(pad(&when(item), when_w), style),
            muted(pad(&took(item), took_w), app),
            Span::styled(ellipsize(&text, room, ell), style),
        ]));
    }
    window_block(lines, selected, selected, room, app)
}

pub fn page(app: &App, page: &RunPage, w: usize, rows: usize) -> Page {
    let t = &app.theme;
    let empty = || Page {
        title: Line::default(),
        right: None,
        body: vec![],
        hints: vec![],
        pinned: vec!["esc back".into()],
    };
    let Some(s) = app.status(&page.key) else {
        return empty();
    };
    let items = app.run_items(page);
    let records: &[RunRecord] = &page.runs;
    let selected = page.selected.min(items.len().saturating_sub(1));
    let shown = items.get(selected).copied();

    let (state, state_color, right) = match shown {
        Some(RunItem::Live(s, _)) => {
            let elapsed = s
                .running
                .as_ref()
                .and_then(|r| parse(&r.started_at))
                .map_or(0, |at| (app.now - at).num_milliseconds());
            let usual = usual_ms(records)
                .map(|u| format!(" · usually {}", span_words(u)))
                .unwrap_or_default();
            (
                "running",
                UiColor::Accent,
                Some(format!("{} in{usual}", span_words(elapsed))),
            )
        }
        Some(RunItem::Record(r)) => (
            outcome_name(r.outcome),
            outcome_color(r.outcome),
            took_ms(r).map(|ms| format!("took {}", span_words(ms))),
        ),
        None => ("no runs yet", UiColor::Muted, None),
    };
    let title = Line::from(vec![
        Span::styled(s.name.clone(), t.bold()),
        muted(" · ", app),
        colored(state, state_color, app),
    ]);

    let detail = match shown {
        Some(RunItem::Live(s, live)) => live_lines(app, page, s, live, w, records),
        Some(RunItem::Record(r)) => record_lines(app, page, s, r, w),
        None => vec![Line::from(muted(
            format!("  {} has not run yet", s.name),
            app,
        ))],
    };

    // The runs to choose from stay at the bottom; the run fills the rest and scrolls.
    let panel_room = if items.len() > 1 && rows >= 6 {
        (items.len() + 1).min(rows / 2).min(8)
    } else {
        0
    };
    let mut body;
    let detail_len = detail.len();
    let detail_room;
    if panel_room >= 2 {
        detail_room = rows - panel_room - 1;
        body = scroll_block(detail, page.scroll, detail_room, app);
        body.resize(detail_room, Line::default());
        body.push(Line::default());
        body.push(heading("Runs", app));
        body.extend(runs_panel(app, &items, selected, w, panel_room - 1));
    } else {
        detail_room = rows;
        body = scroll_block(detail, page.scroll, rows, app);
    }

    let live = matches!(shown, Some(RunItem::Live(..)));
    let mut hints = vec![];
    if live {
        hints.push(hint("x stop it", 1));
    }
    if items.len() > 1 {
        hints.push(hint("↑↓ choose a run", 2));
    }
    if app.chart_source(page).is_some() {
        hints.push(hint(if page.graph { "g steps" } else { "g flowchart" }, 3));
    }
    if !app.run_links(page).is_empty() {
        hints.push(hint("o open link", 4));
    }
    if detail_len > detail_room {
        hints.push(hint("PgUp PgDn scroll", 6));
    }
    hints.push(hint("e edit", 5));
    Page {
        title,
        right: right.map(|r| Line::from(muted(r, app))),
        body,
        hints,
        pinned: vec!["esc back".into()],
    }
}

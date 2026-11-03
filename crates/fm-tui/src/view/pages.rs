//! The automation page (`enter`), the check page (`c`) and the keys page (`?`).

use fm_model::{AutomationState, Hook, Outcome, Status};
use ratatui::text::{Line, Span};

use super::{Page, colored, heading, hint, muted, row, scroll_block, window_block, wrapped};
use crate::app::App;
use crate::text::{ellipsize, one_line, pad, wrap};
use crate::theme::UiColor;
use crate::triage::{Kind, actions_for, classify};
use crate::words::{
    ago_long, ago_words, cadence_words, moment_words, parse, running_words, span_words, time_words,
    timeout_words, took_ms, trigger_words, usual_ms, when_long, when_words,
};

fn state_words(s: &Status) -> &'static str {
    match s.state {
        AutomationState::Invalid => "invalid",
        AutomationState::Disabled => "disabled",
        AutomationState::Off => "off",
        AutomationState::Armed => "armed",
    }
}

fn scope_words(s: &Status) -> &'static str {
    match s.scope {
        fm_model::Scope::User => "user",
        fm_model::Scope::Repository => "repository",
    }
}

/// `Next run tomorrow at 08:00, then weekdays. Its watch checks every 10m.`
fn next_sentence(app: &App, s: &Status) -> Option<String> {
    let a = s.automation.as_ref()?;
    let mut parts = vec![];
    if let (Some(sc), Some(next)) = (&a.on.schedule, &s.next_fire) {
        parts.push(format!(
            "Next run {}, then {}.",
            when_long(next, &app.now),
            cadence_words(&sc.cron)
        ));
    }
    if let Some(w) = a.on.watch.as_ref().filter(|_| s.watch_paused_at.is_none()) {
        let also = if parts.is_empty() {
            "Its watch checks"
        } else {
            "Its watch also checks"
        };
        parts.push(format!("{also} every {}.", w.every));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The sentence an automation's page opens with, in its state's colour.
pub fn lead(app: &App, s: &Status) -> (String, UiColor) {
    let name = &s.name;
    let records = app.records.get(&s.key).map(Vec::as_slice).unwrap_or(&[]);
    if let Some(r) = &s.running {
        let elapsed = parse(&r.started_at).map_or(0, |t| (app.now - t).num_milliseconds());
        return (
            format!(
                "{name} is running: {}",
                running_words(elapsed, usual_ms(records), s.effective_timeout_ms)
            ),
            UiColor::Accent,
        );
    }
    let reason = s.reason.as_deref().map(one_line).unwrap_or_default();
    match classify(s).1 {
        Kind::Invalid => (
            format!("{name} has a problem in its file: {reason}."),
            UiColor::Error,
        ),
        Kind::NotArmed => (
            format!(
                "{name} is not armed yet. Nothing runs it until you arm it, after a preview of what it runs."
            ),
            UiColor::Muted,
        ),
        Kind::Consent => (
            format!("{name} is off: {reason}. Look at it again, then arm it."),
            UiColor::Warning,
        ),
        Kind::Disabled => (
            format!("{name} is disabled. Nothing starts it, not even r, until you enable it."),
            UiColor::Muted,
        ),
        Kind::Failed => {
            let at = s
                .last
                .as_ref()
                .map(|l| ago_long(&l.at, &app.now))
                .unwrap_or_default();
            let again = next_sentence(app, s)
                .map(|n| format!(" {}", n.replacen("Next run", "It runs again on its own", 1)))
                .unwrap_or_default();
            (format!("{name} failed {at}.{again}"), UiColor::Error)
        }
        Kind::WatchProblem => {
            let p = s
                .watch
                .as_ref()
                .and_then(|w| w.problem.as_deref())
                .map(one_line)
                .unwrap_or_default();
            let again = s
                .next_check
                .as_deref()
                .map(|n| format!(" It tries again {}.", when_long(n, &app.now)))
                .unwrap_or_default();
            (
                format!("{name}'s watch could not check: {p}.{again}"),
                UiColor::Warning,
            )
        }
        Kind::Paused => (
            format!(
                "{name}'s watch is paused since {}, so nothing starts it on its own. You can still run it.",
                s.watch_paused_at
                    .as_deref()
                    .map(|t| time_words(t, &app.now))
                    .unwrap_or_default()
            ),
            UiColor::Warning,
        ),
        _ => {
            let healthy = s
                .last
                .as_ref()
                .is_none_or(|l| matches!(l.outcome, Outcome::Ok | Outcome::Skipped));
            let first = if healthy {
                format!("{name} is healthy.")
            } else {
                format!("{name} is armed.")
            };
            let manual = s.automation.as_ref().is_some_and(|a| a.on.manual);
            let next = next_sentence(app, s).unwrap_or_else(|| {
                if manual {
                    "It runs only when you run it.".into()
                } else {
                    "Nothing starts it on its own.".into()
                }
            });
            (
                format!("{first} {next}"),
                if healthy {
                    UiColor::Success
                } else {
                    UiColor::Text
                },
            )
        }
    }
}

const LABEL_W: usize = 11;

pub fn outcome_color(o: Outcome) -> UiColor {
    match o {
        Outcome::Ok => UiColor::Success,
        Outcome::Failed => UiColor::Error,
        Outcome::Aborted => UiColor::Warning,
        Outcome::Skipped | Outcome::Missed => UiColor::Muted,
    }
}

pub fn outcome_name(o: Outcome) -> &'static str {
    match o {
        Outcome::Ok => "ok",
        Outcome::Failed => "failed",
        Outcome::Skipped => "skipped",
        Outcome::Missed => "missed",
        Outcome::Aborted => "aborted",
    }
}

pub fn outcome_words(o: Outcome) -> &'static str {
    match o {
        Outcome::Ok => "",
        Outcome::Failed => "failed",
        Outcome::Skipped => "skipped",
        Outcome::Missed => "missed",
        Outcome::Aborted => "stopped",
    }
}

/// `Agent`, `Last run`, `Message`: how its last run went and what it left for you.
fn last_run_lines(app: &App, s: &Status, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![];
    let value_w = w.saturating_sub(2 + LABEL_W).max(10);
    if let Some(agent) = &s.agent {
        let title = agent
            .parse::<fm_model::CliId>()
            .map(|c| c.title().to_string())
            .unwrap_or_default();
        let mut v = vec![Span::raw(agent.clone())];
        if !title.is_empty() && title != *agent {
            v.push(muted(format!(" · {title}"), app));
        }
        out.push(row("Agent", LABEL_W, v, app));
    }
    let records = app.records.get(&s.key).map(Vec::as_slice).unwrap_or(&[]);
    match (records.first(), &s.last) {
        (Some(last), _) => {
            let at = moment_words(
                last.ended_at.as_deref().unwrap_or(&last.recorded_at),
                &app.now,
            );
            let mark = format!(
                "{} {}",
                app.glyphs.outcome(last.outcome),
                outcome_words(last.outcome)
            );
            let took = took_ms(last)
                .filter(|_| !matches!(last.outcome, Outcome::Skipped | Outcome::Missed))
                .map(|ms| format!(" · took {}", span_words(ms)))
                .unwrap_or_default();
            out.push(row(
                "Last run",
                LABEL_W,
                vec![
                    colored(
                        mark.trim_end().to_string(),
                        outcome_color(last.outcome),
                        app,
                    ),
                    Span::raw(format!(" {at}{took}")),
                ],
                app,
            ));
            if let Some(m) = &last.message {
                for (i, l) in wrap(&one_line(&m.text), value_w)
                    .into_iter()
                    .take(4)
                    .enumerate()
                {
                    out.push(row(
                        if i == 0 { "Message" } else { "" },
                        LABEL_W,
                        vec![Span::raw(l)],
                        app,
                    ));
                }
                for link in &m.links {
                    out.push(row("", LABEL_W, link_spans(app, link, value_w), app));
                }
            } else if last.outcome == Outcome::Failed {
                let why = last
                    .error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .or_else(|| last.reason.clone())
                    .unwrap_or_default();
                for l in wrap(&one_line(&why), value_w).into_iter().take(3) {
                    out.push(row("", LABEL_W, vec![Span::raw(l)], app));
                }
            }
            if !last.notes.is_empty() {
                let n = last.notes.len();
                out.push(row(
                    "",
                    LABEL_W,
                    vec![muted(
                        format!(
                            "and {n} note{} · l shows them",
                            if n == 1 { "" } else { "s" }
                        ),
                        app,
                    )],
                    app,
                ));
            }
        }
        (None, Some(l)) => out.push(row(
            "Last run",
            LABEL_W,
            vec![
                colored(app.glyphs.outcome(l.outcome), outcome_color(l.outcome), app),
                Span::raw(format!(" {}", moment_words(&l.at, &app.now))),
            ],
            app,
        )),
        (None, None) => out.push(row("Last run", LABEL_W, vec![muted("never run", app)], app)),
    }
    out
}

/// `↗ label  https://…`, the url dim, cut without cutting the label.
pub fn link_spans(app: &App, link: &fm_model::RunLink, w: usize) -> Vec<Span<'static>> {
    let ell = app.glyphs.ellipsis;
    let url = one_line(&link.url);
    let label = link
        .label
        .as_deref()
        .map(one_line)
        .filter(|l| !l.is_empty() && *l != url);
    let mut out = vec![colored(
        format!("{} ", app.glyphs.link),
        UiColor::Accent,
        app,
    )];
    let room = w.saturating_sub(crate::text::width(app.glyphs.link) + 1);
    match label {
        Some(label) => {
            let label = ellipsize(&label, (room / 2).max(4), ell);
            let rest = room.saturating_sub(crate::text::width(&label) + 2);
            out.push(colored(label, UiColor::Accent, app));
            if rest >= 8 {
                out.push(muted(format!("  {}", ellipsize(&url, rest, ell)), app));
            }
        }
        None => out.push(colored(ellipsize(&url, room, ell), UiColor::Accent, app)),
    }
    out
}

pub fn automation(app: &App, key: &str, index: usize, w: usize, rows: usize) -> Page {
    let t = &app.theme;
    let Some(s) = app.status(key) else {
        return Page {
            title: Line::default(),
            right: None,
            body: vec![],
            hints: vec![],
            pinned: vec!["esc back".into()],
        };
    };
    let mut body = vec![];
    let (text, tone) = lead(app, s);
    body.extend(wrapped(&text, 2, w.saturating_sub(2), t.fg(tone)));
    if s.also_armed_in_pa {
        body.push(Line::from(colored(
            format!(
                "  {} also armed in pi-automations {} it would run twice",
                app.glyphs.problem, app.glyphs.dash
            ),
            UiColor::Warning,
            app,
        )));
    }
    body.push(Line::default());
    if let Some(d) = s
        .automation
        .as_ref()
        .map(|a| one_line(&a.description))
        .filter(|d| !d.is_empty())
    {
        body.extend(wrapped(&d, 2, w.saturating_sub(2), t.fg(UiColor::Text)));
        body.push(Line::default());
    }
    if s.state != AutomationState::Invalid || s.last.is_some() {
        body.extend(last_run_lines(app, s, w));
        body.push(Line::default());
    }
    let menu = actions_for(s);
    let index = index.min(menu.len().saturating_sub(1));
    let at = body.len() + index;
    for (i, a) in menu.iter().enumerate() {
        let on = i == index;
        let cursor = if on { app.glyphs.cursor } else { " " };
        body.push(Line::from(vec![
            Span::styled(format!("  {cursor} "), t.bold()),
            Span::raw(pad(a.key.unwrap_or(""), 7)),
            Span::styled(
                a.label.clone(),
                if on { t.bold() } else { t.fg(UiColor::Text) },
            ),
        ]));
    }
    let body = window_block(body, at, at, rows, app);
    Page {
        title: Line::from(Span::styled(s.name.clone(), t.bold())),
        right: Some(Line::from(muted(
            format!("{} · {}", state_words(s), scope_words(s)),
            app,
        ))),
        body,
        hints: vec![hint("enter do it", 1), hint("↑↓ choose", 2)],
        pinned: vec!["esc back".into()],
    }
}

/// Every technical detail, label by label.
pub fn check(app: &App, key: &str, scroll: usize, w: usize, rows: usize) -> Page {
    let t = &app.theme;
    let Some(s) = app.status(key) else {
        return Page {
            title: Line::default(),
            right: None,
            body: vec![],
            hints: vec![],
            pinned: vec!["esc back".into()],
        };
    };
    const W: usize = 15;
    let value_w = w.saturating_sub(2 + W).max(10);
    let mut lines: Vec<Line<'static>> = vec![];
    let mut add = |label: &str, value: String, color: UiColor| {
        for (i, l) in crate::text::wrap_hard(&value, value_w)
            .into_iter()
            .enumerate()
        {
            lines.push(row(
                if i == 0 { label } else { "" },
                W,
                vec![colored(l, color, app)],
                app,
            ));
        }
    };
    add("key", s.key.clone(), UiColor::Text);
    add("folder", s.dir.display().to_string(), UiColor::Text);
    add("scope", scope_words(s).into(), UiColor::Text);
    add("state", state_words(s).into(), UiColor::Text);
    if let Some(r) = &s.reason {
        add("why", one_line(r), UiColor::Warning);
    }
    for p in &s.problems {
        if Some(p) != s.reason.as_ref() {
            add("problem", one_line(p), UiColor::Error);
        }
    }
    if s.also_armed_in_pa {
        add(
            "pa",
            "also armed in pi-automations — it would run twice".into(),
            UiColor::Warning,
        );
    }
    add(
        "agent",
        s.agent.clone().unwrap_or_else(|| "none".into()),
        UiColor::Text,
    );
    if let Some(a) = &s.automation {
        add(
            "model",
            a.model
                .as_ref()
                .map(|m| m.display())
                .unwrap_or_else(|| "the agent's default".into()),
            UiColor::Text,
        );
        add(
            "timeout",
            timeout_words(s.effective_timeout_ms),
            UiColor::Text,
        );
        add("enabled", a.enabled.to_string(), UiColor::Text);
        add(
            "flow",
            match a.flow {
                fm_model::Flow::Loose => "loose",
                fm_model::Flow::Strict => "strict",
            }
            .into(),
            UiColor::Text,
        );
        add(
            "flowchart",
            if s.flow { "tracked" } else { "none" }.into(),
            UiColor::Text,
        );
        if let Some(sc) = &a.on.schedule {
            let mut v = sc.cron.clone();
            if let Some(tz) = &sc.timezone {
                v.push_str(&format!(" · {tz}"));
            }
            v.push_str(match sc.catch_up {
                fm_model::CatchUp::Once => " · catch-up once",
                fm_model::CatchUp::Skip => " · no catch-up",
            });
            add("schedule", v, UiColor::Text);
        }
        if let Some(n) = &s.next_fire {
            add(
                "Scheduled for",
                crate::words::day_time_words(n, &app.now),
                UiColor::Text,
            );
        }
        if let Some(wa) = &a.on.watch {
            add(
                "watch",
                format!("{} every {}", wa.script, wa.every),
                UiColor::Text,
            );
        }
        if let Some(p) = &s.watch_paused_at {
            add(
                "paused",
                format!("since {}", moment_words(p, &app.now)),
                UiColor::Warning,
            );
        }
        if let Some(ws) = &s.watch {
            let mut v = ago_words(&ws.checked_at, &app.now);
            if let Some(e) = ws.exit {
                v.push_str(&format!(" · exit {e}"));
            }
            v.push_str(&format!(" · took {}", span_words(ws.duration_ms as i64)));
            add("last check", v, UiColor::Text);
            if let Some(said) = &ws.said {
                add("it said", one_line(said), UiColor::Text);
            }
            if let Some(p) = &ws.problem {
                add("problem", one_line(p), UiColor::Error);
            }
        }
        if let Some(n) = &s.next_check {
            add("next check", when_words(n, &app.now), UiColor::Text);
        }
        if a.on.manual {
            add("manual", "allowed".into(), UiColor::Text);
        }
        for (phase, list) in [("pre", &a.hooks.pre), ("post", &a.hooks.post)] {
            for (i, h) in list.iter().enumerate() {
                let v = match h {
                    Hook::Script(p) => format!("script {p}"),
                    Hook::Prompt(p) => format!("prompt \"{}\"", one_line(p)),
                };
                add(&format!("{phase} {i}"), v, UiColor::Text);
            }
        }
    }
    if let Some(r) = &s.running {
        add(
            "running",
            format!(
                "run {} · pid {} · started {} at {}",
                r.run_id,
                r.pid,
                trigger_words(r.trigger),
                time_words(&r.started_at, &app.now)
            ),
            UiColor::Accent,
        );
    }
    if let Some(l) = &s.last {
        let took = l
            .duration_ms
            .map(|d| format!(" · took {}", span_words(d as i64)))
            .unwrap_or_default();
        add(
            "last run",
            format!(
                "{} · {}{took}",
                outcome_name(l.outcome),
                moment_words(&l.at, &app.now)
            ),
            outcome_color(l.outcome),
        );
    }
    let overflow = lines.len() > rows;
    let body = scroll_block(lines, scroll, rows, app);
    let mut hints = vec![hint("e edit", 1)];
    if overflow {
        hints.push(hint("↑↓ scroll", 2));
    }
    Page {
        title: Line::from(vec![
            Span::styled(s.name.clone(), t.bold()),
            muted(" · check", app),
        ]),
        right: None,
        body,
        hints,
        pinned: vec!["esc back".into()],
    }
}

pub fn help(app: &App, w: usize, rows: usize) -> Page {
    let sections: [(&str, &[(&str, &str)]); 4] = [
        (
            "The page",
            &[
                ("Needs you", "something went wrong or waits for you"),
                ("Running now", "runs going right now; nothing to do"),
                ("Coming up", "what runs on its schedule, soonest first"),
                ("Watching", "what a watch starts, and how often it checks"),
                ("Off and disabled", "disabled or not armed yet"),
            ],
        ),
        (
            "On the selected automation",
            &[
                ("enter", "open it: what it does, its last run, every action"),
                ("r  x", "run now · stop its run (it asks first)"),
                ("s  l", "show its run · its runs"),
                (
                    "space",
                    "arm (after the consent preview), disarm, enable or disable",
                ),
                ("p", "pause or resume its watch"),
                (
                    "e  a  c",
                    "edit · choose its agent · check: every technical detail",
                ),
            ],
        ),
        (
            "On a run page",
            &[
                ("↑↓", "choose a run"),
                ("g", "flowchart, or back to the steps"),
                ("o", "open a link it published"),
                ("PgUp PgDn", "scroll"),
            ],
        ),
        (
            "Everywhere",
            &[
                ("esc", "back"),
                ("q", "close"),
                ("y", "yes, when asked"),
                ("R", "read everything again"),
            ],
        ),
    ];
    let mut lines = vec![];
    for (i, (title, rows_)) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(heading(title, app));
        let key_w = if i == 0 { 18 } else { 11 };
        for (k, v) in rows_.iter() {
            let room = w.saturating_sub(4 + key_w);
            lines.push(Line::from(vec![
                Span::raw(format!("    {}", pad(k, key_w))),
                muted(ellipsize(v, room, app.glyphs.ellipsis), app),
            ]));
        }
    }
    Page {
        title: Line::from(Span::styled("Keys", app.theme.bold())),
        right: None,
        body: scroll_block(lines, 0, rows, app),
        hints: vec![],
        pinned: vec!["esc back".into()],
    }
}

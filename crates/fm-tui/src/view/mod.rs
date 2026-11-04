//! Drawing: the app's state to a buffer. Pure, so every screen is tested with ratatui's
//! `TestBackend`. The frame is pi-automations' `page.ts`: a title line, a blank, the body, a blank
//! and one hint line, which a message replaces for a few seconds.

mod list;
mod overlays;
mod pages;
mod run;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, Overlay, Screen};
use crate::text::{ellipsize, width};
use crate::theme::{Theme, UiColor};

/// A key hint. Lower `rank` is more important and stays longest as the terminal narrows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub text: String,
    pub rank: u8,
}

pub fn hint(text: impl Into<String>, rank: u8) -> Hint {
    Hint {
        text: text.into(),
        rank,
    }
}

pub struct Page {
    pub title: Line<'static>,
    pub right: Option<Line<'static>>,
    pub body: Vec<Line<'static>>,
    pub hints: Vec<Hint>,
    /// Kept on the right of the hint line whatever the width.
    pub pinned: Vec<String>,
}

/// How many body lines fit between the title and the hint line.
pub fn body_rows(rows: usize) -> usize {
    rows.saturating_sub(4)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    render(app, area, frame.buffer_mut());
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = area.width as usize;
    let rows = area.height as usize;
    let page = build(app, w, body_rows(rows));
    let t = &app.theme;
    let bottom = match &app.flash {
        Some(f) => Line::from(Span::styled(
            ellipsize(&f.text, w, app.glyphs.ellipsis),
            t.fg(f.tone),
        )),
        None => hint_line(&page.hints, &page.pinned, w, t),
    };
    let top = title_line(page.title, page.right, w);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows);
    if rows == 1 {
        lines.push(bottom);
    } else if rows <= 3 {
        lines.push(top);
        lines.extend((0..rows - 2).map(|_| Line::default()));
        lines.push(bottom);
    } else {
        let room = rows - 4;
        lines.push(top);
        lines.push(Line::default());
        let mut body = page.body;
        body.truncate(room);
        body.resize(room, Line::default());
        lines.extend(body);
        lines.push(Line::default());
        lines.push(bottom);
    }
    if app.glyphs == crate::glyphs::Glyphs::ASCII {
        for line in lines.iter_mut() {
            for span in line.spans.iter_mut() {
                span.content = crate::text::asciify(&span.content).into();
            }
        }
    }
    Paragraph::new(lines).render(area, buf);
}

/// The page for whatever is on top: an overlay, else the top screen.
fn build(app: &App, w: usize, rows: usize) -> Page {
    if let Some(o) = &app.overlay {
        return match o {
            Overlay::Consent {
                key,
                preview,
                scroll,
            } => overlays::consent(app, key, preview, *scroll, w, rows),
            Overlay::Confirm { key, what } => overlays::confirm(app, key, *what, w),
            Overlay::Agents { key, items, index } => {
                overlays::agents(app, key, items, *index, w, rows)
            }
            Overlay::Links { links, index } => overlays::links(app, links, *index, w, rows),
        };
    }
    match app.screen() {
        Screen::List => list::page(app, w, rows),
        Screen::Automation { key, index } => pages::automation(app, key, *index, w, rows),
        Screen::Run(p) => run::page(app, p, w, rows),
        Screen::Check { key, scroll } => pages::check(app, key, *scroll, w, rows),
        Screen::Help => pages::help(app, w, rows),
    }
}

fn line_width(l: &Line<'_>) -> usize {
    l.spans.iter().map(|s| width(&s.content)).sum()
}

/// `left` and `right` on one line, the right side dropped when they do not both fit.
fn title_line(left: Line<'static>, right: Option<Line<'static>>, w: usize) -> Line<'static> {
    let Some(right) = right else {
        return left;
    };
    let gap = w as isize - line_width(&left) as isize - line_width(&right) as isize;
    if gap < 3 {
        return left;
    }
    let mut spans = left.spans;
    spans.push(Span::raw(" ".repeat(gap as usize)));
    spans.extend(right.spans);
    Line::from(spans)
}

/// `r run now`: the key as it is, the words dim.
fn hint_spans(text: &str, t: &Theme) -> Vec<Span<'static>> {
    match text.split_once(' ') {
        Some((k, rest)) => vec![
            Span::styled(k.to_string(), t.fg(UiColor::Text)),
            Span::styled(format!(" {rest}"), t.fg(UiColor::Muted)),
        ],
        None => vec![Span::styled(text.to_string(), t.fg(UiColor::Muted))],
    }
}

/// As many hints as fit, the least important leaving first, the pinned ones never.
pub fn hint_line(hints: &[Hint], pinned: &[String], w: usize, t: &Theme) -> Line<'static> {
    const SEP: &str = " · ";
    let right_plain = pinned.join(SEP);
    let right_w = width(&right_plain);
    let mut kept: Vec<&Hint> = hints.iter().collect();
    let fits = |kept: &[&Hint]| {
        let left = kept.iter().map(|h| width(&h.text)).sum::<usize>()
            + kept.len().saturating_sub(1) * width(SEP);
        let gap = if left > 0 && right_w > 0 { 3 } else { 0 };
        left + gap + right_w <= w
    };
    while !fits(&kept) && !kept.is_empty() {
        let mut worst = 0;
        for (i, h) in kept.iter().enumerate() {
            if h.rank >= kept[worst].rank {
                worst = i;
            }
        }
        kept.remove(worst);
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, h) in kept.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(SEP, t.fg(UiColor::Muted)));
        }
        spans.extend(hint_spans(&h.text, t));
    }
    if pinned.is_empty() {
        return Line::from(spans);
    }
    let left_w: usize = spans.iter().map(|s| width(&s.content)).sum();
    let mut right: Vec<Span<'static>> = Vec::new();
    for (i, p) in pinned.iter().enumerate() {
        if i > 0 {
            right.push(Span::styled(SEP, t.fg(UiColor::Muted)));
        }
        right.extend(hint_spans(p, t));
    }
    if left_w + right_w < w {
        spans.push(Span::raw(" ".repeat(w - left_w - right_w)));
        spans.extend(right);
        Line::from(spans)
    } else {
        Line::from(right)
    }
}

fn marker(text: &str, app: &App) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {text}"),
        app.theme.fg(UiColor::Muted),
    ))
}

/// `room` lines of `lines` with `from..=to` in view, marking what is hidden.
pub fn window_block(
    lines: Vec<Line<'static>>,
    from: usize,
    to: usize,
    room: usize,
    app: &App,
) -> Vec<Line<'static>> {
    if lines.len() <= room {
        return lines;
    }
    if room < 3 {
        let start = from.min(lines.len().saturating_sub(room));
        return lines.into_iter().skip(start).take(room).collect();
    }
    let c = room - 2;
    let height = to.saturating_sub(from) + 1;
    let start = if height >= c {
        from as isize
    } else {
        from as isize - ((c - height).div_ceil(2)) as isize
    };
    let mut start = start.clamp(0, (lines.len() - c) as isize) as usize;
    let mut end = start + c;
    // A marker is only drawn where something is hidden, so its line goes to the list instead.
    if start == 0 {
        end += 1;
    } else if end >= lines.len() {
        start -= 1;
    }
    let n = lines.len();
    let mut out = Vec::with_capacity(room);
    if start > 0 {
        out.push(marker(app.glyphs.more_above, app));
    }
    out.extend(lines.into_iter().skip(start).take(end - start));
    if end < n {
        out.push(marker(app.glyphs.more_below, app));
    }
    out
}

/// The part of a tall page that starts at `scroll`, with markers for what is hidden. Records how far
/// the page can scroll, so the keys stop there.
pub fn scroll_block(
    lines: Vec<Line<'static>>,
    scroll: usize,
    room: usize,
    app: &App,
) -> Vec<Line<'static>> {
    if lines.len() <= room {
        app.scroll_max.set(0);
        return lines;
    }
    if room < 3 {
        app.scroll_max.set(lines.len() - room);
        let start = scroll.min(lines.len() - room);
        return lines.into_iter().skip(start).take(room).collect();
    }
    // The last screenful has a marker above and none below.
    app.scroll_max.set(lines.len() - (room - 1));
    let start = scroll.min(lines.len() - (room - 1));
    let above = start > 0;
    let mut take = room - usize::from(above);
    if start + take < lines.len() {
        take -= 1;
    }
    let n = lines.len();
    let mut out = Vec::with_capacity(room);
    if above {
        out.push(marker(app.glyphs.more_above, app));
    }
    out.extend(lines.into_iter().skip(start).take(take));
    if start + take < n {
        out.push(marker(app.glyphs.more_below, app));
    }
    out
}

/// Lines of `text` wrapped at `w - indent`, each indented and styled.
pub fn wrapped(text: &str, indent: usize, w: usize, style: Style) -> Vec<Line<'static>> {
    let room = w.saturating_sub(indent).max(8);
    crate::text::wrap(text, room)
        .into_iter()
        .map(|l| Line::from(Span::styled(format!("{}{l}", " ".repeat(indent)), style)))
        .collect()
}

/// `  label     value`: a muted label in a fixed column, then the value.
pub fn row(label: &str, label_w: usize, value: Vec<Span<'static>>, app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("  {}", crate::text::pad(label, label_w)),
        app.theme.fg(UiColor::Muted),
    )];
    spans.extend(value);
    Line::from(spans)
}

pub fn heading(text: &str, app: &App) -> Line<'static> {
    Line::from(Span::styled(format!("  {text}"), app.theme.bold()))
}

pub fn muted(text: impl Into<String>, app: &App) -> Span<'static> {
    Span::styled(text.into(), app.theme.fg(UiColor::Muted))
}

pub fn colored(text: impl Into<String>, color: UiColor, app: &App) -> Span<'static> {
    Span::styled(text.into(), app.theme.fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_leave_least_important_first() {
        let t = Theme::PLAIN;
        let hints = vec![
            hint("r run now", 1),
            hint("enter open", 2),
            hint("↑↓ move", 9),
        ];
        let pinned = vec!["? help".to_string(), "q quit".to_string()];
        let text = |w: usize| {
            hint_line(&hints, &pinned, w, &t)
                .spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(
            text(60),
            format!(
                "r run now · enter open · ↑↓ move{}? help · q quit",
                " ".repeat(13)
            )
        );
        assert!(text(40).starts_with("r run now · enter open"));
        assert!(!text(40).contains("move"));
        assert_eq!(text(15), "? help · q quit");
    }
}

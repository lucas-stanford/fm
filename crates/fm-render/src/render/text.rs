//! Text output. It never contains an escape code, so it pastes straight into a
//! Markdown fence.

use crate::error::Result;
use crate::flow::Flow;
use crate::render::decorate::{self, Mode};
use crate::render::rail;
use crate::render::theme::Theme;
use crate::state::State;

pub fn render(plan: &str, flow: &Flow, state: &State) -> Result<String> {
    // The theme only colours `classDef`s, which the ASCII renderer ignores.
    let source = decorate::decorate(plan, flow, state, Mode::Text, Theme::Light);
    // merman's text layout cannot route every flowchart yet; the rail draws any of them.
    Ok(match render_source(&source) {
        Some(text) => tighten(&text),
        None => rail::render(flow, state),
    })
}

/// merman's drawing, or `None` when it cannot draw this flowchart.
fn render_source(source: &str) -> Option<String> {
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer};

    let output = Renderer::new()
        .render(RenderRequest::ascii(
            source,
            OperationControl::new(),
            merman::AsciiRequest::default(),
        ))
        .ok()?;
    match output {
        RenderOutput::Ascii(Some(ascii)) => Some(ascii.text),
        _ => None,
    }
}

/// merman leaves a blank row inside every box and four rows between ranks. A row of nothing but
/// spaces and vertical lines only stretches what crosses it, so it can go without breaking an
/// edge: a lone one (the box padding) is dropped, and a longer run (a gap) is cut to one row.
fn tighten(text: &str) -> String {
    let stretch = |line: &str| line.chars().all(|c| c == ' ' || c == '│');
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if !stretch(lines[i]) {
            out.push(lines[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && stretch(lines[i]) {
            i += 1;
        }
        // Inside the drawing only: a trailing blank line is not padding.
        if i - start > 1 || i == lines.len() {
            out.push(lines[start]);
        }
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::fixture::{self, PLAN};

    fn graph(json: &str) -> String {
        render(PLAN, &fixture::flow(), &fixture::state(json)).expect("render")
    }

    #[test]
    fn the_text_graph_names_the_nodes_and_has_no_escape_codes() {
        let graph = graph("{}");
        assert!(!graph.contains('\u{1b}'));
        for label in ["Fetch source", "Build binary", "Fix and retry"] {
            assert!(graph.contains(label), "missing {label} in:\n{graph}");
        }
    }

    #[test]
    fn padding_rows_go_and_gaps_shrink_to_one_row() {
        let drawn =
            "┌───┐\n│   │\n│ A │\n│   │\n└─┬─┘\n  │  \n  │  \n  │  \n  ▼  \n┌───┐\n│ B │\n└───┘\n";
        assert_eq!(
            tighten(drawn),
            "┌───┐\n│ A │\n└─┬─┘\n  │  \n  ▼  \n┌───┐\n│ B │\n└───┘\n"
        );
    }

    #[test]
    fn a_top_down_chart_has_no_blank_rows_and_short_gaps() {
        let plan = "flowchart TD\n  a[Gather] --> b[Review]\n  b --> c[Post]\n";
        let flow = crate::flow::parse::parse(plan).expect("plan");
        let graph =
            render(plan, &flow, &State::parse("{}", &flow).expect("state")).expect("render");
        let lines: Vec<&str> = graph.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.contains('┌') {
                assert!(
                    lines[i + 1].chars().any(char::is_alphabetic),
                    "a blank row under a box top:\n{graph}"
                );
            }
        }
        let between = lines.iter().position(|l| l.contains('▼')).expect("arrow")
            - lines
                .iter()
                .position(|l| l.contains('┬'))
                .expect("edge start");
        assert_eq!(between, 2, "{graph}");
    }

    #[test]
    fn the_status_glyph_is_inline_on_the_label_line() {
        let graph = graph(r#"{"nodes": {"fetch": "done", "build": "failed"}}"#);
        let fetch = graph
            .lines()
            .find(|line| line.contains("Fetch source"))
            .expect("fetch line");
        assert!(fetch.contains('✓'), "{graph}");
        assert!(graph.contains('✗'), "{graph}");
        assert!(graph.contains('·'), "pending nodes carry the pending glyph");
    }
}

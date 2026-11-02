//! Turns a plan plus its state into decorated Mermaid.
//!
//! Append-only: the author's lines are copied through untouched and everything
//! generated follows a marker. This relies on a late `id@{ label: … }`
//! overriding the authored label (pinned in tests/merman_seam.rs).

use crate::flow::Flow;
use crate::render::theme::{self, Theme};
use crate::state::State;

const MARKER: &str = "%% fm:decoration";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `<br/>` puts the status glyph on its own line inside the node.
    Svg,
    /// One line: the ASCII renderer does not honour `<br/>`.
    Text,
}

pub fn decorate(plan: &str, flow: &Flow, state: &State, mode: Mode, theme: Theme) -> String {
    let mut out = String::with_capacity(plan.len() + 1024);

    out.push_str(plan.trim_end());
    out.push('\n');
    out.push_str("  ");
    out.push_str(MARKER);
    out.push('\n');

    for style in theme::palette(theme) {
        out.push_str(&format!(
            "  classDef {} fill:{},stroke:{},color:{}\n",
            style.class, style.fill, style.stroke, style.text
        ));
    }

    for node in &flow.nodes {
        let glyph = state.status(&node.id).glyph();
        let label = match mode {
            Mode::Svg => format!("{}<br/>{glyph}", escape(&node.label)),
            Mode::Text => format!("{} {glyph}", escape(&node.label)),
        };
        out.push_str(&format!("  {}@{{ label: \"{label}\" }}\n", node.id));
    }

    // Grouped by status so the output stays small on a large flowchart.
    for style in theme::palette(theme) {
        let ids: Vec<&str> = flow
            .nodes
            .iter()
            .filter(|node| state.status(&node.id) == style.status)
            .map(|node| node.id.as_str())
            .collect();
        if !ids.is_empty() {
            out.push_str(&format!("  class {} {}\n", ids.join(","), style.class));
        }
    }

    for (index, edge) in flow.edges.iter().enumerate() {
        if state.taken(&edge.from, &edge.to) {
            out.push_str(&format!(
                "  linkStyle {index} stroke:{},stroke-width:2px\n",
                theme::taken_edge_stroke(theme)
            ));
        }
    }

    out
}

/// Escapes what would otherwise become markup or terminate the label.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            // A newline would end the statement and make the rest parse as Mermaid.
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::fixture::{self, PLAN};

    fn decorated(json: &str, mode: Mode) -> String {
        decorate(
            PLAN,
            &fixture::flow(),
            &fixture::state(json),
            mode,
            Theme::Light,
        )
    }

    #[test]
    fn the_authors_source_is_copied_through_untouched() {
        let output = decorated("{}", Mode::Svg);
        let head = output.split(MARKER).next().expect("head");
        assert_eq!(head.trim_end(), PLAN.trim_end());
    }

    #[test]
    fn everything_generated_comes_after_the_marker() {
        let output = decorated(r#"{"taken": [["fetch", "build"]]}"#, Mode::Svg);
        let head = output.split(MARKER).next().expect("head");
        for generated in ["classDef", "class ", "linkStyle", "@{ label:"] {
            assert!(
                !head.contains(generated),
                "'{generated}' leaked above the marker"
            );
        }
    }

    #[test]
    fn every_node_gets_a_label_override_and_a_status_class() {
        let output = decorated(
            r#"{"nodes": {"fetch": "done", "build": "active"}}"#,
            Mode::Svg,
        );
        for id in ["fetch", "build", "test", "deploy", "fix"] {
            assert!(
                output.contains(&format!("{id}@{{ label:")),
                "no label for {id}"
            );
        }
        assert!(output.contains("class fetch fm_done"));
        assert!(output.contains("class build fm_active"));
        assert!(output.contains("class test,deploy,fix fm_pending"));
    }

    #[test]
    fn only_a_taken_edge_gets_a_linkstyle() {
        let output = decorated(r#"{"taken": [["fetch", "build"]]}"#, Mode::Svg);
        // fetch --> build is edge 0 in declaration order.
        assert!(output.contains("linkStyle 0 stroke:"));
        assert_eq!(output.matches("linkStyle").count(), 1);
    }

    #[test]
    fn a_linkstyle_index_follows_declaration_order() {
        // test -->|yes| deploy is the third declared edge.
        let output = decorated(r#"{"taken": [["test", "deploy"]]}"#, Mode::Svg);
        assert!(output.contains("linkStyle 2 stroke:"), "{output}");
    }

    #[test]
    fn text_mode_uses_one_line_and_no_markup() {
        let output = decorated(r#"{"nodes": {"fetch": "active"}}"#, Mode::Text);
        assert!(!output.contains("<br/>"));
        assert!(
            output.contains("fetch@{ label: \"Fetch source ▶\" }"),
            "{output}"
        );
    }

    #[test]
    fn a_hostile_label_cannot_escape_its_quotes() {
        let plan = "flowchart LR\n  a[\"x&quot; --> evil <b>\"] --> b\n";
        let flow = crate::flow::parse::parse(plan).expect("plan");
        let output = decorate(plan, &flow, &State::default(), Mode::Svg, Theme::Light);
        let label = output
            .lines()
            .find(|line| line.trim_start().starts_with("a@{"))
            .expect("label line");
        assert!(label.contains("&lt;b&gt;"), "{label}");
        assert_eq!(label.matches('"').count(), 2, "{label}");
    }

    #[test]
    fn escape_covers_every_character_that_breaks_a_label() {
        assert_eq!(escape("a\"b"), "a&quot;b");
        assert_eq!(escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(escape("a&b"), "a&amp;b");
        assert_eq!(escape("a\nb"), "a b");
        assert_eq!(escape("Ünïcödé ✓"), "Ünïcödé ✓");
    }

    #[test]
    fn the_dark_theme_changes_colours_but_not_structure() {
        let flow = fixture::flow();
        let state = State::default();
        let light = decorate(PLAN, &flow, &state, Mode::Svg, Theme::Light);
        let dark = decorate(PLAN, &flow, &state, Mode::Svg, Theme::Dark);
        assert_eq!(light.lines().count(), dark.lines().count());
        assert_ne!(light, dark);
        assert!(dark.contains("#2d2d2d"));
    }

    #[test]
    fn the_decorated_output_is_always_renderable() {
        let json =
            r#"{"nodes": {"fetch": "done", "build": "failed"}, "taken": [["fetch", "build"]]}"#;
        for mode in [Mode::Svg, Mode::Text] {
            let output = decorated(json, mode);
            crate::flow::parse::validate_renderable(&output)
                .unwrap_or_else(|error| panic!("{mode:?} output is not renderable: {error}"));
        }
    }
}

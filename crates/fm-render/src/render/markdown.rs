//! A plan with its node statuses as a Markdown ```mermaid block, for a page that draws Mermaid
//! itself (a GitHub issue or pull request, a wiki): each node filled in its status colour and each
//! arrow taken highlighted, with the author's labels as written.

use crate::flow::Flow;
use crate::render::decorate::{Mode, decorate};
use crate::render::theme::Theme;
use crate::state::State;

pub fn render(plan: &str, flow: &Flow, state: &State, theme: Theme) -> String {
    let source = decorate(plan, flow, state, Mode::Markdown, theme);
    // A fence longer than any run of backticks in the source, so nothing inside can close it.
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}mermaid\n{source}{fence}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::parse;
    use crate::render::fixture::{self, PLAN};

    fn markdown(json: &str) -> String {
        let flow = parse::parse(PLAN).unwrap();
        render(PLAN, &flow, &fixture::state(json), Theme::Light)
    }

    #[test]
    fn a_fenced_mermaid_block_with_statuses_and_no_label_overrides() {
        let md = markdown(
            r#"{"nodes": {"fetch": "done", "build": "active"}, "taken": [["fetch", "build"]]}"#,
        );
        assert!(
            md.starts_with("```mermaid\n") && md.ends_with("```\n"),
            "{md}"
        );
        assert!(md.contains("class fetch fm_done"), "{md}");
        assert!(md.contains("class build fm_active"), "{md}");
        assert!(md.contains("linkStyle 0 "), "{md}");
        assert!(!md.contains("@{"), "{md}");
        // The author's source comes through untouched.
        assert!(md.contains(PLAN.trim_end()), "{md}");
    }

    #[test]
    fn adds_only_statements_mermaid_has_long_supported() {
        let flow = parse::parse(PLAN).unwrap();
        let state =
            fixture::state(r#"{"nodes": {"fetch": "done"}, "taken": [["fetch", "build"]]}"#);
        let source = decorate(PLAN, &flow, &state, Mode::Markdown, Theme::Light);
        for line in source
            .lines()
            .skip_while(|l| !l.contains("fm:decoration"))
            .skip(1)
        {
            let t = line.trim();
            assert!(
                t.starts_with("classDef ")
                    || t.starts_with("class ")
                    || t.starts_with("linkStyle "),
                "{t}"
            );
        }
    }

    #[test]
    fn a_label_with_backticks_cannot_close_the_fence() {
        let plan = "flowchart TD\n  a[\"run ```rm```\"] --> b\n";
        let flow = parse::parse(plan).unwrap();
        let md = render(plan, &flow, &State::default(), Theme::Light);
        assert!(
            md.starts_with("````mermaid\n") && md.ends_with("````\n"),
            "{md}"
        );
    }
}

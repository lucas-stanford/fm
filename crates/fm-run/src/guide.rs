//! What the agent is told about reporting progress, appended to every body prompt (as
//! pi-automations' `notesGuide`/`progressGuide`, with fm's verbs and variables).

use fm_spec::{flow_order, parse_chart};

// Bodies ask for receipts in plain words; this ties those words to the commands.
const RECEIPTS_LINE: &str = "When the instructions ask you to record what you published or to leave a message, use these commands.";

/// One sentence: leave the reader one message with links before finishing.
fn message_line() -> &'static str {
    "Before you finish, leave the reader ONE message: `fm message \"what happened / what needs you\" --link \"label=https://…\"`. Links matter most; a later message replaces the earlier one."
}

/// For a run with no chart: notes and a message only.
pub fn notes_guide() -> String {
    [
        "<notes>",
        RECEIPTS_LINE,
        "If you publish anything or leave something for the reader, record it with `fm note \"what it is\" --link \"label=https://…\"` (AUTOMATION_RUN is already set).",
        message_line(),
        "If `fm` fails, carry on.",
        "</notes>",
    ]
    .join("\n")
}

/// For a run tracked on its flowchart: every verb, and the nodes in the order the run goes.
pub fn progress_guide(chart: &str, strict: bool) -> String {
    let ids: Vec<String> = flow_order(&parse_chart(chart))
        .into_iter()
        .map(|n| n.id)
        .collect();
    let nodes = format!("Nodes: {} (or their exact labels).", ids.join(", "));
    let mut lines: Vec<&str> = vec![
        "<progress>",
        "This run is tracked on its flowchart. Report where you are with `fm` (AUTOMATION_RUN is already set):",
        "- `fm step <node> [-m \"text\"] [--status active|waiting|done|failed|skipped]` — set one node (default active)",
        "- `fm next <to> --from <node> [-m \"text\"]` — finish <node> and start <to>",
        "- `fm say \"text\" [--node <node>]` — add an update to a node",
        "- `fm tasks \"first\" \"second\" … [--node <step>]` — break a step into its tasks, in order",
        "- `fm task <n> [--status done|failed|skipped] [-m \"text\"]` — start task n (by default), or finish it",
        "- `fm note \"text\" --link \"label=https://…\"` — a note on the run's record, with the links you published",
        "- `fm status` — where the run is; any command takes `--json`",
        &nodes,
    ];
    if strict {
        lines.push(
            "Each `##` section is one step: start it with `fm next <id> --from <the step before>`, following the chart's arrows.",
        );
    }
    lines.push(
        "Break a step that takes more than a few minutes into its tasks before you start them, and mark each as it starts and ends: the run's flowchart shows them.",
    );
    lines.extend([
        "Subagents inherit AUTOMATION_RUN; set AUTOMATION_NODE=<its node> for each so its `fm say`/`fm next` land there.",
        RECEIPTS_LINE,
        message_line(),
        "If `fm` fails, carry on.",
        "</progress>",
    ]);
    lines.join("\n")
}

/// For an automation with `nodes:`: which steps run through their own agent, and how to hand one
/// over. `None` when it has none.
pub fn nodes_guide(config: &fm_model::Config, l: &fm_model::Located) -> Option<String> {
    if l.automation.nodes.is_empty() {
        return None;
    }
    let mut lines = vec![
        "<nodes>".to_string(),
        "These steps run through their own agent. When the run reaches one, start it with `fm next` as usual, then hand it over: `fm node <id> -m \"what it needs from you\"` (paths, findings, what to do; add `--cwd <folder>` to run it in another folder). It waits, then prints the step's answer. Go on from that answer as the chart says. If `fm node` fails, say so with `fm say` and do what the step's section says for a failure.".to_string(),
    ];
    for n in &l.automation.nodes {
        lines.push(match crate::node::plan_node(config, l, n) {
            Ok(p) => format!("- {}: {} ({})", n.id, p.cli, p.model.display),
            Err(e) => format!("- {}: cannot run here — {e}", n.id),
        });
    }
    lines.push("</nodes>".to_string());
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = "flowchart TD\n  pre0[\"Pick\"] --> body[\"Verify\"]\n  body --> read[\"Read\"]\n  read --> reproduce\n  read -->|no browser| draft\n  reproduce --> draft\n  draft --> post0\n";

    #[test]
    fn lists_the_nodes_in_flow_order() {
        let g = progress_guide(CHART, false);
        assert!(g.starts_with("<progress>\n"));
        assert!(g.ends_with("\n</progress>"));
        assert!(
            g.contains("Nodes: pre0, body, read, reproduce, draft, post0 (or their exact labels).")
        );
        for verb in ["step", "next", "say", "note", "status"] {
            assert!(g.contains(&format!("`fm {verb}")), "{verb}");
        }
        assert!(g.contains("`fm message "));
        assert!(!g.contains("pa "));
    }

    #[test]
    fn adds_one_line_and_only_in_strict_mode() {
        let strict: Vec<String> = progress_guide(CHART, true)
            .lines()
            .map(String::from)
            .collect();
        let loose: Vec<String> = progress_guide(CHART, false)
            .lines()
            .map(String::from)
            .collect();
        assert_eq!(strict.len(), loose.len() + 1);
        let extra: Vec<&String> = strict.iter().filter(|l| !loose.contains(l)).collect();
        assert_eq!(
            extra,
            [
                "Each `##` section is one step: start it with `fm next <id> --from <the step before>`, following the chart's arrows."
            ]
        );
    }

    #[test]
    fn the_notes_guide_names_note_and_message() {
        let g = notes_guide();
        assert!(g.starts_with("<notes>\n") && g.ends_with("\n</notes>"));
        assert!(g.contains("`fm note "));
        assert!(g.contains("`fm message "));
        assert!(!g.contains("<progress>"));
    }

    #[test]
    fn nodes_are_listed_with_their_agent_and_how_to_hand_one_over() {
        let text = "---\nname: demo\ndescription: d\nflow: strict\nnodes:\n  review: codex\non:\n  manual: true\n---\n\n```mermaid\nflowchart TD\n  build[\"Build\"] --> review[\"Review\"]\n```\n\n## Build {#build}\n\nB.\n\n## Review {#review}\n\nR.\n";
        let (automation, _) = fm_spec::parse_automation(text, "demo").unwrap();
        let l = fm_model::Located {
            automation,
            dir: "/a/demo".into(),
            file: "/a/demo/AUTOMATION.md".into(),
            root: fm_model::Root {
                path: "/a".into(),
                scope: fm_model::Scope::User,
            },
            repo_root: None,
            key: "user:/a:demo".into(),
        };
        let g = nodes_guide(&fm_model::Config::default(), &l).unwrap();
        assert!(
            g.starts_with("<nodes>\n") && g.ends_with("\n</nodes>"),
            "{g}"
        );
        assert!(g.contains("`fm node <id> -m"), "{g}");
        assert!(g.contains("- review: codex (codex default)"), "{g}");
        let mut plain = l.clone();
        plain.automation.nodes.clear();
        assert!(nodes_guide(&fm_model::Config::default(), &plain).is_none());
    }
}

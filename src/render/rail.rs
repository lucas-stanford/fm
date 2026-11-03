//! The text layout fm falls back to when merman cannot route a flowchart: every node in one
//! column, in flow order, whatever direction the plan asks for. The arrow from a node to the one
//! drawn under it runs between the two boxes. Every other arrow is written beside its source and
//! points up (`▲`) or down (`▼`) to the node it reaches, or round (`↺`) to its own. A taken arrow
//! is drawn heavy. It always fits a narrow pane, however many arrows cross.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use unicode_width::UnicodeWidthStr;

use crate::flow::{Edge, Flow};
use crate::state::State;

pub fn render(flow: &Flow, state: &State) -> String {
    let index: HashMap<&str, usize> = flow
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id.as_str(), i))
        .collect();
    let order = order(flow, &index);
    let mut row_of = vec![0; flow.nodes.len()];
    for (row, &node) in order.iter().enumerate() {
        row_of[node] = row;
    }

    let titles: Vec<String> = flow
        .nodes
        .iter()
        .map(|node| format!("{} {}", node.label, state.status(&node.id).glyph()))
        .collect();
    // One space either side of the widest title; every box is as wide, so the rail is straight.
    let inner = titles.iter().map(|t| t.width()).max().unwrap_or(0) + 2;
    let middle = inner / 2;

    let mut out = String::new();
    for (row, &node) in order.iter().enumerate() {
        let id = flow.nodes[node].id.as_str();
        let below = order.get(row + 1).copied();
        let mut rail = None;
        let mut notes = Vec::new();
        for edge in flow.edges.iter().filter(|edge| edge.from == id) {
            let to = index[edge.to.as_str()];
            if rail.is_none() && Some(to) == below {
                rail = Some(edge);
                continue;
            }
            let towards = match row_of[to] {
                r if r < row => '▲',
                r if r > row => '▼',
                _ => '↺',
            };
            notes.push(format!(
                "{}{towards} {}{}",
                stroke(state, edge, "──", "━━"),
                flow.nodes[to].label,
                edge.label
                    .as_ref()
                    .map(|label| format!(" ({label})"))
                    .unwrap_or_default()
            ));
        }

        out.push_str(&format!("┌{}┐\n", "─".repeat(inner)));
        let title = &titles[node];
        let left = (inner - title.width()) / 2;
        for line in 0..notes.len().max(1) {
            let content = if line == 0 {
                format!(
                    "{}{title}{}",
                    " ".repeat(left),
                    " ".repeat(inner - title.width() - left)
                )
            } else {
                " ".repeat(inner)
            };
            let note = notes.get(line).map(String::as_str).unwrap_or("");
            out.push_str(&format!("│{content}│{note}\n"));
        }
        match rail {
            Some(edge) => {
                let pad = " ".repeat(middle + 1);
                out.push_str(&format!(
                    "└{}┬{}┘\n",
                    "─".repeat(middle),
                    "─".repeat(inner - middle - 1)
                ));
                let label = edge
                    .label
                    .as_ref()
                    .map(|l| format!(" {l}"))
                    .unwrap_or_default();
                out.push_str(&format!("{pad}{}{label}\n", stroke(state, edge, "│", "┃")));
                out.push_str(&format!("{pad}▼\n"));
            }
            None => {
                out.push_str(&format!("└{}┘\n", "─".repeat(inner)));
                if below.is_some() {
                    out.push('\n');
                }
            }
        }
    }
    out
}

fn stroke<'a>(state: &State, edge: &Edge, plain: &'a str, taken: &'a str) -> &'a str {
    if state.taken(&edge.from, &edge.to) {
        taken
    } else {
        plain
    }
}

/// Node indices in flow order: a topological order of the arrows that do not close a cycle, ties
/// going to the node mentioned first.
fn order(flow: &Flow, index: &HashMap<&str, usize>) -> Vec<usize> {
    let count = flow.nodes.len();
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); count];
    for edge in &flow.edges {
        next[index[edge.from.as_str()]].push(index[edge.to.as_str()]);
    }

    // An arrow is a back edge when it reaches a node still open in a depth-first walk that
    // starts from each node in order of first mention and follows arrows in source order.
    const NEW: u8 = 0;
    const OPEN: u8 = 1;
    const CLOSED: u8 = 2;
    let mut mark = vec![NEW; count];
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); count];
    for root in 0..count {
        if mark[root] != NEW {
            continue;
        }
        mark[root] = OPEN;
        let mut stack = vec![(root, 0)];
        while let Some((node, at)) = stack.last_mut() {
            let node = *node;
            let Some(&to) = next[node].get(*at) else {
                mark[node] = CLOSED;
                stack.pop();
                continue;
            };
            *at += 1;
            if mark[to] == OPEN {
                continue;
            }
            forward[node].push(to);
            if mark[to] == NEW {
                mark[to] = OPEN;
                stack.push((to, 0));
            }
        }
    }

    let mut waiting = vec![0; count];
    for targets in &forward {
        for &to in targets {
            waiting[to] += 1;
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = (0..count)
        .filter(|&n| waiting[n] == 0)
        .map(Reverse)
        .collect();
    let mut order = Vec::with_capacity(count);
    while let Some(Reverse(node)) = ready.pop() {
        order.push(node);
        for &to in &forward[node] {
            waiting[to] -= 1;
            if waiting[to] == 0 {
                ready.push(Reverse(to));
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::parse::parse;

    fn drawn(plan: &str, state: &str) -> String {
        let flow = parse(plan).expect("plan");
        render(&flow, &State::parse(state, &flow).expect("state"))
    }

    fn titled(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|line| line.starts_with('│'))
            .map(|line| line.split('│').nth(1).unwrap_or("").trim())
            .filter(|title| !title.is_empty())
            .collect()
    }

    #[test]
    fn nodes_run_down_one_column_in_flow_order() {
        // `done` is mentioned second but reached last.
        let text = drawn(
            "flowchart LR\n  a[Start] -->|skip| done[Done]\n  a --> b[Work] --> done\n",
            "{}",
        );
        assert_eq!(titled(&text), ["Start ·", "Work ·", "Done ·"]);
    }

    #[test]
    fn the_arrow_to_the_next_node_runs_between_the_boxes_and_the_rest_are_notes() {
        let text = drawn(
            "flowchart TD\n  a[Read] -->|nothing to do| c[Report]\n  a --> b[Work]\n  \
             b -->|again| a\n  b --> c\n",
            "{}",
        );
        assert_eq!(
            text,
            "┌──────────┐\n\
             │  Read ·  │──▼ Report (nothing to do)\n\
             └─────┬────┘\n      \
             │\n      \
             ▼\n\
             ┌──────────┐\n\
             │  Work ·  │──▲ Read (again)\n\
             └─────┬────┘\n      \
             │\n      \
             ▼\n\
             ┌──────────┐\n\
             │ Report · │\n\
             └──────────┘\n"
        );
    }

    #[test]
    fn taken_arrows_are_heavy_and_a_rail_label_sits_beside_the_rail() {
        let text = drawn(
            "flowchart TD\n  a[Check] -->|ok| b[Ship]\n  a -->|no| c[Fix]\n",
            r#"{"nodes": {"a": "done", "b": "active"}, "taken": [["a", "b"]]}"#,
        );
        assert!(text.contains("┃ ok"), "{text}");
        assert!(text.contains("│ Check ✓ │──▼ Fix (no)\n"), "{text}");
        let text = drawn(
            "flowchart TD\n  a[Check] -->|ok| b[Ship]\n  a -->|no| c[Fix]\n",
            r#"{"taken": [["a", "c"]]}"#,
        );
        assert!(text.contains("━━▼ Fix (no)"), "{text}");
        assert!(text.contains("│ ok"), "{text}");
    }

    #[test]
    fn a_node_with_several_notes_grows_a_row_for_each() {
        let text = drawn(
            "flowchart TD\n  a[Gate] --> b[Next]\n  a -->|x| c[One]\n  a -->|y| d[Two]\n  \
             a --> a\n",
            "{}",
        );
        let gate: Vec<&str> = text.lines().skip(1).take(3).collect();
        assert!(gate[0].ends_with("──▼ One (x)"), "{text}");
        assert!(gate[1].ends_with("──▼ Two (y)"), "{text}");
        assert!(gate[2].ends_with("──↺ Gate"), "{text}");
    }

    #[test]
    fn unconnected_neighbours_are_separated_by_a_blank_line() {
        let text = drawn("flowchart TD\n  a[A]\n  b[B]\n", "{}");
        assert_eq!(
            text,
            "┌─────┐\n│ A · │\n└─────┘\n\n┌─────┐\n│ B · │\n└─────┘\n"
        );
    }

    #[test]
    fn wide_characters_keep_the_boxes_square() {
        let text = drawn("flowchart TD\n  a[\"漢字\"] --> b[B]\n", "{}");
        let widths: Vec<usize> = text
            .lines()
            .filter(|line| line.starts_with(['┌', '│', '└']))
            .map(UnicodeWidthStr::width)
            .collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{text}");
    }
}

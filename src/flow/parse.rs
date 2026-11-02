//! A strict parser for the flowchart subset in docs/SUPPORTED_MERMAID.md.
//!
//! It reports unsupported input with a line:col; whatever it accepts is also
//! checked through merman by [`validate_renderable`].

use crate::error::{Code, Failure, Result, fail};
use crate::flow::{Edge, Flow, Node};

pub const MAX_PLAN_BYTES: usize = 256 * 1024;
pub const MAX_NODES: usize = 500;

const DIRECTIONS: &[&str] = &["TD", "TB", "BT", "LR", "RL"];

/// Longest first, so `-.-` cannot match a prefix of `-.->`.
const ARROWS: &[&str] = &["-.->", "-.-", "==>", "===", "-->", "---", "--o", "--x"];

/// (open, close). Longest opener first, so `[[` is tried before `[`.
const SHAPES: &[(&str, &str)] = &[
    ("([", "])"),
    ("[[", "]]"),
    ("[(", ")]"),
    ("((", "))"),
    ("{{", "}}"),
    ("[/", "/]"),
    ("[/", "\\]"),
    ("[\\", "\\]"),
    ("[\\", "/]"),
    ("[", "]"),
    ("(", ")"),
    (">", "]"),
    ("{", "}"),
];

/// Styling is how fm draws statuses, so a plan may not carry its own.
const RESERVED: &[&str] = &["classDef", "class", "linkStyle", "style", "click"];

/// Characters that cannot be in a node id but are easy to type in one; seeing
/// one right after an id gets a specific error instead of "expected an arrow".
const BAD_IN_ID: &[char] = &[
    '.', ':', '@', '#', '$', '&', '*', '+', '!', '?', ',', ';', '\'', '`', '~', '^', '/',
];

struct Line<'a> {
    raw: &'a str,
    number: usize,
}

struct NodeRef {
    id: String,
    label: String,
    shaped: bool,
    consumed: usize,
}

pub fn parse(text: &str) -> Result<Flow> {
    if text.len() > MAX_PLAN_BYTES {
        return Err(fail!(
            Code::Plan,
            "the flowchart is {} bytes; the limit is {MAX_PLAN_BYTES}",
            text.len()
        ));
    }
    let mut flow = Flow::default();
    let mut open_subgraphs: Vec<String> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let at = Line {
            raw,
            number: index + 1,
        };
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = keyword(line, "flowchart").or_else(|| keyword(line, "graph")) {
            let token = rest.trim();
            if !token.is_empty() {
                check_direction(token, &at)?;
            }
        } else if let Some(rest) = keyword(line, "subgraph") {
            open_subgraphs.push(subgraph_id(rest, &at)?);
        } else if line == "end" {
            if open_subgraphs.pop().is_none() {
                return Err(unsupported(&at, 0, "'end' without an open subgraph"));
            }
        } else if let Some(rest) = keyword(line, "direction") {
            check_direction(rest.trim(), &at)?;
        } else if let Some(directive) = RESERVED.iter().find(|d| keyword(line, d).is_some()) {
            return Err(unsupported(
                &at,
                0,
                format!("'{directive}' is reserved for fm, which uses it to draw statuses"),
            ));
        } else {
            statement(&mut flow, line, &at)?;
        }
    }

    if let Some(unclosed) = open_subgraphs.pop() {
        return Err(fail!(
            Code::Plan,
            "subgraph '{unclosed}' is never closed with 'end'"
        ));
    }
    if flow.nodes.len() > MAX_NODES {
        return Err(fail!(
            Code::Plan,
            "the flowchart has {} nodes; the limit is {MAX_NODES}",
            flow.nodes.len()
        ));
    }
    Ok(flow)
}

/// Cross-checks a plan through merman, so a plan merman cannot read is reported
/// as a plan error rather than as a render failure.
pub fn validate_renderable(mermaid: &str) -> Result<()> {
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer};

    let outcome = Renderer::new().render(RenderRequest::semantic(mermaid, OperationControl::new()));
    match outcome {
        Ok(RenderOutput::Semantic(Some(_))) => Ok(()),
        Ok(_) => Err(fail!(Code::Plan, "the renderer found no flowchart")),
        Err(error) => Err(fail!(
            Code::Plan,
            "the renderer rejected the flowchart: {error}"
        )),
    }
}

fn check_direction(token: &str, at: &Line<'_>) -> Result<()> {
    if DIRECTIONS.contains(&token) {
        return Ok(());
    }
    Err(unsupported(
        at,
        column_of(at.raw, token),
        format!("'{token}' is not a direction (TD, TB, BT, LR, RL)"),
    ))
}

fn subgraph_id(rest: &str, at: &Line<'_>) -> Result<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(unsupported(at, 0, "subgraph needs an id"));
    }
    let id = match split_shape(rest) {
        Some(node) => node.id,
        None => rest.to_string(),
    };
    validate_node_id(&id, at)?;
    Ok(id)
}

/// Parses a chain, `a[A] -->|yes| b(B) --> c`, recording one edge per arrow.
fn statement(flow: &mut Flow, line: &str, at: &Line<'_>) -> Result<()> {
    let first = node_ref(line, at)?;
    let mut rest = line[first.consumed..].trim_start();
    let mut source = record_node(flow, first);

    while !rest.is_empty() {
        let Some(consumed) = arrow_len(rest) else {
            return Err(unsupported(
                at,
                column_of(at.raw, rest),
                format!(
                    "expected an arrow (-->, ---, -.->, ==>, --o, --x) or end of line, \
                     found '{}'",
                    first_token(rest)
                ),
            ));
        };
        rest = rest[consumed..].trim_start();
        if rest.is_empty() {
            return Err(unsupported(
                at,
                column_of(at.raw, rest),
                "an arrow must be followed by a node",
            ));
        }
        let target = node_ref(rest, at)?;
        rest = rest[target.consumed..].trim_start();
        let target = record_node(flow, target);
        flow.edges.push(Edge {
            from: source,
            to: target.clone(),
        });
        source = target;
    }
    Ok(())
}

fn node_ref(rest: &str, at: &Line<'_>) -> Result<NodeRef> {
    if let Some(node) = split_shape(rest) {
        return Ok(node);
    }
    let length = id_len(rest);
    if length == 0 {
        return Err(unsupported(
            at,
            column_of(at.raw, rest),
            format!("expected a node, found '{}'", first_token(rest)),
        ));
    }
    if rest[length..].starts_with(BAD_IN_ID) {
        return Err(bad_id(first_token(rest), at, column_of(at.raw, rest)));
    }
    let id = &rest[..length];
    Ok(NodeRef {
        id: id.to_string(),
        label: id.to_string(),
        shaped: false,
        consumed: length,
    })
}

/// Adds a node on first mention and returns its id. A shaped definition wins
/// over a bare mention, whichever comes first.
fn record_node(flow: &mut Flow, node: NodeRef) -> String {
    if let Some(existing) = flow.nodes.iter_mut().find(|n| n.id == node.id) {
        if !existing.shaped && node.shaped {
            existing.shaped = true;
            existing.label = node.label;
        }
        return node.id;
    }
    flow.nodes.push(Node {
        id: node.id.clone(),
        label: node.label,
        shaped: node.shaped,
    });
    node.id
}

fn unsupported(at: &Line<'_>, column: usize, why: impl Into<String>) -> Failure {
    let why = why.into();
    fail!(Code::Plan, "{}:{}: {why}", at.number, column + 1)
}

fn bad_id(id: &str, at: &Line<'_>, column: usize) -> Failure {
    unsupported(
        at,
        column,
        format!(
            "node id '{id}' may contain only ASCII letters, digits, '-' and '_' \
             (put punctuation in the label instead)"
        ),
    )
}

fn validate_node_id(id: &str, at: &Line<'_>) -> Result<()> {
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(bad_id(id, at, column_of(at.raw, id)));
    }
    Ok(())
}

/// Length of the node id at the start of `text`. A '-' belongs to the id unless
/// it starts an arrow, so `my-node` is one id and `a-->b` is two.
fn id_len(text: &str) -> usize {
    let mut length = 0;
    for byte in text.bytes() {
        let in_id = byte.is_ascii_alphanumeric()
            || byte == b'_'
            || (byte == b'-'
                && !text[length..].starts_with("--")
                && !text[length..].starts_with("-."));
        if !in_id {
            break;
        }
        length += 1;
    }
    length
}

/// Splits `id[Label]`; `None` when `rest` does not start with a shaped node.
fn split_shape(rest: &str) -> Option<NodeRef> {
    let open_at = id_len(rest);
    if open_at == 0 || open_at == rest.len() {
        return None;
    }
    let tail = &rest[open_at..];

    // `[/…/]` and `[/…\]` share an opener, so the shape that closes first wins;
    // on a tie the earlier (longer) opener wins, so `[[x]]` is not `[` + `[x]`.
    let mut best: Option<(usize, &str, &str)> = None;
    for (open, close) in SHAPES {
        if !tail.starts_with(open) {
            continue;
        }
        let Some(body_end) = find_close(&tail[open.len()..], close) else {
            continue;
        };
        let end = body_end + open.len();
        if best.is_none_or(|(current, _, _)| end < current) {
            best = Some((end, open, close));
        }
    }
    let (body_end, open, close) = best?;
    Some(NodeRef {
        id: rest[..open_at].to_string(),
        label: unquote(&tail[open.len()..body_end]),
        shaped: true,
        consumed: open_at + body_end + close.len(),
    })
}

/// Finds `close`, skipping any inside a quoted label, so `a["[all]"]` closes at
/// the last bracket.
fn find_close(body: &str, close: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut in_quotes = false;
    for index in 0..bytes.len() {
        if bytes[index] == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes && bytes[index..].starts_with(close.as_bytes()) {
            return Some(index);
        }
    }
    None
}

fn unquote(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    trimmed.to_string()
}

/// Length of the arrow at the start of `rest`, including a label written as
/// `-->|text|` or `-- text -->`.
fn arrow_len(rest: &str) -> Option<usize> {
    if let Some(inline) = rest.strip_prefix("--")
        && !inline.starts_with(['-', '>', 'o', 'x', '.'])
    {
        for token in ARROWS {
            if let Some(end) = inline.find(token)
                && !inline[..end].trim().is_empty()
            {
                return Some(2 + end + token.len());
            }
        }
    }
    let token = ARROWS.iter().find(|token| rest.starts_with(**token))?;
    let after = &rest[token.len()..];
    if let Some(pipe) = after.strip_prefix('|')
        && let Some(end) = pipe.find('|')
    {
        return Some(token.len() + 1 + end + 1);
    }
    Some(token.len())
}

fn first_token(rest: &str) -> &str {
    rest.split_whitespace().next().unwrap_or("")
}

/// Strips a leading keyword, requiring a word break so `classy` is not `class`.
fn keyword<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(word)?;
    match rest.chars().next() {
        Some(next) if next.is_ascii_alphanumeric() || next == '_' => None,
        _ => Some(rest),
    }
}

/// 0-based column, in characters, of `needle` inside `haystack`. `needle` must
/// be a slice of `haystack`; otherwise the line's indentation is used.
fn column_of(haystack: &str, needle: &str) -> usize {
    let base = haystack.as_ptr() as usize;
    let at = needle.as_ptr() as usize;
    if at >= base && at <= base + haystack.len() {
        haystack[..at - base].chars().count()
    } else {
        haystack.len() - haystack.trim_start().len()
    }
}

/// Removes a trailing `%%` comment, keeping a `%%` inside a quoted label.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    for index in 0..bytes.len() {
        match bytes[index] {
            b'"' => in_quotes = !in_quotes,
            b'%' if !in_quotes && bytes.get(index + 1) == Some(&b'%') => return &line[..index],
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(flow: &Flow) -> Vec<&str> {
        flow.nodes.iter().map(|node| node.id.as_str()).collect()
    }

    fn labels(flow: &Flow) -> Vec<&str> {
        flow.nodes.iter().map(|node| node.label.as_str()).collect()
    }

    fn edges(flow: &Flow) -> Vec<(&str, &str)> {
        flow.edges
            .iter()
            .map(|edge| (edge.from.as_str(), edge.to.as_str()))
            .collect()
    }

    fn rejected(plan: &str) -> Failure {
        let error = parse(plan).expect_err("must reject");
        assert_eq!(error.code, Code::Plan);
        error
    }

    #[test]
    fn nodes_and_edges_are_kept_in_source_order() {
        let flow = parse(
            "flowchart LR\n  \
             fetch[Fetch source] --> build[Build binary] --> test{Tests pass?}\n  \
             test -->|yes| deploy[Deploy]\n  \
             test -->|no| fix[Fix and retry] --> build\n  \
             a-->b\n",
        )
        .expect("parse");
        assert_eq!(
            ids(&flow),
            ["fetch", "build", "test", "deploy", "fix", "a", "b"]
        );
        assert_eq!(flow.node("test").unwrap().label, "Tests pass?");
        assert_eq!(
            edges(&flow),
            [
                ("fetch", "build"),
                ("build", "test"),
                ("test", "deploy"),
                ("test", "fix"),
                ("fix", "build"),
                ("a", "b"),
            ]
        );
    }

    #[test]
    fn every_supported_shape_yields_its_label() {
        let flow = parse(
            "flowchart TD\n a[r] --> b(ro) --> c([st]) --> d[[sub]] --> e[(cyl)]\n \
             f((ci)) --> g>as] --> h{rh} --> i{{hex}} --> j[/par/]\n \
             k[\\alt\\] --> l[/tra\\] --> m[\\tal/] --> my-node\n",
        )
        .expect("parse");
        assert_eq!(
            labels(&flow),
            [
                "r", "ro", "st", "sub", "cyl", "ci", "as", "rh", "hex", "par", "alt", "tra", "tal",
                "my-node"
            ]
        );
    }

    #[test]
    fn every_supported_arrow_and_label_form_is_an_edge() {
        let flow = parse(
            "flowchart LR\n a --> b\n b --- c\n c -.-> d\n d -.- e\n \
             e ==> f\n f === g\n g --o h\n h --x i\n i -->|yes| j\n j -- maybe --> k\n",
        )
        .expect("parse");
        assert_eq!(flow.edges.len(), 10);
        assert_eq!(edges(&flow)[9], ("j", "k"));
    }

    #[test]
    fn a_quoted_label_may_contain_delimiters_and_comment_markers() {
        let flow = parse(
            "flowchart LR\n  %% a comment\n  a[\"Tests [all] pass?\"] --> b[B] %% trailing\n  \
             c[\"100%% done\"]\n",
        )
        .expect("parse");
        assert_eq!(labels(&flow), ["Tests [all] pass?", "B", "100%% done"]);
    }

    #[test]
    fn subgraphs_and_directions_are_accepted() {
        let flow = parse(
            "flowchart TD\n subgraph prep[Prepare]\n direction LR\n a[A] --> b[B]\n end\n \
             b --> c[C]\n",
        )
        .expect("parse");
        assert_eq!(ids(&flow), ["a", "b", "c"]);
    }

    #[test]
    fn a_shaped_definition_wins_over_a_bare_mention() {
        let flow = parse("flowchart LR\n a[Authored] --> b\n b --> a\n b[Later]\n").expect("parse");
        assert_eq!(labels(&flow), ["Authored", "Later"]);
    }

    #[test]
    fn styling_directives_are_reserved() {
        for directive in [
            "classDef x fill:#fff",
            "class a x",
            "linkStyle 0 stroke:#000",
            "style a fill:#fff",
            "click a href \"https://example.com\"",
        ] {
            let error = rejected(&format!("flowchart LR\n a --> b\n {directive}\n"));
            assert!(error.message.contains("reserved for fm"), "{directive}");
        }
        assert!(parse("flowchart LR\n classy --> b\n").is_ok());
    }

    #[test]
    fn errors_carry_line_and_column() {
        assert!(
            rejected("flowchart LR\n a --> b\n c ~~> d\n")
                .message
                .starts_with("3:4:")
        );
        let error = rejected("flowchart SIDEWAYS\n a --> b\n");
        assert!(error.message.starts_with("1:11:") && error.message.contains("SIDEWAYS"));
        // Columns count characters, not bytes.
        let error = rejected("flowchart LR\n a[Ünïcödé] ~~> b\n");
        assert!(error.message.starts_with("2:13:"), "{}", error.message);
    }

    #[test]
    fn malformed_structure_is_rejected() {
        assert!(
            rejected("flowchart TD\n subgraph s1[S]\n a --> b\n")
                .message
                .contains("never closed")
        );
        rejected("flowchart TD\n a --> b\n end\n");
        assert!(
            rejected("flowchart LR\n a --> \n")
                .message
                .contains("followed by a node")
        );
        assert!(
            rejected("flowchart LR\n a.b --> c\n")
                .message
                .contains("put punctuation in the label")
        );
    }

    #[test]
    fn oversized_plans_are_rejected() {
        let huge = format!("flowchart LR\n{}", "  a --> b\n".repeat(30_000));
        assert!(rejected(&huge).message.contains("bytes"));
        let many: String = (0..=MAX_NODES).map(|i| format!("  n{i}\n")).collect();
        assert!(
            rejected(&format!("flowchart LR\n{many}"))
                .message
                .contains("nodes")
        );
    }

    #[test]
    fn everything_the_parser_accepts_is_renderable_by_merman() {
        for plan in [
            "flowchart LR\n fetch[Fetch] --> build[Build] --> test{Ok?}\n test -->|yes| done[Done]\n",
            "flowchart TD\n subgraph s1[Prep]\n a[A] --> b[B]\n end\n b --> c[C]\n",
            "flowchart LR\n a --> b --> c --> a\n",
            "flowchart LR\n a[\"Ünïcödé ✓\"] --> b((round))\n",
        ] {
            parse(plan).expect("fm accepts it");
            validate_renderable(plan).expect("merman accepts it");
        }
    }
}

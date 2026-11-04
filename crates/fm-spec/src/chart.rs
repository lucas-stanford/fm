//! Just enough of a Mermaid flowchart to know its nodes and arrows; everything else is skipped.
//! Also the step ids shared by the chart, the run and the run page, and the order a run goes in.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use fancy_regex::Regex;
use fm_model::{Automation, Hook};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartNode {
    pub id: String,
    /// The label, or the id for a node without one.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chart {
    /// In order of first appearance.
    pub nodes: Vec<ChartNode>,
    pub edges: Vec<ChartEdge>,
    /// Ids of `subgraph id [...]` groups. An arrow to one makes it a node too; it is not a step.
    pub subgraphs: Vec<String>,
}

pub(crate) fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid built-in pattern")
}

/// Opening and closing delimiters, longest first so `((` wins over `(`.
const SHAPES: [(&str, &[&str]); 12] = [
    ("(((", &[")))"]),
    ("((", &["))"]),
    ("([", &["])"]),
    ("[[", &["]]"]),
    ("[(", &[")]"]),
    ("{{", &["}}"]),
    ("[/", &["/]", "\\]"]),
    ("[\\", &["\\]", "/]"]),
    ("[", &["]"]),
    ("(", &[")"]),
    ("{", &["}"]),
    (">", &["]"]),
];

static SKIP: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(subgraph|end|classDef|class|style|linkStyle|click|direction|accTitle|accDescr)(?![A-Za-z0-9_])",
    )
});
static HEADER: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(flowchart|graph)(?![A-Za-z0-9_])"));
static ID: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[\p{L}\p{N}_]+(?:[-.](?![-.>=])[\p{L}\p{N}_]+)*"));
static INLINE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"^(<?)(--|==|-\.)\s+([^|]*?)\s+(-{2,}[>ox]?|={2,}[>ox]?|\.-+[>ox]?|-{3,})(?=\s|[\p{L}\p{N}_"])"#,
    )
});
static ARROW: LazyLock<Regex> = LazyLock::new(|| re(r"^[<ox]?(?:-{2,}|={2,}|-\.+-|~{3,})[>ox]?"));
static PIPE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\|([^|]*)\|"));
static CLASS: LazyLock<Regex> = LazyLock::new(|| re(r"^:::[A-Za-z0-9_-]+"));
static SUBGRAPH: LazyLock<Regex> = LazyLock::new(|| re(r"^subgraph\s+(.*)$"));
static GROUP_TAIL: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(\[|$)"));

fn find_at_start<'a>(r: &Regex, s: &'a str) -> Option<&'a str> {
    r.find(s).ok().flatten().map(|m| m.as_str())
}

fn unquote(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// A link at the start of `s`: its length in bytes and its label.
fn read_link(s: &str) -> Option<(usize, Option<String>)> {
    // `-- text -->`, `== text ==>`, `-. text .->`: the label sits inside the arrow.
    if let Ok(Some(c)) = INLINE_LINK.captures(s) {
        let whole = c.get(0)?.as_str().len();
        let label = c.get(3).map(|m| m.as_str().trim()).unwrap_or("");
        return Some((whole, (!label.is_empty()).then(|| label.to_string())));
    }
    let arrow = find_at_start(&ARROW, s)?;
    let mut length = arrow.len();
    if let Ok(Some(p)) = PIPE.captures(&s[length..]) {
        length += p.get(0)?.as_str().len();
        let label = unquote(p.get(1).map(|m| m.as_str().trim()).unwrap_or(""));
        return Some((length, (!label.is_empty()).then(|| label.to_string())));
    }
    Some((length, None))
}

/// One node at the start of `s`: its id, its label if it has one, and how many bytes it used.
fn read_node(s: &str) -> Option<(String, Option<String>, usize)> {
    let id = find_at_start(&ID, s)?.to_string();
    let mut at = id.len();
    let mut label: Option<String> = None;

    for (open, closes) in SHAPES {
        if !s[at..].starts_with(open) {
            continue;
        }
        let mut i = at + open.len();
        let text;
        if s[i..].starts_with('"') {
            let Some(end) = s[i + 1..].find('"') else {
                return Some((id, None, at));
            };
            text = &s[i + 1..i + 1 + end];
            i = i + 1 + end + 1;
            let Some(close) = closes.iter().find(|c| s[i..].starts_with(**c)) else {
                return Some((id, None, at));
            };
            i += close.len();
        } else {
            let best = closes
                .iter()
                .filter_map(|c| s[i..].find(c).map(|p| (p, c.len())))
                .min_by_key(|(p, _)| *p);
            let Some((end, used)) = best else {
                return Some((id, None, at));
            };
            text = &s[i..i + end];
            i = i + end + used;
        }
        label = Some(text.trim().to_string());
        at = i;
        break;
    }

    if let Some(cls) = find_at_start(&CLASS, &s[at..]) {
        at += cls.len();
    }
    Some((id, label.filter(|l| !l.is_empty()), at))
}

/// Split on `;` outside quotes, so `a --> b; b --> c` is two statements.
fn statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, ch) in line.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if ch == ';' && !quoted {
            out.push(&line[start..i]);
            start = i + 1;
        }
    }
    out.push(&line[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Read a flowchart's nodes, in order of first appearance, and its arrows.
pub fn parse_chart(text: &str) -> Chart {
    let mut chart = Chart::default();
    let mut index: HashMap<String, usize> = HashMap::new();

    let mut see = |chart: &mut Chart, id: &str, label: Option<&str>| match index.get(id) {
        None => {
            index.insert(id.to_string(), chart.nodes.len());
            chart.nodes.push(ChartNode {
                id: id.to_string(),
                label: label.unwrap_or(id).to_string(),
            });
        }
        Some(&i) => {
            if let Some(l) = label
                && chart.nodes[i].label == id
            {
                chart.nodes[i].label = l.to_string();
            }
        }
    };

    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for raw in normalized.split('\n') {
        let line = raw.find("%%").map_or(raw, |i| &raw[..i]).trim();
        if line.is_empty() || HEADER.is_match(line).unwrap_or(false) {
            continue;
        }
        for statement in statements(line) {
            if let Ok(Some(group)) = SUBGRAPH.captures(statement) {
                // `subgraph id [title]` or `subgraph id`; a bare title with spaces has no id.
                let rest = group.get(1).map_or("", |m| m.as_str());
                if let Some(id) = find_at_start(&ID, rest)
                    && !chart.subgraphs.iter().any(|g| g == id)
                    && GROUP_TAIL.is_match(&rest[id.len()..]).unwrap_or(false)
                {
                    chart.subgraphs.push(id.to_string());
                }
                continue;
            }
            if SKIP.is_match(statement).unwrap_or(false) {
                continue;
            }

            let mut rest = statement;
            let mut previous: Option<Vec<String>> = None;
            let mut pending_label: Option<String> = None;
            while !rest.is_empty() {
                // A group: `a & b & c`.
                let mut group: Vec<String> = Vec::new();
                while let Some((id, label, length)) = read_node(rest) {
                    see(&mut chart, &id, label.as_deref());
                    group.push(id);
                    rest = rest[length..].trim_start();
                    let Some(after) = rest.strip_prefix('&') else {
                        break;
                    };
                    rest = after.trim_start();
                }
                if group.is_empty() {
                    break;
                }
                if let Some(prev) = &previous {
                    for from in prev {
                        for to in &group {
                            chart.edges.push(ChartEdge {
                                from: from.clone(),
                                to: to.clone(),
                                label: pending_label.clone(),
                            });
                        }
                    }
                }
                let Some((length, label)) = read_link(rest) else {
                    break;
                };
                previous = Some(group);
                pending_label = label;
                rest = rest[length..].trim_start();
            }
        }
    }
    chart
}

/// The ids reachable in one step from `id`, in chart order and without repeats.
pub fn successors<'a>(chart: &'a Chart, id: &str) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for e in &chart.edges {
        if e.from == id && !out.contains(&e.to.as_str()) {
            out.push(&e.to);
        }
    }
    out
}

/// Each node's strongly connected component (Tarjan's, without recursion so a long chart cannot
/// exhaust the stack): nodes on a loop together share one.
pub fn components(ids: &[String], edges: &[(String, String)]) -> HashMap<String, usize> {
    let mut number: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    for id in ids
        .iter()
        .map(String::as_str)
        .chain(edges.iter().flat_map(|(a, b)| [a.as_str(), b.as_str()]))
    {
        if !number.contains_key(id) {
            number.insert(id, names.len());
            names.push(id);
        }
    }
    let mut next_of: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    for (a, b) in edges {
        next_of[number[a.as_str()]].push(number[b.as_str()]);
    }

    let n = names.len();
    let mut index: Vec<Option<usize>> = vec![None; n];
    let mut low = vec![0; n];
    let mut component: Vec<Option<usize>> = vec![None; n];
    let mut stack: Vec<usize> = Vec::new();
    let (mut next, mut count) = (0, 0);

    for root in ids.iter().map(|id| number[id.as_str()]) {
        if index[root].is_some() {
            continue;
        }
        // (node, how many of its arrows are done)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(top) = work.last_mut() {
            let v = top.0;
            if top.1 == 0 && index[v].is_none() {
                index[v] = Some(next);
                low[v] = next;
                next += 1;
                stack.push(v);
            }
            let arrow = next_of[v].get(top.1).copied();
            if let Some(w) = arrow {
                top.1 += 1;
                match index[w] {
                    None => work.push((w, 0)),
                    Some(iw) if component[w].is_none() => low[v] = low[v].min(iw),
                    Some(_) => {}
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if Some(low[v]) == index[v] {
                while let Some(member) = stack.pop() {
                    component[member] = Some(count);
                    if member == v {
                        break;
                    }
                }
                count += 1;
            }
        }
    }

    names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| component[i].map(|c| (name.to_string(), c)))
        .collect()
}

/// The nodes in the order a run goes through them: each after every node with an arrow into it.
/// A loop's nodes stay together, in chart order; ties keep chart order, so a shortcut such as
/// `body --> post` does not pull `post` ahead of the steps on the longer path.
pub fn flow_order(chart: &Chart) -> Vec<ChartNode> {
    let ids: Vec<String> = chart.nodes.iter().map(|n| n.id.clone()).collect();
    let at: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let edges: Vec<(String, String)> = chart
        .edges
        .iter()
        .filter(|e| at.contains_key(e.from.as_str()) && at.contains_key(e.to.as_str()))
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    let scc = components(&ids, &edges);

    let mut group_order: Vec<usize> = Vec::new();
    let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
    for id in &ids {
        let g = scc[id];
        if !groups.contains_key(&g) {
            group_order.push(g);
        }
        groups.entry(g).or_default().push(id.clone());
    }
    let first = |g: usize| at[groups[&g][0].as_str()];

    let mut waiting: HashMap<usize, usize> = group_order.iter().map(|g| (*g, 0)).collect();
    let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut linked: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (a, b) in &edges {
        let (ga, gb) = (scc[a], scc[b]);
        if ga == gb || !linked.insert((ga, gb)) {
            continue;
        }
        after.entry(ga).or_default().push(gb);
        *waiting.entry(gb).or_default() += 1;
    }

    let mut ready: Vec<usize> = group_order
        .iter()
        .copied()
        .filter(|g| waiting[g] == 0)
        .collect();
    let mut order: Vec<String> = Vec::new();
    while !ready.is_empty() {
        ready.sort_by_key(|g| first(*g));
        let g = ready.remove(0);
        order.extend(groups[&g].iter().cloned());
        for h in after.get(&g).cloned().unwrap_or_default() {
            let left = waiting[&h] - 1;
            waiting.insert(h, left);
            if left == 0 {
                ready.push(h);
            }
        }
    }

    let by_id: HashMap<&str, &ChartNode> = chart.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    order.iter().map(|id| by_id[id.as_str()].clone()).collect()
}

/// `pre0` · `body` · `post0`: the id of one step of a run.
pub fn node_id_for(phase: &str, index: usize) -> String {
    if phase == "body" {
        "body".to_string()
    } else {
        format!("{phase}{index}")
    }
}

/// `pre0`, `body`, `post0`…: the ids kept for the hooks and the body. Every other node is a step.
pub fn is_reserved_node(id: &str) -> bool {
    if id == "body" {
        return true;
    }
    let digits = id.strip_prefix("pre").or_else(|| id.strip_prefix("post"));
    digits.is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
}

/// What a hook is called on screen: `pre[0] scripts/counts.js`, or `pre[0] prompt`.
pub fn hook_label(hook: &Hook, phase: &str, index: usize) -> String {
    match hook {
        Hook::Script(s) => format!("{phase}[{index}] {s}"),
        Hook::Prompt(_) => format!("{phase}[{index}] prompt"),
    }
}

/// Quote a Mermaid label: generated labels contain `[` and `]`.
pub fn mermaid_label(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "'"))
}

/// The chart for an automation without one: its hooks and body, in order.
pub fn plan_from_automation(a: &Automation) -> String {
    let mut lines = vec!["flowchart TD".to_string()];
    let mut ids = Vec::new();
    for (i, hook) in a.hooks.pre.iter().enumerate() {
        let id = node_id_for("pre", i);
        lines.push(format!(
            "  {id}[{}]",
            mermaid_label(&hook_label(hook, "pre", i))
        ));
        ids.push(id);
    }
    ids.push("body".to_string());
    lines.push(format!(
        "  body[{}]",
        mermaid_label(&format!("body: {}", a.name))
    ));
    for (i, hook) in a.hooks.post.iter().enumerate() {
        let id = node_id_for("post", i);
        lines.push(format!(
            "  {id}[{}]",
            mermaid_label(&hook_label(hook, "post", i))
        ));
        ids.push(id);
    }
    for pair in ids.windows(2) {
        lines.push(format!("  {} --> {}", pair[0], pair[1]));
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(text: &str) -> Vec<String> {
        flow_order(&parse_chart(text))
            .into_iter()
            .map(|n| n.id)
            .collect()
    }

    const TEAM_REVIEW: &str = "flowchart TD
  pre0[\"Pick a teammate's PR\"] --> body[\"Review the PR\"]
  body --> read[\"Read the change\"]
  body -->|saved review or sign-off| post[\"Post the review\"]
  read --> map[\"Map the change\"]
  map --> panel[\"The reviewers\"]
  panel --> draft[\"Choose the findings\"]
  draft --> post
  post --> post0[\"Record the run\"]";

    #[test]
    fn flow_order_puts_a_step_after_every_step_leading_to_it() {
        assert_eq!(
            order(TEAM_REVIEW),
            [
                "pre0", "body", "read", "map", "panel", "draft", "post", "post0"
            ]
        );
    }

    #[test]
    fn flow_order_keeps_chart_order_when_arrows_allow() {
        assert_eq!(
            order("flowchart TD\n  a --> b\n  b --> c\n  a --> d"),
            ["a", "b", "c", "d"]
        );
    }

    #[test]
    fn flow_order_keeps_a_loop_together() {
        let text = "flowchart TD\n  start --> fix\n  fix --> test\n  test -->|fails| fix\n  start --> done\n  test -->|passes| done";
        assert_eq!(order(text), ["start", "fix", "test", "done"]);
    }

    #[test]
    fn flow_order_keeps_unlinked_nodes_in_place() {
        assert_eq!(
            order("flowchart TD\n  a[A]\n  b --> c\n  z[Z]"),
            ["a", "b", "c", "z"]
        );
    }

    #[test]
    fn parses_nodes_labels_and_arrows() {
        let c = parse_chart(TEAM_REVIEW);
        assert_eq!(c.nodes.len(), 8);
        assert_eq!(c.nodes[0].label, "Pick a teammate's PR");
        let post = c.nodes.iter().find(|n| n.id == "post").unwrap();
        assert_eq!(post.label, "Post the review");
        assert_eq!(c.nodes[3].id, "post");
        assert_eq!(
            c.edges[2],
            ChartEdge {
                from: "body".into(),
                to: "post".into(),
                label: Some("saved review or sign-off".into())
            }
        );
        assert_eq!(successors(&c, "body"), ["read", "post"]);
    }

    #[test]
    fn reads_shapes_groups_inline_labels_and_comments() {
        let c = parse_chart(
            "graph LR\n  a((Round)) -- yes --> b{Choice}\n  b -.-> c[(Store)] & d>Flag]\n  %% a comment\n  c ==> e:::hot; e --- f\n  click a href \"x\"\n  style a fill:#f00\n",
        );
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d", "e", "f"]);
        assert_eq!(c.nodes[0].label, "Round");
        assert_eq!(c.nodes[1].label, "Choice");
        assert_eq!(c.nodes[2].label, "Store");
        assert_eq!(c.nodes[3].label, "Flag");
        assert_eq!(c.nodes[4].label, "e");
        assert_eq!(c.edges[0].label.as_deref(), Some("yes"));
        let pairs: Vec<(&str, &str)> = c
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [("a", "b"), ("b", "c"), ("b", "d"), ("c", "e"), ("e", "f")]
        );
    }

    #[test]
    fn subgraphs_are_groups_not_steps() {
        let c = parse_chart(
            "flowchart TD\n  read --> checks\n  subgraph checks [Checks]\n    lint\n  end\n  subgraph Plain title\n  end\n",
        );
        let ids: Vec<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["read", "checks", "lint"]);
        assert_eq!(c.subgraphs, ["checks"]);
    }

    #[test]
    fn components_group_loops() {
        let ids: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let edges: Vec<(String, String)> = [("a", "b"), ("b", "c"), ("c", "b"), ("c", "d")]
            .map(|(x, y)| (x.to_string(), y.to_string()))
            .to_vec();
        let scc = components(&ids, &edges);
        assert_eq!(scc["b"], scc["c"]);
        assert_ne!(scc["a"], scc["b"]);
        assert_ne!(scc["d"], scc["b"]);
    }

    #[test]
    fn components_survive_a_very_long_chart() {
        let n = 200_000;
        let ids: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();
        let mut edges: Vec<(String, String)> = ids
            .windows(2)
            .map(|w| (w[0].clone(), w[1].clone()))
            .collect();
        edges.push((ids[n - 1].clone(), ids[n - 2].clone()));
        let scc = components(&ids, &edges);
        assert_eq!(scc.len(), n);
        assert_eq!(scc[&ids[n - 1]], scc[&ids[n - 2]]);
        assert_ne!(scc[&ids[0]], scc[&ids[1]]);
    }

    #[test]
    fn reserved_ids_and_plans() {
        for id in ["body", "pre0", "pre12", "post3"] {
            assert!(is_reserved_node(id), "{id}");
        }
        for id in ["pre", "postx", "read", "bodies"] {
            assert!(!is_reserved_node(id), "{id}");
        }
        let (a, _) = crate::parse::parse_automation(
            "---\nname: x\ndescription: d\non:\n  manual: true\nhooks:\n  pre:\n    - script: s.js\n  post:\n    - prompt: Sum up.\n---\n\nBody.\n",
            "x",
        )
        .unwrap();
        assert_eq!(
            plan_from_automation(&a),
            "flowchart TD\n  pre0[\"pre[0] s.js\"]\n  body[\"body: x\"]\n  post0[\"post[0] prompt\"]\n  pre0 --> body\n  body --> post0\n"
        );
        assert_eq!(order(&plan_from_automation(&a)), ["pre0", "body", "post0"]);
    }
}

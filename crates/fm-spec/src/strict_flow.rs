//! `flow: strict`: the body's flowchart and its `##` sections are one thing. Every step node has
//! exactly one section, `## <label> {#<id>}`, in the chart's order, and a section that branches
//! names where it can go. Pure: text in, problems out.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use fancy_regex::{Captures, Regex};

use crate::chart::{Chart, components, is_reserved_node, parse_chart, re};
use crate::flow_block::{is_closing_fence, is_flowchart, opening_fence};

/// One thing wrong between a strict body and its chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowProblem {
    /// 1-based line in the file; `None` for a chart-wide problem.
    pub line: Option<usize>,
    /// `<what is wrong> — <what fixes it>`.
    pub text: String,
}

/// One problem as a line: `line 12: <what> — <fix>`.
pub fn flow_problem_line(p: &FlowProblem) -> String {
    match p.line {
        Some(n) => format!("line {n}: {}", p.text),
        None => p.text.clone(),
    }
}

static ENTITY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)&#x([0-9a-f]+);|(?:&#|#)([0-9]+);|[&#]([a-z]+);"));
static BR: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<br\s*/?>"));
static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"!?\[([^\]]*)\]\([^)]*\)"));
static TICKS: LazyLock<Regex> = LazyLock::new(|| re(r"`+"));
static STRONG: LazyLock<Regex> = LazyLock::new(|| re(r"(\*\*|__|~~)(.+?)\1"));
static EMPHASIS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(^|[^\p{L}\p{N}])([*_])(.+?)\2(?=$|[^\p{L}\p{N}])"));
static SPACE: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$"));
static CLOSING_HASHES: LazyLock<Regex> = LazyLock::new(|| re(r"(?:^|[ \t]+)#+$"));
static BOUND: LazyLock<Regex> = LazyLock::new(|| re(r"^(.*?)[ \t]*\{#([^\s{}]+)\}$"));
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| re(r"^\d+[.)]\s*"));

fn replace_all(r: &Regex, text: &str, with: &str) -> String {
    r.replace_all(text, with).into_owned()
}

/// Mermaid's `#quot;` / `#35;` codes and HTML's `&amp;` / `&#35;` / `&#x23;` as characters.
fn decode_entities(text: &str) -> String {
    ENTITY
        .replace_all(text, |c: &Captures<'_, str>| {
            let whole = c.get(0).map_or("", |m| m.as_str()).to_string();
            if let Some(name) = c.get(3) {
                return match name.as_str().to_ascii_lowercase().as_str() {
                    "amp" => "&".to_string(),
                    "lt" => "<".to_string(),
                    "gt" => ">".to_string(),
                    "quot" => "\"".to_string(),
                    "apos" => "'".to_string(),
                    "nbsp" => "\u{a0}".to_string(),
                    _ => whole,
                };
            }
            let n = match (c.get(1), c.get(2)) {
                (Some(hex), _) => u32::from_str_radix(hex.as_str(), 16).ok(),
                (_, Some(dec)) => dec.as_str().parse::<u32>().ok(),
                _ => None,
            };
            match n.filter(|n| *n > 0).and_then(char::from_u32) {
                Some(ch) => ch.to_string(),
                None => whole,
            }
        })
        .into_owned()
}

/// A label or heading as a reader sees it: entities decoded, no markup, no quotes, single spaces.
pub fn plain_text(text: &str) -> String {
    let mut s = decode_entities(text);
    s = replace_all(&BR, &s, " ");
    s = replace_all(&LINK, &s, "$1");
    s = replace_all(&TICKS, &s, "");
    s = replace_all(&STRONG, &s, "$2");
    s = replace_all(&EMPHASIS, &s, "${1}${3}");
    s = s.replace('"', "");
    s = replace_all(&SPACE, &s, " ");
    s.trim().to_string()
}

struct Heading {
    index: usize,
    level: usize,
    text: String,
}

struct Scan<'a> {
    lines: Vec<&'a str>,
    headings: Vec<Heading>,
    chart: Option<String>,
}

fn scan(body: &str) -> Scan<'_> {
    let lines: Vec<&str> = body
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut headings = Vec::new();
    let mut chart = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((_, ch, length, info)) = opening_fence(line) {
            let mut end = i + 1;
            while end < lines.len() && !is_closing_fence(lines[end], ch, length) {
                end += 1;
            }
            let word = info.split_whitespace().next().unwrap_or("");
            if chart.is_none() && end < lines.len() && word.eq_ignore_ascii_case("mermaid") {
                let content = &lines[i + 1..end];
                if is_flowchart(content.iter().copied()) {
                    chart = Some(content.join("\n"));
                }
            }
            i = end + 1;
            continue;
        }
        if let Ok(Some(h)) = HEADING.captures(line) {
            let level = h.get(1).map_or(0, |m| m.as_str().len());
            let raw = h.get(2).map_or("", |m| m.as_str());
            let text = replace_all(&CLOSING_HASHES, raw, "").trim().to_string();
            headings.push(Heading {
                index: i,
                level,
                text,
            });
        }
        i += 1;
    }
    Scan {
        lines,
        headings,
        chart,
    }
}

fn code(id: &str) -> String {
    format!("`{id}`")
}

fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

fn named(id: &str, label: &str) -> String {
    format!("{} ({})", code(id), quoted(label))
}

fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn mentions_word(text: &str, word: &str, case_insensitive: bool) -> bool {
    let flags = if case_insensitive { "(?i)" } else { "" };
    let pattern = format!(
        r"{flags}(?<![\p{{L}}\p{{N}}_-]){}(?![\p{{L}}\p{{N}}_-])",
        fancy_regex::escape(word)
    );
    Regex::new(&pattern)
        .ok()
        .and_then(|r| r.is_match(text).ok())
        .unwrap_or(false)
}

fn mentions(text: &str, id: &str, label: &str) -> bool {
    if mentions_word(text, id, false) {
        return true;
    }
    let want = plain_text(label);
    !want.is_empty() && mentions_word(&plain_text(text), &want, true)
}

struct Section {
    line: usize,
    text: String,
}

/// Check a strict body against its flowchart. `first_line` is the file line the body starts on,
/// so problems point into the file. Every problem, by line, chart-wide ones last.
pub fn check_strict_flow(body: &str, first_line: usize) -> Vec<FlowProblem> {
    let Scan {
        lines,
        headings,
        chart,
    } = scan(body);
    let Some(chart_text) = chart else {
        return vec![FlowProblem {
            line: None,
            text: "the body has no flowchart — add a ```mermaid block that starts with flowchart TD and has a node per step, or remove flow: strict".to_string(),
        }];
    };

    let chart: Chart = parse_chart(&chart_text);
    let groups: HashSet<&str> = chart.subgraphs.iter().map(String::as_str).collect();
    let steps: Vec<&crate::chart::ChartNode> = chart
        .nodes
        .iter()
        .filter(|n| !is_reserved_node(&n.id) && !groups.contains(n.id.as_str()))
        .collect();
    let labels: HashMap<&str, String> = steps
        .iter()
        .map(|n| (n.id.as_str(), plain_text(&n.label)))
        .collect();
    let at = |index: usize| index + first_line;
    let bound = |text: &str| -> Option<(String, String)> {
        let c = BOUND.captures(text).ok().flatten()?;
        Some((
            c.get(1)?.as_str().to_string(),
            c.get(2)?.as_str().to_string(),
        ))
    };
    let claimed: HashSet<String> = headings
        .iter()
        .filter(|h| h.level == 2)
        .filter_map(|h| bound(&h.text).map(|(_, id)| id))
        .collect();

    let mut problems: Vec<FlowProblem> = Vec::new();
    let mut sections: HashMap<String, Section> = HashMap::new();
    // An unbound heading that reads like an unclaimed step, numbered or not, is that step's section.
    let mut guessed: HashSet<String> = HashSet::new();

    for (k, h) in headings.iter().enumerate() {
        if h.level != 2 {
            continue;
        }
        let line = at(h.index);
        let shown = format!("## {}", h.text);
        let Some((title_raw, id)) = bound(&h.text) else {
            let plain = plain_text(&h.text);
            let unnumbered = replace_all(&NUMBERED, &plain, "");
            let like = steps.iter().find(|n| {
                let label = &labels[n.id.as_str()];
                !claimed.contains(&n.id)
                    && !guessed.contains(&n.id)
                    && (plain == *label || unnumbered == *label)
            });
            let text = match like {
                Some(n) => {
                    guessed.insert(n.id.clone());
                    format!(
                        "{} has no node id — write `## {} {{#{}}}`",
                        quoted(&shown),
                        labels[n.id.as_str()],
                        n.id
                    )
                }
                None => format!(
                    "{} is not a step — end it with {{#id}} for its node on the chart, or make it a ### under a step",
                    quoted(&shown)
                ),
            };
            problems.push(FlowProblem {
                line: Some(line),
                text,
            });
            continue;
        };
        if is_reserved_node(&id) {
            let what = if id == "body" { "the body" } else { "a hook" };
            problems.push(FlowProblem {
                line: Some(line),
                text: format!(
                    "{} names {}, which is {what} and has no section — give it a step's id, or make it a ### under a step",
                    quoted(&shown),
                    code(&id)
                ),
            });
            continue;
        }
        let Some(label) = labels.get(id.as_str()) else {
            let known: Vec<&str> = steps.iter().map(|n| n.id.as_str()).collect();
            let fix = if known.is_empty() {
                format!("add {id} to the chart")
            } else {
                format!("use one of {}, or add {id} to the chart", known.join(", "))
            };
            problems.push(FlowProblem {
                line: Some(line),
                text: format!(
                    "{} names {}, which is not on the chart — {fix}",
                    quoted(&shown),
                    code(&id)
                ),
            });
            continue;
        };
        if let Some(before) = sections.get(&id) {
            problems.push(FlowProblem {
                line: Some(line),
                text: format!(
                    "{} has a second section — merge it into the one on line {}",
                    code(&id),
                    before.line
                ),
            });
            continue;
        }
        let title = plain_text(&title_raw);
        if title != *label {
            problems.push(FlowProblem {
                line: Some(line),
                text: format!(
                    "heading {} does not match node {}'s label {} — make them the same text",
                    quoted(&title),
                    code(&id),
                    quoted(label)
                ),
            });
        }
        let end = headings[k + 1..]
            .iter()
            .find(|n| n.level <= 2)
            .map_or(lines.len(), |n| n.index);
        sections.insert(
            id,
            Section {
                line,
                text: lines[h.index + 1..end].join("\n"),
            },
        );
    }

    for n in &steps {
        if !sections.contains_key(&n.id) && !guessed.contains(&n.id) {
            let label = &labels[n.id.as_str()];
            problems.push(FlowProblem {
                line: None,
                text: format!(
                    "node {} has no section — add `## {label} {{#{}}}`",
                    named(&n.id, label),
                    n.id
                ),
            });
        }
    }

    let step_ids: Vec<String> = steps.iter().map(|n| n.id.clone()).collect();
    let is_step: HashSet<&str> = step_ids.iter().map(String::as_str).collect();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen_edges: HashSet<(&str, &str)> = HashSet::new();
    for e in &chart.edges {
        if is_step.contains(e.from.as_str())
            && is_step.contains(e.to.as_str())
            && seen_edges.insert((e.from.as_str(), e.to.as_str()))
        {
            edges.push((e.from.clone(), e.to.clone()));
        }
    }
    // Arrows on a loop are not ordered. Every other arrow a → b orders the loops (or single
    // steps) they join: every section of a's group comes before every section of b's.
    let scc = components(&step_ids, &edges);
    let mut members_of: HashMap<usize, Vec<&String>> = HashMap::new();
    for s in &step_ids {
        if sections.contains_key(s.as_str()) {
            members_of.entry(scc[s.as_str()]).or_default().push(s);
        }
    }
    let mut arrows_into: HashMap<usize, Vec<(&String, &String)>> = HashMap::new();
    let mut arrows_from: HashMap<&str, Vec<&String>> = HashMap::new();
    for (a, b) in &edges {
        arrows_from.entry(a.as_str()).or_default().push(b);
        if scc[a] != scc[b] {
            arrows_into.entry(scc[b]).or_default().push((a, b));
        }
    }

    for id in &step_ids {
        let Some(to) = sections.get(id) else {
            continue;
        };
        let mut late: Option<(&str, &str, &str)> = None;
        for (a, b) in arrows_into.get(&scc[id]).into_iter().flatten() {
            if let Some(x) = members_of
                .get(&scc[a.as_str()])
                .into_iter()
                .flatten()
                .find(|s| sections[s.as_str()].line > to.line)
            {
                late = Some((a, b, x));
                break;
            }
        }
        if let Some((a, b, x)) = late {
            let from = &sections[x];
            let mut loops = Vec::new();
            if x != a {
                loops.push(format!("{} is on a loop with {}", code(x), code(a)));
            }
            if id != b {
                loops.push(format!("{} is on a loop with {}", code(id), code(b)));
            }
            let loops = if loops.is_empty() {
                String::new()
            } else {
                format!(", and {}", loops.join(" and "))
            };
            problems.push(FlowProblem {
                line: Some(to.line),
                text: format!(
                    "section {} comes before {} (line {}), but the chart goes {a} → {b}{loops} — move {} below {}",
                    code(id),
                    code(x),
                    from.line,
                    code(id),
                    code(x)
                ),
            });
        }
    }

    for id in &step_ids {
        let Some(section) = sections.get(id) else {
            continue;
        };
        let next: Vec<&String> = arrows_from
            .get(id.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|b| *b != id)
            .collect();
        if next.len() < 2 {
            continue;
        }
        for b in &next {
            let label = &labels[b.as_str()];
            if mentions(&section.text, b, label) {
                continue;
            }
            let all: Vec<String> = next.iter().map(|n| code(n)).collect();
            problems.push(FlowProblem {
                line: Some(section.line),
                text: format!(
                    "section {} branches to {} but never mentions {} — say when to go there, for example `then go to {b}`",
                    code(id),
                    list(&all),
                    named(b, label)
                ),
            });
        }
    }

    problems.sort_by_key(|p| p.line.unwrap_or(usize::MAX));
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = "```mermaid
flowchart TD
  pre0[\"Pick the change\"] --> body[\"Review it\"]
  body --> read[\"Read the change\"]
  read --> map[\"Map the code\"]
  map --> draft[\"Draft the review\"]
  draft -->|clean| post[\"Post the comment\"]
  draft -->|findings| fix[\"Fix the findings\"]
  fix -->|again| draft
  post --> post0
```";

    const READ: &str = "## Read the change {#read}\n\nRead the diff.";
    const MAP: &str = "## Map the code {#map}\n\nList what it touches.";
    const DRAFT: &str = "## Draft the review {#draft}\n\nIf nothing is left, `pa next post`; otherwise go to Fix the findings.";
    const FIX: &str = "## Fix the findings {#fix}\n\nFix them, then back to `pa next draft`.";
    const POST: &str = "## Post the comment {#post}\n\nPost it.";

    fn body_with(parts: &[&str], chart: &str) -> String {
        let mut out = vec!["# Review a change", "", chart, ""];
        for p in parts {
            out.push(p);
            out.push("");
        }
        out.join("\n")
    }

    fn body(parts: &[&str]) -> String {
        body_with(parts, CHART)
    }

    fn lines(text: &str) -> Vec<String> {
        lines_from(text, 1)
    }

    fn lines_from(text: &str, first: usize) -> Vec<String> {
        check_strict_flow(text, first)
            .iter()
            .map(flow_problem_line)
            .collect()
    }

    fn valid() -> String {
        body(&[READ, MAP, DRAFT, FIX, POST])
    }

    #[test]
    fn a_body_that_follows_its_chart_has_no_problems() {
        let text = body(&[
            "The intro before the first step is free text.",
            READ,
            "### Details\n\nA subsection belongs to the step above it.",
            MAP,
            DRAFT,
            FIX,
            POST,
        ]);
        assert!(lines(&text).is_empty(), "{:?}", lines(&text));
        assert!(lines(&valid()).is_empty(), "{:?}", lines(&valid()));
    }

    #[test]
    fn ignores_headings_inside_code_fences() {
        let fenced = "```markdown\n## Not a step\n## Another {#nope}\n```";
        let tilde = "~~~\n## Also not one\n~~~";
        assert!(lines(&body(&[READ, fenced, tilde, MAP, DRAFT, FIX, POST])).is_empty());
    }

    #[test]
    fn matches_labels_with_markup() {
        let chart = "```mermaid\nflowchart LR\n  a[\"Read the<br/>change\"] --> b[\"Run `npm test`\"]\n  b --> c[Ship it]\n```";
        let text = body_with(
            &[
                "## Read the  change {#a}",
                "## Run `npm test` {#b}",
                "## **Ship** it {#c}",
            ],
            chart,
        );
        assert!(lines(&text).is_empty(), "{:?}", lines(&text));
    }

    #[test]
    fn uses_the_id_as_the_label_of_a_node_without_one() {
        let chart = "```mermaid\nflowchart TD\n  gather --> report\n```";
        assert!(
            lines(&body_with(
                &["## gather {#gather}", "## report {#report}"],
                chart
            ))
            .is_empty()
        );
    }

    #[test]
    fn a_branch_may_name_its_targets_many_ways() {
        let chart = "```mermaid\nflowchart TD\n  check --> ok[\"Ship it\"]\n  check --> bad[\"Send it back\"]\n```";
        for v in [
            "Go to ok, or to bad.",
            "See #ok and #bad.",
            "Run `pa next ok` or `pa next bad`.",
            "Then ship it — or SEND IT BACK.",
        ] {
            let check = format!("## check {{#check}}\n\n{v}");
            let text = body_with(
                &[&check, "## Ship it {#ok}", "## Send it back {#bad}"],
                chart,
            );
            assert!(lines(&text).is_empty(), "{v}: {:?}", lines(&text));
        }
    }

    #[test]
    fn rule_a_needs_a_flowchart() {
        assert_eq!(
            lines("# Title\n\n## Read {#read}\n"),
            [
                "the body has no flowchart — add a ```mermaid block that starts with flowchart TD and has a node per step, or remove flow: strict"
            ]
        );
        assert_eq!(
            lines("```mermaid\nsequenceDiagram\n  a->>b: hi\n```\n").len(),
            1
        );
    }

    #[test]
    fn rule_b_one_section_per_step() {
        assert_eq!(
            lines(&body(&[READ, MAP, DRAFT, POST])),
            ["node `fix` (\"Fix the findings\") has no section — add `## Fix the findings {#fix}`"]
        );
        assert_eq!(
            lines(&body(&[
                READ,
                "## Read the change {#read}",
                MAP,
                DRAFT,
                FIX,
                POST
            ])),
            ["line 19: `read` has a second section — merge it into the one on line 15"]
        );
    }

    #[test]
    fn rule_c_every_heading_is_a_step() {
        let text = body(&[
            "## Notes",
            READ,
            MAP,
            DRAFT,
            "## Something else {#other}",
            FIX,
            POST,
            "## Record {#post0}",
            "## The run {#body}",
        ]);
        assert_eq!(
            lines(&text),
            [
                "line 15: \"## Notes\" is not a step — end it with {#id} for its node on the chart, or make it a ### under a step",
                "line 29: \"## Something else {#other}\" names `other`, which is not on the chart — use one of read, map, draft, post, fix, or add other to the chart",
                "line 39: \"## Record {#post0}\" names `post0`, which is a hook and has no section — give it a step's id, or make it a ### under a step",
                "line 41: \"## The run {#body}\" names `body`, which is the body and has no section — give it a step's id, or make it a ### under a step",
            ]
        );
        assert_eq!(
            lines(&body(&["## 1. Read the change", MAP, DRAFT, FIX, POST])),
            [
                "line 15: \"## 1. Read the change\" has no node id — write `## Read the change {#read}`"
            ]
        );
        assert!(
            lines(&body(&[
                "## Read the change {#read} ##",
                MAP,
                DRAFT,
                FIX,
                POST
            ]))
            .is_empty()
        );
    }

    #[test]
    fn rule_d_heading_is_the_label() {
        assert_eq!(
            lines(&body(&["## Read the diff {#read}", MAP, DRAFT, FIX, POST])),
            [
                "line 15: heading \"Read the diff\" does not match node `read`'s label \"Read the change\" — make them the same text"
            ]
        );
    }

    #[test]
    fn rule_e_chart_order() {
        assert_eq!(
            lines(&body(&[MAP, READ, DRAFT, FIX, POST])),
            [
                "line 15: section `map` comes before `read` (line 19), but the chart goes read → map — move `map` below `read`"
            ]
        );
        assert!(lines(&body(&[READ, MAP, FIX, DRAFT, POST])).is_empty());

        let sections = [
            "## read {#read}\n\nGo to verify, or skip to report.",
            "## verify {#verify}",
            "## report {#report}",
        ];
        let a = "```mermaid\nflowchart TD\n  body --> read\n  read --> verify\n  read -->|skip| report\n  verify --> report\n  report -->|retry| verify\n```";
        let b = "```mermaid\nflowchart TD\n  body --> read\n  read -->|skip| report\n  read --> verify\n  verify --> report\n  report -->|retry| verify\n```";
        assert!(lines(&body_with(&sections, a)).is_empty());
        assert!(lines(&body_with(&sections, b)).is_empty());

        let chart =
            "```mermaid\nflowchart TD\n  a --> b\n  b --> c\n  c -->|retry| b\n  c --> d\n```";
        let c = "## c {#c}\n\nBack to b, or on to d.";
        assert!(
            lines(&body_with(
                &["## a {#a}", "## b {#b}", c, "## d {#d}"],
                chart
            ))
            .is_empty()
        );
        assert_eq!(
            lines(&body_with(
                &["## a {#a}", c, "## d {#d}", "## b {#b}"],
                chart
            )),
            [
                "line 17: section `d` comes before `b` (line 19), but the chart goes c → d, and `b` is on a loop with `c` — move `d` below `b`"
            ]
        );
        assert_eq!(
            lines(&body_with(
                &[c, "## a {#a}", "## b {#b}", "## d {#d}"],
                chart
            )),
            [
                "line 11: section `c` comes before `a` (line 15), but the chart goes a → b, and `c` is on a loop with `b` — move `c` below `a`"
            ]
        );
    }

    #[test]
    fn subgraphs_are_not_steps() {
        let chart = "```mermaid\nflowchart TD\n  read --> checks\n  subgraph checks [Checks]\n    lint --> test\n  end\n```";
        assert!(
            lines(&body_with(
                &["## read {#read}", "## lint {#lint}", "## test {#test}"],
                chart
            ))
            .is_empty()
        );
        assert!(
            lines(&body_with(
                &[
                    "## read {#read}",
                    "## lint {#lint}",
                    "## test {#test}",
                    "## checks {#checks}"
                ],
                chart
            ))[0]
                .contains("names `checks`, which is not on the chart")
        );
    }

    #[test]
    fn rule_f_branches_name_their_targets() {
        let draft = "## Draft the review {#draft}\n\nWrite it, then `pa next post`.";
        assert_eq!(
            lines(&body(&[READ, MAP, draft, FIX, POST])),
            [
                "line 23: section `draft` branches to `post` and `fix` but never mentions `fix` (\"Fix the findings\") — say when to go there, for example `then go to fix`"
            ]
        );
        let chart = "```mermaid\nflowchart TD\n  check --> ok\n  check --> bad\n```";
        assert!(
            lines(&body_with(
                &[
                    "## check {#check}\n\n### Then\n\nGo to ok or bad.",
                    "## ok {#ok}",
                    "## bad {#bad}"
                ],
                chart
            ))
            .is_empty()
        );
        assert_eq!(
            lines(&body_with(
                &[
                    "## check {#check}\n\nGo to ok.",
                    "## ok {#ok}\n\nNot bad.",
                    "## bad {#bad}"
                ],
                chart
            ))
            .len(),
            1
        );
        assert_eq!(
            lines(&body_with(
                &[
                    "## check {#check}\n\nIt is okay, or badly done.",
                    "## ok {#ok}",
                    "## bad {#bad}"
                ],
                chart
            ))
            .len(),
            2
        );
    }

    #[test]
    fn every_problem_at_once_by_line() {
        let text = body(&[
            "## Notes",
            MAP,
            "## Read the diff {#read}",
            "## Draft the review {#draft}\n\nThen `pa next post`.",
        ]);
        assert_eq!(
            lines_from(&text, 8),
            [
                "line 22: \"## Notes\" is not a step — end it with {#id} for its node on the chart, or make it a ### under a step",
                "line 24: section `map` comes before `read` (line 28), but the chart goes read → map — move `map` below `read`",
                "line 28: heading \"Read the diff\" does not match node `read`'s label \"Read the change\" — make them the same text",
                "line 30: section `draft` branches to `post` and `fix` but never mentions `fix` (\"Fix the findings\") — say when to go there, for example `then go to fix`",
                "node `post` (\"Post the comment\") has no section — add `## Post the comment {#post}`",
                "node `fix` (\"Fix the findings\") has no section — add `## Fix the findings {#fix}`",
            ]
        );
    }

    #[test]
    fn a_long_chart_checks_quickly() {
        let n = 3000;
        let mut chart = String::from("```mermaid\nflowchart TD\n");
        let mut parts = Vec::new();
        for i in 0..n {
            if i + 1 < n {
                chart.push_str(&format!("  s{i} --> s{}\n", i + 1));
            }
            parts.push(format!("## s{i} {{#s{i}}}"));
        }
        chart.push_str("```");
        let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
        let started = std::time::Instant::now();
        assert!(lines(&body_with(&refs, &chart)).is_empty());
        let mut reversed = refs.clone();
        reversed.reverse();
        assert_eq!(lines(&body_with(&reversed, &chart)).len(), n - 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn plain_text_decodes_and_strips() {
        assert_eq!(plain_text("Fix #quot;flaky#quot; tests"), "Fix flaky tests");
        assert_eq!(
            plain_text("Issue #35;12 &amp; &lt;b&gt; &#35;3 &#x41;"),
            "Issue #12 & <b> #3 A"
        );
        assert_eq!(plain_text("Read the<br/>\"change\""), "Read the change");
        assert_eq!(
            plain_text("**Bold** _em_ `code` [link](https://x.test) ~~gone~~"),
            "Bold em code link gone"
        );
        assert_eq!(plain_text("snake_case_name stays"), "snake_case_name stays");
        let chart = "```mermaid\nflowchart TD\n  a[\"Fix #quot;flaky#quot; tests\"] --> b[\"Build &amp; test\"]\n  b --> c[\"Ship #35;1\"]\n```";
        assert!(
            lines(&body_with(
                &[
                    "## Fix \"flaky\" tests {#a}",
                    "## Build & test {#b}",
                    "## Ship #1 {#c}"
                ],
                chart
            ))
            .is_empty()
        );
        assert!(
            lines(&body_with(
                &[
                    "## Fix &quot;flaky&quot; tests {#a}",
                    "## Build &amp; test {#b}",
                    "## Ship &#35;1 {#c}"
                ],
                chart
            ))
            .is_empty()
        );
    }
}

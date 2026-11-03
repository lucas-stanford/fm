//! The chart in an `AUTOMATION.md` body: the first fenced `mermaid` block that is a flowchart.

use std::path::Path;

/// A flowchart block: its text (LF, de-indented, one trailing newline) and its byte range in the
/// Markdown it was found in, fences included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowBlock {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

struct Line<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn lines_of(markdown: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    while let Some(nl) = markdown[start..].find('\n').map(|i| start + i) {
        let text_end = if nl > start && markdown.as_bytes()[nl - 1] == b'\r' {
            nl - 1
        } else {
            nl
        };
        lines.push(Line {
            start,
            end: nl + 1,
            text: &markdown[start..text_end],
        });
        start = nl + 1;
    }
    if start < markdown.len() || markdown.is_empty() {
        lines.push(Line {
            start,
            end: markdown.len(),
            text: &markdown[start..],
        });
    }
    lines
}

fn body_start_line(lines: &[Line<'_>]) -> usize {
    if lines.first().map(|l| l.text) != Some("---") {
        return 0;
    }
    (1..lines.len())
        .find(|&i| lines[i].text == "---" || lines[i].text == "...")
        .map_or(0, |i| i + 1)
}

/// `( {0,3})(`{3,}|~{3,})(info)`; a backtick fence's info has no backtick.
pub(crate) fn opening_fence(line: &str) -> Option<(usize, char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = rest.len() - rest.trim_start_matches(ch).len();
    if length < 3 {
        return None;
    }
    let info = &rest[length..];
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some((indent, ch, length, info))
}

/// ` {0,3}` then `length` or more fence characters, then only whitespace.
pub(crate) fn is_closing_fence(line: &str, ch: char, length: usize) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    let run = rest.len() - rest.trim_start_matches(ch).len();
    run >= length && rest[run..].trim().is_empty()
}

fn first_info_word(info: &str) -> &str {
    info.split_whitespace().next().unwrap_or("")
}

/// True when the first line that is not blank or a `%%` comment starts with `flowchart` or `graph`.
pub(crate) fn is_flowchart<'a>(lines: impl Iterator<Item = &'a str>) -> bool {
    lines
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%"))
        .is_some_and(|first| {
            let lower = first.to_ascii_lowercase();
            ["flowchart", "graph"].iter().any(|kw| {
                lower.starts_with(kw)
                    && !lower[kw.len()..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        })
}

fn normalize_chart(raw: &str, indent: usize) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let prefix = " ".repeat(indent);
    let joined = unified
        .split('\n')
        .map(|l| {
            if indent > 0 {
                l.strip_prefix(prefix.as_str()).unwrap_or(l)
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n", joined.trim_end_matches('\n'))
}

/// The first ```` ```mermaid ```` flowchart (or `graph`) block, skipping a frontmatter block if the
/// text starts with one. Blocks inside a longer fence do not count.
pub fn find_flow_block(markdown: &str) -> Option<FlowBlock> {
    let lines = lines_of(markdown);
    let mut i = body_start_line(&lines);
    while i < lines.len() {
        let Some((indent, ch, length, info)) = opening_fence(lines[i].text) else {
            i += 1;
            continue;
        };
        let close = (i + 1..lines.len()).find(|&j| is_closing_fence(lines[j].text, ch, length))?;
        if first_info_word(info).eq_ignore_ascii_case("mermaid") {
            let content = &markdown[lines[i].end..lines[close].start];
            let text = normalize_chart(content, indent);
            if is_flowchart(text.split('\n')) {
                return Some(FlowBlock {
                    text,
                    start: lines[i].start,
                    end: lines[close].end,
                });
            }
        }
        i = close + 1;
    }
    None
}

/// The chart a run is tracked on: the block in `AUTOMATION.md`, else the older `flow.mmd` beside
/// it, else `None`. Reads only.
pub fn read_chart(dir: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(dir.join("AUTOMATION.md"))
        && let Some(block) = find_flow_block(&text)
        && !block.text.trim().is_empty()
    {
        return Some(block.text);
    }
    let legacy = std::fs::read_to_string(dir.join("flow.mmd")).ok()?;
    (!legacy.trim().is_empty()).then_some(legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_frontmatter_and_finds_the_first_flowchart() {
        let md = [
            "---",
            "name: daily",
            "---",
            "```mermaid",
            "sequenceDiagram",
            "  A->>B: hi",
            "```",
            "```mermaid",
            "%% comment",
            "",
            "flowchart TD",
            "  body[Body]",
            "```",
            "```mermaid",
            "graph LR",
            "  a --> b",
            "```",
            "",
        ]
        .join("\n");
        assert_eq!(
            find_flow_block(&md).unwrap().text,
            "%% comment\n\nflowchart TD\n  body[Body]\n"
        );
    }

    #[test]
    fn accepts_tildes_graph_and_indented_fences() {
        let md = "Intro\n  ~~~mermaid\n  graph LR\n    a --> b\n  ~~~\n";
        let block = find_flow_block(md).unwrap();
        assert_eq!(block.text, "graph LR\n  a --> b\n");
        assert_eq!(
            &md[block.start..block.end],
            "  ~~~mermaid\n  graph LR\n    a --> b\n  ~~~\n"
        );
    }

    #[test]
    fn ignores_a_mermaid_example_inside_a_longer_fence() {
        let md = "````markdown\n```mermaid\nflowchart TD\n  a\n```\n````\n```mermaid\ngraph LR\n  b\n```\n";
        assert_eq!(find_flow_block(md).unwrap().text, "graph LR\n  b\n");
    }

    #[test]
    fn skips_unclosed_fences_and_other_diagrams() {
        assert_eq!(find_flow_block("```mermaid\nflowchart TD\n  a\n"), None);
        assert_eq!(
            find_flow_block("```mermaid\nsequenceDiagram\n  A->>B: hi\n```\ntext\n"),
            None
        );
    }

    #[test]
    fn offsets_are_against_the_crlf_original() {
        let md = "Title\r\n\r\n```mermaid\r\nflowchart TD\r\n  body[Body]\r\n```\r\nTail\r\n";
        let block = find_flow_block(md).unwrap();
        assert_eq!(block.start, md.find("```mermaid").unwrap());
        assert_eq!(block.end, md.find("Tail").unwrap());
        assert_eq!(block.text, "flowchart TD\n  body[Body]\n");
    }

    #[test]
    fn reads_the_block_before_flow_mmd() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_chart(dir.path()), None);
        std::fs::write(dir.path().join("flow.mmd"), "flowchart TD\n  a --> b\n").unwrap();
        std::fs::write(
            dir.path().join("AUTOMATION.md"),
            "---\nname: x\n---\n\nNo chart.\n",
        )
        .unwrap();
        assert_eq!(
            read_chart(dir.path()).as_deref(),
            Some("flowchart TD\n  a --> b\n")
        );
        std::fs::write(
            dir.path().join("AUTOMATION.md"),
            "---\nname: x\n---\n\n```mermaid\ngraph LR\n  c\n```\n",
        )
        .unwrap();
        assert_eq!(read_chart(dir.path()).as_deref(), Some("graph LR\n  c\n"));
    }
}

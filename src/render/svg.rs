//! SVG output: merman's drawing plus a `data-id` on each node, dark-theme
//! overrides for what `classDef` cannot reach, and sanitisation, because the
//! SVG may be shown in a browser.

use crate::error::{Code, Result, fail};
use crate::flow::Flow;
use crate::render::decorate::{self, Mode};
use crate::render::theme::Theme;
use crate::state::State;

/// Keeps a pathological plan from producing a document a viewer cannot load.
const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;

pub fn render(plan: &str, flow: &Flow, state: &State, theme: Theme) -> Result<String> {
    let source = decorate::decorate(plan, flow, state, Mode::Svg, theme);
    let raw = render_source(&source)?;
    let identified = add_node_ids(&raw);
    let themed = apply_theme_overrides(&identified, theme);
    let safe = sanitize(&themed);
    if safe.len() > MAX_SVG_BYTES {
        return Err(fail!(
            Code::Render,
            "the rendered SVG is {} bytes, over the {MAX_SVG_BYTES}-byte limit",
            safe.len()
        ));
    }
    Ok(safe)
}

/// `classDef` only styles nodes; edge labels, clusters and the background keep
/// merman's light theme, so the dark theme overrides them with a stylesheet.
fn apply_theme_overrides(svg: &str, theme: Theme) -> String {
    if theme != Theme::Dark {
        return svg.to_string();
    }
    let overrides = concat!(
        "<style>",
        // merman sets a white background inline on the root element.
        "#merman{background-color:#1f1f1f !important;}",
        "#merman .edgeLabel{color:#f3f2f1;background:#1f1f1f;fill:#1f1f1f;}",
        "#merman .edgeLabel p{background:#1f1f1f;}",
        "#merman .edgeLabel rect,#merman .labelBkg{fill:#1f1f1f;background:#1f1f1f;opacity:1;}",
        "#merman .cluster rect{fill:#242424;stroke:#4a4a4a;}",
        "#merman .cluster-label,#merman .cluster text{fill:#f3f2f1;}",
        "#merman .marker{fill:#8a8886;stroke:#8a8886;}",
        "#merman .flowchart-link{stroke:#6b6b6b;}",
        "</style>"
    );
    match svg.rfind("</svg>") {
        Some(at) => format!("{}{overrides}{}", &svg[..at], &svg[at..]),
        None => svg.to_string(),
    }
}

fn render_source(source: &str) -> Result<String> {
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer};

    let output = Renderer::new()
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            merman::SvgRequest::default(),
        ))
        .map_err(|error| fail!(Code::Render, "the renderer failed: {error}"))?;
    match output {
        RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_string()),
        _ => Err(fail!(Code::Render, "the renderer produced no SVG")),
    }
}

/// Recovers the node id from merman's `merman-flowchart-<nodeId>-<index>`. An id
/// may itself end in `-<digits>`, so exactly one trailing index is stripped.
fn node_id_from(group_id: &str) -> Option<&str> {
    let rest = group_id.strip_prefix("merman-flowchart-")?;
    let dash = rest.rfind('-')?;
    let (id, index) = rest.split_at(dash);
    if index.len() > 1 && index[1..].chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
        Some(id)
    } else {
        None
    }
}

/// Adds `data-id="<nodeId>"` to every node group, so a host page can select a
/// node by the id the author wrote.
fn add_node_ids(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len() + 256);
    let mut rest = svg;
    const NEEDLE: &str = "id=\"merman-flowchart-";

    while let Some(at) = rest.find(NEEDLE) {
        let value_start = at + "id=\"".len();
        let Some(value_end) = rest[value_start..].find('"').map(|end| value_start + end) else {
            break;
        };
        let group_id = &rest[value_start..value_end];
        out.push_str(&rest[..=value_end]);
        if let Some(node_id) = node_id_from(group_id) {
            out.push_str(&format!(" data-id=\"{node_id}\""));
        }
        rest = &rest[value_end + 1..];
    }
    out.push_str(rest);
    out
}

/// Strips scripts, event handlers and external references. Labels come from
/// whoever wrote the plan, so this is defence in depth. `<foreignObject>` is
/// kept: merman puts every label inside one.
fn sanitize(svg: &str) -> String {
    drop_attributes(&drop_elements(svg, "script"))
}

/// Removes `<tag …>…</tag>` pairs and self-closing forms.
fn drop_elements(svg: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(at) = rest.find(&open) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match tail.find(&close) {
            Some(end) => rest = &tail[end + close.len()..],
            None => match tail.find('>') {
                Some(end) => rest = &tail[end + 1..],
                None => rest = "",
            },
        }
    }
    out.push_str(rest);
    out
}

/// Removes event handlers, `<a>` and external `href`s, keeping same-document
/// `url(#…)` references (arrow markers). Every `on<letters>=` attribute goes:
/// no standard SVG attribute has that shape, and an unknown event cannot slip
/// through an allow-list.
fn drop_attributes(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    let mut scanned = 0;
    let mut in_tag = false;
    let mut quote = None;
    loop {
        let candidate = [
            "<a ",
            " on",
            "xlink:href=\"http",
            "href=\"http",
            "href=\"javascript",
        ]
        .iter()
        .filter_map(|needle| rest.find(needle).map(|at| (at, *needle)))
        .min_by_key(|(at, _)| *at);
        let Some((at, needle)) = candidate else {
            break;
        };
        let absolute = svg.len() - rest.len() + at;
        for ch in svg[scanned..absolute].chars() {
            if let Some(delimiter) = quote {
                if ch == delimiter {
                    quote = None;
                }
            } else {
                match ch {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    '"' | '\'' if in_tag => quote = Some(ch),
                    _ => {}
                }
            }
        }
        scanned = absolute;
        if needle == " on" {
            // ` only ` in text is not an attribute.
            let after = &rest[at + 3..];
            let name_len = after.chars().take_while(|c| c.is_ascii_lowercase()).count();
            let is_attribute =
                in_tag && quote.is_none() && name_len > 0 && after[name_len..].starts_with('=');
            if !is_attribute {
                out.push_str(&rest[..at + 3]);
                rest = &rest[at + 3..];
                continue;
            }
        }
        out.push_str(&rest[..at]);
        // Skip to the end of the attribute, or of the tag for `<a `.
        let tail = &rest[at..];
        let skip = if needle == "<a " {
            tail.find('>').map(|end| end + 1).unwrap_or(tail.len())
        } else if needle == " on" {
            let start = tail.find('=').expect("matched an attribute") + 1;
            let value = &tail[start..];
            if let Some(delimiter @ ('"' | '\'')) = value.chars().next() {
                value[1..]
                    .find(delimiter)
                    .map(|end| start + end + 2)
                    .unwrap_or_else(|| tail.find('>').unwrap_or(tail.len()))
            } else {
                value
                    .find(|ch: char| ch.is_whitespace() || ch == '>')
                    .map(|end| start + end)
                    .unwrap_or(tail.len())
            }
        } else {
            let value_start = tail.find('"').map(|q| q + 1).unwrap_or(tail.len());
            tail[value_start..]
                .find('"')
                .map(|end| value_start + end + 1)
                .unwrap_or(tail.len())
        };
        rest = &tail[skip..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::fixture::{self, PLAN};

    fn svg(json: &str, theme: Theme) -> String {
        render(PLAN, &fixture::flow(), &fixture::state(json), theme).expect("render")
    }

    #[test]
    fn every_node_gets_a_data_id_matching_its_authored_id() {
        let svg = svg("{}", Theme::Light);
        for id in ["fetch", "build", "test", "deploy", "fix"] {
            assert!(
                svg.contains(&format!("data-id=\"{id}\"")),
                "node '{id}' is not addressable"
            );
        }
    }

    #[test]
    fn a_node_id_containing_hyphens_and_digits_round_trips() {
        assert_eq!(node_id_from("merman-flowchart-fetch-0"), Some("fetch"));
        assert_eq!(
            node_id_from("merman-flowchart-my-node-1-12"),
            Some("my-node-1")
        );
        assert_eq!(node_id_from("merman-flowchart-b2-3"), Some("b2"));
        // Not a node group.
        assert_eq!(node_id_from("merman-L_a_b_0"), None);
        assert_eq!(node_id_from("merman-drop-shadow"), None);
    }

    #[test]
    fn the_status_class_reaches_the_node_group() {
        let svg = svg(
            r#"{"nodes": {"fetch": "done", "build": "active"}}"#,
            Theme::Light,
        );
        assert!(svg.contains("fm_done"));
        assert!(svg.contains("fm_active"));
        assert!(svg.contains("fm_pending"));
    }

    #[test]
    fn a_taken_edge_is_highlighted() {
        let taken = svg(r#"{"taken": [["fetch", "build"]]}"#, Theme::Light);
        let untaken = svg("{}", Theme::Light);
        assert!(taken.contains("stroke:#107c10;stroke-width:2px"), "{taken}");
        assert!(!untaken.contains("stroke:#107c10;stroke-width:2px"));
    }

    #[test]
    fn a_hostile_label_cannot_inject_script_into_the_svg() {
        let plan = "flowchart LR\n  a[\"<script>x</script> onload=x\"] --> b\n";
        let flow = crate::flow::parse::parse(plan).expect("plan");
        let svg = render(plan, &flow, &State::default(), Theme::Light).expect("render");
        assert!(!svg.contains("<script"), "a script element survived");
        assert!(svg.contains("onload=x"), "literal label text was lost");
    }

    #[test]
    fn sanitisation_keeps_the_markers_the_graph_needs() {
        let svg = svg("{}", Theme::Light);
        assert!(
            svg.contains("marker-end=\"url(#"),
            "arrow markers were stripped"
        );
        assert!(svg.contains("<foreignObject"), "labels were stripped");
    }

    #[test]
    fn external_references_and_links_are_removed() {
        let dirty = r##"<svg><a href="https://evil.example/x"><text>hi</text></a>
            <image xlink:href="http://evil.example/p.png"/>
            <rect onclick="steal()" fill="#fff"/></svg>"##;
        let clean = sanitize(dirty);
        assert!(!clean.contains("evil.example"));
        assert!(!clean.contains("onclick"));
        assert!(clean.contains("fill=\"#fff\""), "{clean}");
    }

    #[test]
    fn the_on_rule_is_fail_closed_and_does_not_touch_other_attributes() {
        let kept =
            drop_attributes(r##"<g onclick="x()" onfuturevent="y()" fill="#fff" stroke="a"/>"##);
        assert!(!kept.contains("onclick"), "{kept}");
        assert!(!kept.contains("onfuturevent"), "{kept}");
        assert!(kept.contains("fill=\"#fff\""), "{kept}");
        assert!(kept.contains("stroke=\"a\""), "{kept}");
    }

    #[test]
    fn the_word_on_in_text_is_not_mistaken_for_an_attribute() {
        for text in [
            "<text>waiting on CI</text>",
            "<text>Run oncall=primary</text><g data-id=\"b\"/>",
            "<text>x' onmouseover='y</text><g data-id=\"b\"/>",
            "<text>&lt;img onerror=bad&gt;</text><g data-id=\"b\"/>",
            "<g title=\"literal onload=text > value\"/>",
        ] {
            assert_eq!(drop_attributes(text), text);
        }
    }

    #[test]
    fn event_attribute_removal_respects_quotes_and_tag_boundaries() {
        for dirty in [
            "<g onload='bad' fill=\"red\"/>",
            "<g onload=bad fill=\"red\"/>",
            "<g title=\"a > b\" onload=\"bad\" fill=\"red\"/>",
            "<g onload=\"bad\" onfuture='bad' fill=\"red\"/>",
        ] {
            let clean = drop_attributes(dirty);
            assert!(!clean.contains(" onload="), "{clean}");
            assert!(!clean.contains(" onfuture="), "{clean}");
            assert!(clean.contains("fill=\"red\"/>"), "{clean}");
        }
    }

    #[test]
    fn the_dark_theme_restyles_what_class_def_cannot_reach() {
        let dark = svg("{}", Theme::Dark);
        assert!(dark.starts_with("<svg"));
        assert!(dark.contains(".edgeLabel"), "edge labels were not restyled");
        assert!(
            dark.contains(".cluster rect"),
            "subgraphs were not restyled"
        );
        // Outside the root element the browser would ignore it.
        let style_at = dark.rfind(".edgeLabel").expect("override");
        let close_at = dark.rfind("</svg>").expect("closing tag");
        assert!(
            style_at < close_at,
            "the override was appended outside the document"
        );

        let light = svg("{}", Theme::Light);
        assert!(light.starts_with("<svg"));
        assert!(
            !light.contains("#1f1f1f"),
            "the dark override leaked into the light theme"
        );
    }

    #[test]
    fn unicode_labels_survive_rendering() {
        let plan = "flowchart LR\n  a[\"Ünïcödé ✓\"] --> b[Ok]\n";
        let flow = crate::flow::parse::parse(plan).expect("plan");
        let svg = render(plan, &flow, &State::default(), Theme::Light).expect("render");
        assert!(svg.contains("Ünïcödé ✓"));
    }
}

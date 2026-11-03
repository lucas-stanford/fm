//! Pins the merman behaviours fm's decoration depends on, which is why merman is
//! pinned to an exact version. If one of these fails after an upgrade,
//! `src/render/decorate.rs` or `src/render/svg.rs` needs to change with it.

use merman::{OperationControl, RenderOutput, RenderRequest, Renderer};

const PLAN: &str = "flowchart LR\n  \
    fetch[Fetch source] --> build[Build binary] --> test{Tests pass?}\n  \
    test -->|yes| deploy[Deploy]\n  \
    test -->|no| fix[Fix and retry] --> build\n";

/// The shape of what `decorate.rs` appends.
const DECORATION: &str = "\n  %% fm:decoration\n  \
    classDef fm_pending fill:#f3f2f1,stroke:#8a8886,color:#605e5c\n  \
    classDef fm_active fill:#fff4ce,stroke:#c77700,color:#3b3a39\n  \
    classDef fm_done fill:#dff6dd,stroke:#107c10,color:#3b3a39\n  \
    fetch@{ label: \"Fetch source<br/>DONE\" }\n  \
    build@{ label: \"Build binary<br/>ACTIVE\" }\n  \
    class fetch fm_done\n  \
    class build fm_active\n  \
    class test,deploy,fix fm_pending\n  \
    linkStyle 0 stroke:#107c10,stroke-width:2px\n";

fn render_svg(source: &str) -> String {
    let output = Renderer::new()
        .render(RenderRequest::svg(
            source,
            OperationControl::new(),
            merman::SvgRequest::default(),
        ))
        .expect("render must succeed");
    match output {
        RenderOutput::Svg(Some(svg)) => svg.svg().to_string(),
        other => panic!("expected SVG output, got {other:?}"),
    }
}

fn decorated() -> String {
    format!("{PLAN}{DECORATION}")
}

#[test]
fn late_label_override_wins_over_the_authored_label() {
    let svg = render_svg(&decorated());
    assert!(
        svg.contains("DONE") && svg.contains("ACTIVE"),
        "a late label override did not reach the SVG"
    );
    assert!(
        svg.contains("Deploy"),
        "an undecorated node lost its authored label"
    );
}

#[test]
fn class_def_becomes_a_scoped_css_rule() {
    let svg = render_svg(&decorated());
    assert!(
        svg.contains(".fm_done") && svg.contains("#dff6dd"),
        "classDef fm_done did not become a CSS rule"
    );
}

#[test]
fn class_assignment_lands_on_the_node_group() {
    let svg = render_svg(&decorated());
    for (id, class) in [
        ("fetch", "fm_done"),
        ("build", "fm_active"),
        ("deploy", "fm_pending"),
        ("test", "fm_pending"),
        ("fix", "fm_pending"),
    ] {
        let group = node_group(&svg, id);
        assert!(
            group.contains(class),
            "node '{id}' is missing class '{class}': {group}"
        );
    }
}

#[test]
fn link_style_highlights_exactly_the_named_edge() {
    let svg = render_svg(&decorated());
    // Edge 0 in declaration order is fetch --> build.
    let taken = edge_path(&svg, "L_fetch_build_0");
    assert!(
        taken.contains("stroke:#107c10"),
        "linkStyle 0 did not style the first declared edge: {taken}"
    );
    let untaken = edge_path(&svg, "L_test_deploy_0");
    assert!(
        !untaken.contains("stroke:#107c10"),
        "linkStyle 0 leaked onto an edge it does not name: {untaken}"
    );
}

#[test]
fn nodes_carry_a_stable_merman_id_but_no_data_id() {
    let svg = render_svg(PLAN);
    for id in ["fetch", "build", "test", "deploy", "fix"] {
        assert!(
            svg.contains(&format!("id=\"merman-flowchart-{id}-")),
            "node '{id}' lost its merman id"
        );
        assert!(
            !svg.contains(&format!("data-id=\"{id}\"")),
            "merman now emits data-id on nodes; svg.rs need not add it"
        );
    }
}

#[test]
fn node_id_survives_hyphens_and_digits() {
    // svg.rs strips exactly one trailing `-<index>` to recover the node id.
    let svg = render_svg("flowchart LR\n  my-node-1[First] --> b2[Second]\n");
    assert!(svg.contains("id=\"merman-flowchart-my-node-1-0\""));
    assert!(svg.contains("id=\"merman-flowchart-b2-1\""));
}

#[test]
fn edges_already_carry_a_data_id() {
    let svg = render_svg(PLAN);
    assert!(
        svg.contains("data-id=\"L_fetch_build_0\""),
        "edges lost their data-id; a host page can no longer address them"
    );
}

#[test]
fn subgraphs_render_as_identified_clusters() {
    let svg = render_svg(
        "flowchart TD\n  subgraph s1[Prepare]\n    a[A] --> b[B]\n  end\n  b --> c[C]\n",
    );
    assert!(
        svg.contains("class=\"cluster\""),
        "subgraph lost its cluster"
    );
    assert!(svg.contains("id=\"merman-s1\""), "subgraph lost its id");
    assert!(
        svg.contains("id=\"merman-flowchart-a-0\""),
        "nested node lost its id"
    );
}

/// `fixtures/plans/exits.mmd` is the plan that exercises fm's text fallback. When merman learns
/// to route it, pick another plan merman refuses, or the rail goes untested end to end.
#[test]
fn the_ascii_renderer_still_cannot_route_the_exits_fixture() {
    let plan = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/plans/exits.mmd"),
    )
    .expect("exits fixture");
    let outcome = Renderer::new().render(RenderRequest::ascii(
        &plan,
        OperationControl::new(),
        merman::AsciiRequest::default(),
    ));
    assert!(
        outcome.is_err(),
        "merman now draws exits.mmd as text; it no longer exercises the rail"
    );
}

/// Returns the opening `<g …>` tag of a node group, by the merman id scheme.
fn node_group<'a>(svg: &'a str, node_id: &str) -> &'a str {
    tag_at(svg, &format!("id=\"merman-flowchart-{node_id}-"), node_id)
}

/// Returns the opening `<path …>` tag of an edge, by its merman edge id.
fn edge_path<'a>(svg: &'a str, edge_id: &str) -> &'a str {
    tag_at(svg, &format!("id=\"merman-{edge_id}\""), edge_id)
}

fn tag_at<'a>(svg: &'a str, needle: &str, what: &str) -> &'a str {
    let at = svg
        .find(needle)
        .unwrap_or_else(|| panic!("'{what}' not found in the SVG"));
    let start = svg[..at].rfind('<').expect("opening tag");
    let end = at + svg[at..].find('>').expect("closing bracket") + 1;
    &svg[start..end]
}

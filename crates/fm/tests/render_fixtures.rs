//! Every plan in `fixtures/plans/` rendered through `fm render`.
//!
//! No golden output: it would churn with every merman layout change. The
//! renderer behaviours fm relies on are pinned in `tests/merman_seam.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::cargo::cargo_bin;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/plans")
}

fn valid_plans() -> Vec<PathBuf> {
    let mut plans: Vec<PathBuf> = std::fs::read_dir(fixtures())
        .expect("fixtures/plans")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "mmd"))
        .filter(|path| {
            !matches!(
                path.file_stem().and_then(|stem| stem.to_str()),
                Some("invalid") | Some("unsupported")
            )
        })
        .collect();
    plans.sort();
    assert!(plans.len() >= 9, "the fixture corpus shrank");
    plans
}

fn fm(args: &[&str]) -> Output {
    Command::new(cargo_bin("fm"))
        .args(args)
        .output()
        .expect("fm runs")
}

fn render(plan: &Path, extra: &[&str]) -> String {
    let plan = plan.display().to_string();
    let mut args = vec!["render", "--plan", plan.as_str()];
    args.extend_from_slice(extra);
    let output = fm(&args);
    assert!(
        output.status.success(),
        "{plan} {extra:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(
        !stdout.trim().is_empty(),
        "{plan} {extra:?} rendered nothing"
    );
    stdout
}

/// Node ids, recovered from the `data-id` attributes fm adds to the SVG.
fn node_ids(svg: &str) -> Vec<String> {
    svg.split("data-id=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|id| !id.starts_with("L_"))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_fixture_plan_renders_as_text_and_svg_with_a_state() {
    let dir = tempfile::tempdir().expect("temp");
    for plan in valid_plans() {
        let svg = render(&plan, &["--format", "svg"]);
        assert!(svg.starts_with("<svg"), "{}", plan.display());
        let ids = node_ids(&svg);
        assert!(!ids.is_empty(), "{} has no node ids", plan.display());

        // A mixed state, so the decoration exercises several status classes.
        let statuses = ["done", "active", "failed", "waiting", "skipped"];
        let nodes: Vec<String> = ids
            .iter()
            .zip(statuses)
            .map(|(id, status)| format!("\"{id}\": \"{status}\""))
            .collect();
        let state = dir.path().join("state.json");
        std::fs::write(&state, format!("{{\"nodes\": {{{}}}}}", nodes.join(", "))).unwrap();
        let state = state.display().to_string();

        for format in ["svg", "text"] {
            let rendered = render(&plan, &["--state", &state, "--format", format]);
            assert!(
                !rendered.contains('\u{1b}'),
                "{} leaked escape codes into {format}",
                plan.display()
            );
        }
        let text = render(&plan, &["--state", &state]);
        assert!(text.contains('✓'), "{}:\n{text}", plan.display());
    }
}

#[test]
fn a_plan_merman_cannot_route_is_drawn_as_a_rail_with_every_arrow() {
    let text = render(&fixtures().join("exits.mmd"), &[]);
    for exit in [
        "──▼ Write the report (nothing to do)",
        "──▲ Build (first failure)",
        "──▼ Write the report (second failure)",
        "──▼ Write the report (no review)",
        "──▼ Write the report (blocked)",
        "──▼ Close",
        "│ release",
    ] {
        assert!(text.contains(exit), "missing {exit:?}:\n{text}");
    }
}

#[test]
fn the_invalid_fixture_is_refused_with_a_position() {
    let plan = fixtures().join("invalid.mmd").display().to_string();
    let output = fm(&["render", "--plan", &plan]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("plan: 3:"), "no line:col: {stderr}");
}

#[test]
fn the_unsupported_fixture_explains_why_it_is_reserved() {
    let plan = fixtures().join("unsupported.mmd").display().to_string();
    let output = fm(&["render", "--plan", &plan]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("reserved for fm"), "unhelpful: {stderr}");
}

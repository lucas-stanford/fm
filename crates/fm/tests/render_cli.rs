//! `fm render` end to end: arguments, stdin, state files, output and exit codes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use assert_cmd::cargo::cargo_bin;

fn branch() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/plans/branch.mmd")
        .display()
        .to_string()
}

const STATE: &str = r#"{
  "nodes": {"start": "done", "check": "done", "ship": "active", "fix": "skipped"},
  "taken": [["start", "check"], ["check", "ship"]],
  "unknown": "ignored"
}"#;

fn fm(args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(cargo_bin("fm"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fm runs");
    let mut pipe = child.stdin.take().expect("stdin");
    if let Some(input) = stdin {
        pipe.write_all(input.as_bytes()).expect("write stdin");
    }
    drop(pipe);
    child.wait_with_output().expect("fm exits")
}

fn ok(output: &Output) -> String {
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "noise on stderr");
    String::from_utf8(output.stdout.clone()).expect("utf-8")
}

fn failed(output: &Output, code: i32) -> String {
    assert_eq!(output.status.code(), Some(code), "wrong exit status");
    assert!(
        output.stdout.is_empty(),
        "stdout must stay empty on failure"
    );
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn state_file(dir: &tempfile::TempDir, json: &str) -> String {
    let path = dir.path().join("state.json");
    std::fs::write(&path, json).expect("write state");
    path.display().to_string()
}

#[test]
fn text_is_the_default_and_carries_status_glyphs() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, STATE);
    let text = ok(&fm(
        &["render", "--plan", &branch(), "--state", &state],
        None,
    ));
    assert!(!text.contains('\u{1b}'));
    assert!(!text.contains("<svg"));
    let line_of = |label: &str| {
        text.lines()
            .find(|line| line.contains(label))
            .unwrap_or_else(|| panic!("no {label} in:\n{text}"))
            .to_string()
    };
    assert!(line_of("Start").contains('✓'), "{text}");
    assert!(line_of("Ship it").contains('▶'), "{text}");
    assert!(line_of("Fix it").contains('↷'), "{text}");
    assert!(line_of("Ask a human").contains('·'), "{text}");
}

#[test]
fn markdown_is_a_mermaid_block_with_status_colours() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, STATE);
    let md = ok(&fm(
        &[
            "render",
            "--plan",
            &branch(),
            "--state",
            &state,
            "--format",
            "markdown",
        ],
        None,
    ));
    assert!(md.starts_with("```mermaid\nflowchart LR\n"), "{md}");
    assert!(md.trim_end().ends_with("```"), "{md}");
    for class in ["fm_done", "fm_active", "fm_skipped", "fm_pending"] {
        assert!(md.contains(class), "no {class}");
    }
    // Light by default, so it reads on a light page; the taken arrows are highlighted.
    assert!(md.contains("#107c10"), "{md}");
    assert!(md.contains("linkStyle "), "{md}");
    // The author's labels are left as written.
    assert!(!md.contains("@{"), "{md}");
}

#[test]
fn svg_is_standalone_themed_and_marks_taken_edges() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, STATE);
    let svg = ok(&fm(
        &[
            "render",
            "--plan",
            &branch(),
            "--state",
            &state,
            "--format",
            "svg",
        ],
        None,
    ));
    assert!(svg.starts_with("<svg"), "{svg}");
    assert!(svg.trim_end().ends_with("</svg>"));
    for class in ["fm_done", "fm_active", "fm_skipped", "fm_pending"] {
        assert!(svg.contains(class), "no {class}");
    }
    // Dark is the default: its taken-edge colour and its label override.
    assert!(svg.contains("#4ad04a"), "taken edges are not highlighted");
    assert!(svg.contains("#1f1f1f"), "the dark theme was not applied");
    assert!(svg.contains("data-id=\"ship\""));

    let light = ok(&fm(
        &[
            "render",
            "--plan",
            &branch(),
            "--state",
            &state,
            "--format",
            "svg",
            "--theme",
            "light",
        ],
        None,
    ));
    assert!(light.contains("#107c10"), "light taken-edge colour missing");
    assert!(!light.contains("#1f1f1f"));
}

#[test]
fn no_state_means_everything_is_pending() {
    let text = ok(&fm(&["render", "--plan", &branch()], None));
    assert!(!text.contains('✓'));
    assert!(text.contains('·'));
}

#[test]
fn the_plan_can_come_from_stdin() {
    let plan = std::fs::read_to_string(branch()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, STATE);
    let text = ok(&fm(
        &["render", "--plan", "-", "--state", &state],
        Some(&plan),
    ));
    assert!(text.contains("Ship it ▶"), "{text}");
}

#[test]
fn the_state_can_come_from_stdin() {
    let text = ok(&fm(
        &["render", "--plan", &branch(), "--state", "-"],
        Some(STATE),
    ));
    assert!(text.contains("Ship it ▶"), "{text}");
}

#[test]
fn a_byte_order_mark_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, &format!("\u{feff}{STATE}"));
    ok(&fm(
        &["render", "--plan", &branch(), "--state", &state],
        None,
    ));
}

#[test]
fn out_writes_the_file_and_nothing_to_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("graph.svg");
    let stdout = ok(&fm(
        &[
            "render",
            "--plan",
            &branch(),
            "--format",
            "svg",
            "--out",
            &out.display().to_string(),
        ],
        None,
    ));
    assert!(stdout.is_empty());
    assert!(std::fs::read_to_string(&out).unwrap().starts_with("<svg"));
}

#[test]
fn an_unknown_node_in_the_state_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, r#"{"nodes": {"deploy": "done"}}"#);
    let stderr = failed(
        &fm(&["render", "--plan", &branch(), "--state", &state], None),
        2,
    );
    assert!(stderr.contains("'deploy'"), "{stderr}");

    let state = state_file(&dir, r#"{"taken": [["start", "nowhere"]]}"#);
    let stderr = failed(
        &fm(&["render", "--plan", &branch(), "--state", &state], None),
        2,
    );
    assert!(stderr.contains("'nowhere'"), "{stderr}");
}

#[test]
fn an_unknown_status_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, r#"{"nodes": {"start": "finished"}}"#);
    let stderr = failed(
        &fm(&["render", "--plan", &branch(), "--state", &state], None),
        2,
    );
    assert!(stderr.contains("finished"), "{stderr}");
}

#[test]
fn bad_json_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_file(&dir, "{\"nodes\": ");
    let stderr = failed(
        &fm(&["render", "--plan", &branch(), "--state", &state], None),
        2,
    );
    assert!(stderr.contains("not valid JSON"), "{stderr}");
}

#[test]
fn a_missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.mmd").display().to_string();
    let stderr = failed(&fm(&["render", "--plan", &missing], None), 1);
    assert!(stderr.contains("missing.mmd"), "{stderr}");
}

#[test]
fn render_without_a_plan_is_a_usage_error() {
    let stderr = failed(&fm(&["render"], None), 2);
    assert!(stderr.contains("Usage: fm render"), "{stderr}");
}

#[test]
fn an_unknown_command_is_refused() {
    let stderr = failed(&fm(&["no-such-command"], None), 1);
    assert!(stderr.contains("no-such-command"), "{stderr}");
}

#[test]
fn help_and_version() {
    let help = ok(&fm(&["--help"], None));
    assert!(
        help.contains("render") && help.contains("Draw a Mermaid flowchart"),
        "{help}"
    );
    let render_help = ok(&fm(&["render", "--help"], None));
    assert!(render_help.contains("--state <file|->"));
    assert!(render_help.contains("interrupted"));
    let version = ok(&fm(&["--version"], None));
    assert_eq!(version.trim(), format!("fm {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn validate_checks_one_file_as_the_specification_says() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("demo");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("AUTOMATION.md");
    std::fs::write(
        &file,
        "---\nname: demo\ndescription: d\non:\n  manual: true\n---\n\nRun.\n",
    )
    .unwrap();
    let folder_arg = folder.display().to_string();
    assert_eq!(
        ok(&fm(&["validate", &folder_arg], None)).trim(),
        "demo: valid"
    );

    std::fs::write(
        &file,
        "---\nname: demo\ndescription: d\nbudget: 5\non:\n  manual: true\n---\n\nRun.\n",
    )
    .unwrap();
    let stderr = failed(&fm(&["validate", &file.display().to_string()], None), 2);
    assert_eq!(
        stderr.trim(),
        "budget: unknown field — Draft 0.2.0 has name, description, enabled, model, timeout, on, hooks, flow, nodes"
    );
    let out = fm(
        &["validate", &folder_arg, "--json", "--name", "other"],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .starts_with("budget: unknown field"),
        "{v}"
    );
}

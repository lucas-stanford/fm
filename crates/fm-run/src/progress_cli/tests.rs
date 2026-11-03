//! Ported from pi-automations' `progress.test.ts`, `message.test.ts` and `flow.test.ts`: the
//! in-run verbs as an agent calls them, with only the environment a run hands it.

use chrono::TimeZone;
use fm_model::StatePaths;
use fm_state::{FakeEnv, OpenArgs, open_progress, progress_message, progress_status};
use serde_json::Value;

use super::*;
use crate::tracker::chart_nodes;

const CHART: &str = "flowchart TD
  %% the run moves pre0 and body.
  pre0[\"Pick the next issue\"] --> body[\"Verify the issue\"]
  body --> read[\"Read the issue\"]
  read --> reproduce[\"Try it on production\"]
  read -->|no browser needed| draft[\"Draft the verdict\"]
  reproduce --> draft
  draft --> post0[\"Record the run\"]
";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 17, 14, 0, 0).unwrap()
}

struct Opened {
    _dir: tempfile::TempDir,
    paths: StatePaths,
    run: String,
}

fn opened_with(chart: &str, strict: bool) -> Opened {
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let run = "20260917-140000-a1b2".to_string();
    open_progress(
        &paths,
        &OpenArgs {
            run_id: run.clone(),
            key: "user:/r:demo".into(),
            name: "demo".into(),
            chart: chart.into(),
            nodes: chart_nodes(chart),
            strict,
        },
        now(),
    )
    .unwrap();
    Opened {
        _dir: dir,
        paths,
        run,
    }
}

fn opened() -> Opened {
    opened_with(CHART, false)
}

impl Opened {
    fn env(&self) -> FakeEnv {
        FakeEnv::default()
            .set("FM_RUN", &self.run)
            .set("FM_STATE_DIR", self.paths.root.to_string_lossy())
    }
}

fn cli(args: &[&str], env: &FakeEnv) -> CliOutput {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    progress_cli(&args, env, now())
}

fn json(out: &CliOutput) -> Value {
    serde_json::from_str(out.stdout.trim()).unwrap_or_else(|e| panic!("{e}: {out:?}"))
}

fn ok(stdout: &str) -> CliOutput {
    CliOutput {
        code: 0,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

#[test]
fn an_agent_reports_with_only_the_run_environment() {
    let o = opened();
    let env = o.env();
    let r = cli(&["step", "read", "-m", "Reading #12"], &env);
    assert_eq!(r, ok("read active\n"));
    assert_eq!(
        cli(&["step", "read"], &env).stdout,
        "read active (unchanged)\n"
    );
    let r = cli(
        &["next", "draft", "--from", "read", "-m", "no browser needed"],
        &env,
    );
    assert_eq!(r.stdout, "draft active, read done\n");
    assert_eq!(cli(&["say", "found", "it"], &env).stdout, "said on draft\n");
    let r = cli(
        &["step", "draft", "--status=done", "--message", "reproduced"],
        &env,
    );
    assert_eq!(r.stdout, "draft done\n");
    let r = cli(
        &[
            "note",
            "Drafted the verdict",
            "--link",
            "issue=https://x.test/issues/12",
            "--link=https://x.test/d",
        ],
        &env,
    );
    assert_eq!(r.stdout, "noted with 2 links\n");

    let s = progress_status(&o.paths, Some(&o.run)).unwrap();
    assert_eq!(s.taken, [("read".to_string(), "draft".to_string())]);
    let draft = s.node("draft").unwrap();
    assert_eq!(draft.status, StepStatus::Done);
    assert_eq!(draft.text.as_deref(), Some("reproduced"));
    assert_eq!(s.notes[0].links.len(), 2);
}

#[test]
fn json_output_is_one_object_ok_or_not() {
    let o = opened();
    let env = o.env();
    let v = json(&cli(&["step", "read", "--json"], &env));
    assert_eq!(v["ok"], true);
    assert_eq!(v["changed"], true);
    assert_eq!(v["warnings"], Value::Array(vec![]));
    assert_eq!(v["data"]["run"], o.run.as_str());
    assert_eq!(v["data"]["node"]["id"], "read");
    assert_eq!(v["data"]["node"]["status"], "active");
    assert_eq!(v["data"]["node"]["text"], Value::Null);
    let said = json(&cli(&["say", "found it", "--json"], &env));
    assert_eq!(said["data"]["node"]["text"], "found it");
    let noted = json(&cli(&["note", "n", "--json"], &env));
    assert_eq!(noted["data"]["notes"], 1);

    let status = json(&cli(&["status", "--json"], &env));
    assert_eq!(status["ok"], true);
    assert!(status.get("changed").is_none());
    let ids: Vec<&str> = status["data"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["pre0", "body", "read", "reproduce", "draft", "post0"]);
    assert_eq!(status["data"]["closed"], false);

    let bad = cli(&["step", "raed", "--json"], &env);
    assert_eq!(bad.code, 2);
    assert_eq!(bad.stderr, "");
    let v = json(&bad);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "UNKNOWN_NODE");
    assert!(
        v["error"]["suggestions"][0]
            .as_str()
            .unwrap()
            .contains("read")
    );
}

#[test]
fn strict_refuses_through_the_cli_with_exit_2_and_a_loose_run_takes_any_move() {
    let o = opened_with(CHART, true);
    let env = o.env();
    assert_eq!(cli(&["step", "draft"], &env).code, 0);
    let r = cli(&["next", "read", "--from", "draft"], &env);
    assert_eq!(r.code, 2);
    assert_eq!(
        r.stderr,
        "fm: the chart has no arrow draft → read; from draft it goes to post0 — follow the chart's arrows\n  did you mean: post0\n"
    );
    let v = json(&cli(&["next", "read", "--from", "draft", "--json"], &env));
    assert_eq!(v["error"]["code"], "NOT_AN_EDGE");

    let loose = opened();
    let env = loose.env();
    assert_eq!(cli(&["step", "draft"], &env).code, 0);
    assert_eq!(cli(&["next", "read", "--from", "draft"], &env).code, 0);
}

#[test]
fn next_comes_from_fm_node_else_the_one_running_node() {
    let o = opened();
    let env = o.env();
    cli(&["step", "read"], &env);
    assert_eq!(
        cli(&["next", "reproduce"], &env).stdout,
        "reproduce active, read done\n"
    );
    let with_node = o.env().set("FM_NODE", "reproduce");
    assert_eq!(
        cli(&["next", "draft"], &with_node).stdout,
        "draft active, reproduce done\n"
    );
    cli(&["step", "body"], &env);
    let r = cli(&["next", "post0"], &env);
    assert_eq!(r.code, 2);
    assert!(
        r.stderr
            .starts_with("fm: 2 nodes are running — say which one with --from\n  did you mean: "),
        "{r:?}"
    );
}

#[test]
fn messages_and_links() {
    let o = opened();
    let env = o.env();
    let human = cli(
        &[
            "message",
            "Digest is up",
            "--link",
            "digest=https://x.test/d/1",
        ],
        &env,
    );
    assert_eq!(human, ok("message left with 1 link\n"));
    let v = json(&cli(&["message", "Digest is up, 2 wait", "--json"], &env));
    assert_eq!(v["ok"], true);
    assert_eq!(v["changed"], true);
    assert_eq!(v["data"]["message"]["text"], "Digest is up, 2 wait");
    assert_eq!(v["data"]["message"]["links"], Value::Array(vec![]));
    assert_eq!(cli(&["message", "--json"], &env).code, 1);
    let bad = cli(&["message", "x", "--link", "file:///x", "--json"], &env);
    assert_eq!(bad.code, 1);
    assert_eq!(json(&bad)["error"]["code"], "BAD_LINK");

    let status = json(&cli(&["status", "--json"], &env));
    assert_eq!(status["data"]["message"]["text"], "Digest is up, 2 wait");

    progress_message(
        &o.paths,
        Some(&o.run),
        &LinkedArgs {
            text: "Final\u{7} word".into(),
            links: vec!["pr=https://x.test/pr/7".into()],
            request_id: None,
        },
        now(),
    )
    .unwrap();
    let lines = status_lines(&progress_status(&o.paths, Some(&o.run)).unwrap());
    assert_eq!(
        lines[lines.len() - 2..],
        ["message Final word", "        pr: https://x.test/pr/7"]
    );
}

#[test]
fn a_run_with_no_chart_takes_notes_and_refuses_moves_by_name() {
    let o = opened_with("", false);
    let env = o.env();
    for args in [["step", "body", "--json"], ["say", "hello", "--json"]] {
        let r = cli(&args, &env);
        assert_eq!(r.code, 2);
        assert_eq!(json(&r)["error"]["code"], "NO_CHART");
    }
    let r = cli(
        &[
            "note",
            "Posted the summary",
            "--link",
            "summary=https://x.test/s/1",
        ],
        &env,
    );
    assert_eq!(r.code, 0, "{r:?}");
}

#[test]
fn request_ids_make_retries_no_ops() {
    let o = opened();
    let env = o.env();
    cli(&["step", "read"], &env);
    assert_eq!(
        cli(&["say", "one", "--request-id", "k1"], &env).stdout,
        "said on read\n"
    );
    assert_eq!(
        cli(&["say", "one", "--request-id", "k1"], &env).stdout,
        "said on read (already said)\n"
    );
    assert_eq!(
        cli(&["note", "n", "--request-id", "n1"], &env).stdout,
        "noted\n"
    );
    assert_eq!(
        cli(&["note", "n", "--request-id", "n1"], &env).stdout,
        "noted (already noted)\n"
    );
    assert_eq!(
        cli(&["message", "m", "--request-id", "m1"], &env).stdout,
        "message left\n"
    );
    assert_eq!(
        cli(&["message", "m", "--request-id", "m1"], &env).stdout,
        "message left (already left)\n"
    );
}

#[test]
fn usage_is_exit_1_and_refusals_exit_2() {
    let o = opened();
    let env = o.env();
    for args in [
        vec!["step"],
        vec!["step", "a", "b"],
        vec!["step", "read", "--status", "interrupted"],
        vec!["next"],
        vec!["say"],
        vec!["say", "   "],
        vec!["note"],
        vec!["status", "extra"],
        vec!["step", "read", "--bogus"],
        vec!["step", "read", "-m"],
        vec!["dance"],
    ] {
        let r = cli(&args, &env);
        assert_eq!(r.code, 1, "{args:?}: {r:?}");
        assert!(r.stderr.starts_with("fm: "), "{r:?}");
        assert_eq!(r.stderr.matches('\n').count(), 1, "{r:?}");
    }
    assert_eq!(
        cli(&["step", "read", "--status", "nope"], &env).stderr,
        "fm: --status must be one of active|waiting|done|failed|skipped\n"
    );
    // Words after `--` are text, even when they look like flags.
    let r = cli(&["say", "--node", "read", "--", "--not-a-flag"], &env);
    assert_eq!(r.code, 0, "{r:?}");
    let s = progress_status(&o.paths, Some(&o.run)).unwrap();
    assert_eq!(
        s.node("read").unwrap().text.as_deref(),
        Some("--not-a-flag")
    );
}

#[test]
fn finds_the_run_from_the_environment_or_says_why_not() {
    let o = opened();
    let no_run = FakeEnv::default().set("FM_STATE_DIR", o.paths.root.to_string_lossy());
    let r = cli(&["status"], &no_run);
    assert_eq!(r.code, 2);
    assert!(
        r.stderr
            .starts_with("fm: not inside a run — FM_RUN is not set"),
        "{r:?}"
    );
    assert_eq!(
        json(&cli(&["status", "--json"], &no_run))["error"]["code"],
        "NO_RUN"
    );
    assert_eq!(cli(&["status", "--run", &o.run], &no_run).code, 0);

    // No FM_STATE_DIR: the state folder, from FM_HOME.
    let home = FakeEnv::default()
        .set("FM_RUN", &o.run)
        .set("FM_HOME", o.paths.root.to_string_lossy());
    assert_eq!(cli(&["status"], &home).code, 0);

    let elsewhere = tempfile::tempdir().unwrap();
    let wrong = FakeEnv::default()
        .set("FM_RUN", &o.run)
        .set("FM_STATE_DIR", elsewhere.path().to_string_lossy());
    let v = json(&cli(&["status", "--json"], &wrong));
    assert_eq!(v["error"]["code"], "UNKNOWN_RUN");
    let escape = FakeEnv::default()
        .set("FM_RUN", "../secrets")
        .set("FM_STATE_DIR", elsewhere.path().to_string_lossy());
    assert_eq!(
        json(&cli(&["status", "--json"], &escape))["error"]["code"],
        "UNKNOWN_RUN"
    );
}

#[test]
fn long_text_is_kept_short_with_a_warning() {
    let o = opened();
    let env = o.env();
    let long = "x".repeat(fm_state::progress::TEXT_MAX + 10);
    let r = cli(&["step", "read", "-m", &long], &env);
    assert_eq!(r.code, 0);
    assert!(r.stderr.starts_with("fm: text was "), "{r:?}");
    let v = json(&cli(&["say", &long, "--json"], &env));
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn status_is_readable_and_carries_no_controls() {
    let o = opened_with(
        "flowchart TD\n  a[\"label\u{1b}]8;;https://x\u{7}\"] --> b\n",
        false,
    );
    let env = o.env();
    cli(&["step", "a", "-m", "line one\nline\u{1b}[2J two"], &env);
    cli(&["next", "b"], &env);
    cli(
        &["note", "posted", "--link", "re\u{1b}view=https://x.test/1"],
        &env,
    );
    let out = cli(&["status"], &env);
    assert_eq!(out.code, 0);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines[0], format!("demo · run {}", o.run));
    assert!(lines[1].starts_with("  ✓ a   done      label"), "{lines:?}");
    assert!(lines[1].ends_with(" — line one line[2J two"), "{lines:?}");
    assert_eq!(lines[2], "  ▶ b   active");
    assert_eq!(lines[3], "taken  a→b");
    assert_eq!(lines[4], "note   posted");
    assert!(
        lines[5].starts_with("       review: https://x.test/1"),
        "{lines:?}"
    );
    assert!(!out.stdout.chars().any(|c| c.is_control() && c != '\n'));
}

#[test]
fn every_verb_is_known_and_the_help_names_them() {
    for v in IN_RUN_VERBS {
        assert!(is_in_run_verb(v));
    }
    assert!(!is_in_run_verb("run"));
    let help = in_run_help_lines().join("\n");
    for v in IN_RUN_VERBS {
        assert!(help.contains(&format!("fm {v} ")), "{v}");
    }
}

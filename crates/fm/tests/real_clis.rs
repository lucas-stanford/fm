//! The end-to-end smoke through the real agent CLIs on this machine: for each one installed, a
//! one-line automation is checked, armed and run, and its record read back. It spends real model
//! calls and needs each CLI signed in, so it is ignored by default:
//!
//! ```text
//! cargo test -p fm --test real_clis -- --ignored --nocapture
//! ```
//!
//! `FM_SMOKE_CLIS=pi,copilot` limits it to those CLIs. Everything fm owns (its state, the home it
//! scans for user automations) is a temporary folder; the automation is repository scope in a
//! temporary git repository. The CLIs keep their real sign-in through their own variables
//! (`COPILOT_HOME`, `PI_CODING_AGENT_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Fx, fm_exe};

/// The CLI's own folder under the real home, for the variable that points it there.
const SIGN_IN: [(&str, &str, &str); 4] = [
    ("copilot", "COPILOT_HOME", ".copilot"),
    ("pi", "PI_CODING_AGENT_DIR", ".pi/agent"),
    ("claude", "CLAUDE_CONFIG_DIR", ".claude"),
    ("codex", "CODEX_HOME", ".codex"),
];

struct Smoke {
    fx: Fx,
    real_home: PathBuf,
}

impl Smoke {
    fn new() -> Smoke {
        let real_home = fm::env::home_dir().expect("a home directory");
        let fx = Fx::new();
        let git = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&fx.cwd)
            .status()
            .expect("git");
        assert!(git.success());
        Smoke { fx, real_home }
    }

    fn fm(&self, args: &[&str]) -> Output {
        let mut c = Command::new(fm_exe());
        c.args(args)
            .current_dir(&self.fx.cwd)
            .env("FM_HOME", &self.fx.state)
            .env("FM_NO_NOTIFY", "1")
            .env("HOME", &self.fx.home)
            .env("USERPROFILE", &self.fx.home)
            .env("PI_AGENT_DIR", &self.fx.pa)
            .env_remove("AUTOMATION_RUN")
            .env_remove("FM_BIN");
        for (_, var, dir) in SIGN_IN {
            let real = self.real_home.join(dir);
            if std::env::var_os(var).is_none() && real.is_dir() {
                c.env(var, real);
            }
        }
        c.output().expect("fm runs")
    }

    /// The CLIs `fm agent list` finds: a version where a missing one shows `—`.
    fn found(&self) -> Vec<String> {
        let out = self.fm(&["agent", "list"]);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        println!("{text}");
        text.lines()
            .skip(1)
            .take_while(|l| !l.trim().is_empty())
            .filter_map(|l| {
                let mut words = l.trim_start_matches(['*', ' ']).split_whitespace();
                let (cli, version) = (words.next()?, words.next()?);
                let known = cli.parse::<fm_model::CliId>().is_ok();
                (known && version != "—").then(|| cli.to_string())
            })
            .collect()
    }

    fn write(&self, name: &str) {
        let dir = self.fx.repo_root().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("AUTOMATION.md"),
            format!(
                "---\nname: {name}\ndescription: Smoke test. Answers one line.\ntimeout: 3m\non:\n  manual: true\n---\n\nAnswer in one line, without using any tools: what is the capital of France?\n"
            ),
        )
        .unwrap();
    }
}

fn text(o: &Output) -> String {
    format!(
        "exit {:?}\n{}{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// S2 for one CLI: check, arm, run, then the record. `Err` says what went wrong.
fn replay(s: &Smoke, cli: &str) -> Result<String, String> {
    let name = format!("smoke-{cli}");
    s.write(&name);
    let key = s.fx.repo_key(&name);
    let mut config = std::fs::read_to_string(s.fx.state.join("config.toml")).unwrap_or_default();
    config.push_str(&format!(
        "\n[automations.'{key}']\nagent = \"{cli}\"\nworktree = false\n"
    ));
    s.fx.config(&config);

    let check = s.fm(&["check", &name]);
    let shown = text(&check);
    if !check.status.success() || !shown.contains("< <prompt>") {
        return Err(format!("check: {shown}"));
    }
    let arm = s.fm(&["arm", &name, "--yes"]);
    if !arm.status.success() {
        return Err(format!("arm: {}", text(&arm)));
    }
    let run = s.fm(&["run", &name]);
    let ran = text(&run);
    println!("--- {cli}\n{ran}");
    let log = s.fm(&["log", &name, "--json"]);
    let rec: serde_json::Value = String::from_utf8_lossy(&log.stdout)
        .lines()
        .last()
        .and_then(|l| serde_json::from_str(l).ok())
        .ok_or_else(|| format!("no record: {ran}"))?;
    let at = |p: &str| rec.pointer(p).and_then(|v| v.as_str()).unwrap_or("");
    if at("/outcome") != "ok" {
        return Err(format!("outcome {}: {rec}", at("/outcome")));
    }
    let answer = at("/output/text");
    let fine = run.status.code() == Some(0)
        && at("/agent") == cli
        && at("/session/format").starts_with(&format!("{cli}/"))
        && Path::new(at("/session/file")).is_file()
        && answer.contains("Paris")
        && ran.lines().filter(|l| l.trim() == answer.trim()).count() == 1;
    if !fine {
        return Err(format!("record or output not as expected: {rec}\n{ran}"));
    }
    Ok(format!(
        "{cli}: ok, {} {} session {} ({}), answer {answer:?}",
        at("/model"),
        at("/id"),
        at("/session/id"),
        at("/session/format")
    ))
}

#[test]
#[ignore = "spends real model calls through the agent CLIs installed here"]
fn every_installed_cli_runs_an_automation_to_an_ok_record() {
    let s = Smoke::new();
    let only: Option<Vec<String>> = std::env::var("FM_SMOKE_CLIS")
        .ok()
        .map(|v| v.split(',').map(|c| c.trim().to_string()).collect());
    let clis: Vec<String> = s
        .found()
        .into_iter()
        .filter(|c| only.as_ref().is_none_or(|o| o.contains(c)))
        .collect();
    assert!(!clis.is_empty(), "no agent CLI to run here");
    let mut failed = Vec::new();
    for cli in &clis {
        match replay(&s, cli) {
            Ok(line) => println!("{line}"),
            Err(why) => failed.push(format!("{cli}: {why}")),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

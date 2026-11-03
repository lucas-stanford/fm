//! The docs cannot drift from the code: `docs/CLI.md` against `fm --help`, every command's help,
//! the in-run flags and the `FM_*` environment variables, `docs/CONFIG.md` against
//! `Config::default()`, and the README's commands against the real ones.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use common::Fx;
use fm_model::{AutomationSettings, CliSettings, Config};
use serde_json::Value;

fn doc(name: &str) -> String {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(name);
    std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()))
        .replace("\r\n", "\n")
}

fn help(fx: &Fx, path: &[&str]) -> String {
    let out = fx.fm().args(path).arg("--help").output().unwrap();
    assert!(out.status.success(), "fm {} --help failed", path.join(" "));
    String::from_utf8(out.stdout).unwrap()
}

/// The lines of a help section such as `Commands:` or `Options:`, up to the blank line after it.
fn section<'a>(help: &'a str, title: &str) -> Vec<&'a str> {
    help.lines()
        .skip_while(|l| *l != title)
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .collect()
}

/// Subcommand names, without clap's own `help`.
fn commands(help: &str) -> Vec<String> {
    section(help, "Commands:")
        .iter()
        .filter(|l| l.starts_with("  ") && !l.starts_with("   "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| *n != "help")
        .map(str::to_string)
        .collect()
}

/// Flags, long form when there is one: `--json`, `--yes`, `-n`.
fn flags(help: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in section(help, "Options:") {
        let words: Vec<&str> = line
            .split_whitespace()
            .map(|w| w.trim_end_matches(','))
            .take_while(|w| w.starts_with('-') || w.starts_with('<'))
            .filter(|w| w.starts_with('-'))
            .collect();
        let flag = words.iter().find(|w| w.starts_with("--")).or(words.first());
        if let Some(f) = flag
            && !["--help", "-h", "--version", "-V"].contains(f)
        {
            out.push((*f).to_string());
        }
    }
    out
}

/// Every `###` section of CLI.md, as (heading, text). Other headings end a section and are kept
/// with their `#`s, so they can be told apart.
fn h3_sections(md: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in md.lines() {
        if let Some(h) = line.strip_prefix("### ") {
            out.push((h.to_string(), String::new()));
        } else if line.starts_with("## ") || line.starts_with("# ") {
            out.push((line.to_string(), String::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push_str(line);
            last.1.push('\n');
        }
    }
    out
}

#[test]
fn cli_md_covers_every_command_and_flag() {
    let fx = Fx::new();
    let md = doc("docs/CLI.md");
    let top_help = help(&fx, &[]);
    let top = commands(&top_help);
    assert!(top.len() >= 15, "parsed too few commands: {top:?}");

    let mut paths: Vec<Vec<String>> = vec![vec!["fire".into()]];
    for c in &top {
        let h = help(&fx, &[c]);
        let subs = commands(&h);
        if subs.is_empty() {
            paths.push(vec![c.clone()]);
        }
        for s in subs {
            paths.push(vec![c.clone(), s]);
        }
    }

    let sections = h3_sections(&md);
    for path in &paths {
        let words: Vec<&str> = path.iter().map(String::as_str).collect();
        let cmd = format!("fm {}", words.join(" "));
        assert!(md.contains(&cmd), "docs/CLI.md does not mention `{cmd}`");
        let h = help(&fx, &words);
        let text = if path[0] == "fire" {
            md.split("## `fm fire`")
                .nth(1)
                .unwrap_or_default()
                .to_string()
        } else {
            let own = format!("`fm {}`", path[0]);
            sections
                .iter()
                .find(|(h, _)| h.contains(&own))
                .or_else(|| sections.iter().find(|(_, t)| t.contains(&cmd)))
                .map(|(_, t)| t.clone())
                .unwrap_or_default()
        };
        for flag in flags(&h) {
            assert!(
                text.contains(&flag),
                "docs/CLI.md: the section on `{cmd}` does not mention its flag {flag}"
            );
        }
    }

    // Aliases clap prints, such as `[alias: runs]`.
    for line in section(&top_help, "Commands:") {
        if let Some(rest) = line.split("[alias: ").nth(1) {
            let alias = rest.trim_end_matches(']');
            assert!(
                md.contains(&format!("fm {alias}")),
                "docs/CLI.md does not mention the alias `fm {alias}`"
            );
        }
    }

    for verb in fm::cli::IN_RUN_VERBS {
        assert!(
            md.contains(&format!("`fm {verb}")),
            "docs/CLI.md does not document the in-run command `fm {verb}`"
        );
    }

    let codes = top_help
        .lines()
        .find_map(|l| l.strip_prefix("Exit codes:"))
        .expect("fm --help lists the exit codes");
    for part in codes.split('·') {
        let code = part.split_whitespace().next().unwrap();
        assert!(
            md.contains(&format!("| `{code}` |")),
            "docs/CLI.md has no row for exit code {code}"
        );
    }

    // And nothing documented that is gone.
    for (heading, _) in sections.iter().filter(|(h, _)| !h.starts_with('#')) {
        for name in heading.split("`fm ").skip(1) {
            let name = name.split(['`', ' ']).next().unwrap();
            assert!(
                top.iter().any(|c| c == name),
                "docs/CLI.md documents `fm {name}`, which fm --help does not list"
            );
        }
    }
}

/// How CONFIG.md writes a default: `none`, or the TOML value in code font.
fn shown(v: &Value) -> String {
    match v {
        Value::Null => "none".into(),
        Value::String(s) => format!("`\"{s}\"`"),
        Value::Array(a) if a.is_empty() => "`[]`".into(),
        other => format!("`{other}`"),
    }
}

fn object(v: impl serde::Serialize) -> serde_json::Map<String, Value> {
    match serde_json::to_value(v).unwrap() {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

/// Every key `config.toml` takes, with its default as CONFIG.md shows it.
fn expected_keys() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in object(Config::default()) {
        match (k.as_str(), &v) {
            ("agents", _) => {
                for (k2, v2) in object(CliSettings::default()) {
                    out.insert(format!("agents.<cli>.{k2}"), shown(&v2));
                }
            }
            ("automations", _) => {
                for (k2, v2) in object(AutomationSettings::default()) {
                    out.insert(format!("automations.\"<key>\".{k2}"), shown(&v2));
                }
            }
            (_, Value::Object(m)) => {
                for (k2, v2) in m {
                    out.insert(format!("{k}.{k2}"), shown(v2));
                }
            }
            _ => {
                out.insert(k, shown(&v));
            }
        }
    }
    out
}

/// The `| `key` | default | meaning |` rows of CONFIG.md.
fn documented_keys(md: &str) -> BTreeMap<String, String> {
    md.lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            let cells: Vec<&str> = l.split(" | ").collect();
            let key = cells[0].trim_start_matches("| ").trim_matches('`');
            (key.to_string(), cells[1].trim().to_string())
        })
        .collect()
}

fn toml_blocks(md: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut heading = String::new();
    let mut lines = md.lines();
    while let Some(line) = lines.next() {
        if line.starts_with('#') {
            heading = line.to_string();
        }
        if line == "```toml" {
            let block: Vec<&str> = lines.by_ref().take_while(|l| *l != "```").collect();
            out.push((heading.clone(), block.join("\n")));
        }
    }
    out
}

#[test]
fn config_md_shows_every_key_with_its_real_default() {
    let md = doc("docs/CONFIG.md");
    let want = expected_keys();
    let have = documented_keys(&md);
    let missing: Vec<_> = want.keys().filter(|k| !have.contains_key(*k)).collect();
    assert!(
        missing.is_empty(),
        "docs/CONFIG.md has no row for {missing:?}"
    );
    let extra: Vec<_> = have.keys().filter(|k| !want.contains_key(*k)).collect();
    assert!(
        extra.is_empty(),
        "docs/CONFIG.md documents keys fm does not have: {extra:?}"
    );
    for (key, default) in &want {
        assert_eq!(
            &have[key], default,
            "docs/CONFIG.md: the default of `{key}`"
        );
    }
}

#[test]
fn config_md_examples_are_valid_and_the_defaults_block_is_the_default() {
    let md = doc("docs/CONFIG.md");
    let blocks = toml_blocks(&md);
    assert!(blocks.len() >= 3);
    for (heading, block) in &blocks {
        if let Err(e) = fm_state::parse_config(block) {
            panic!("docs/CONFIG.md, a block under {heading:?}: {e}\n{block}");
        }
    }
    let (_, defaults) = blocks
        .iter()
        .find(|(h, _)| h == "## Every default")
        .expect("docs/CONFIG.md has an `Every default` block");
    assert_eq!(fm_state::parse_config(defaults).unwrap(), Config::default());
    // Every key with a value is written out, not left to the default.
    let written: BTreeSet<&str> = defaults
        .lines()
        .filter_map(|l| l.split_once(" = ").map(|(k, _)| k))
        .collect();
    for (key, default) in expected_keys() {
        let leaf = key.rsplit('.').next().unwrap().to_string();
        let scalar = !key.contains('<') && default != "none";
        assert!(
            !scalar || written.contains(leaf.as_str()),
            "docs/CONFIG.md: `Every default` does not write `{key}`"
        );
    }
}

#[test]
fn cli_md_names_every_fm_environment_variable() {
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "tests" && n != "bin") {
                    walk(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in std::fs::read_dir(&crates).unwrap().flatten() {
        let src = c.path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    let mut vars = BTreeSet::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for rest in text.split("\"FM_").skip(1) {
            let tail: String = rest
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                .collect();
            // The fake agent's knobs are for tests, not for people.
            if rest[tail.len()..].starts_with('"') && !tail.starts_with("FAKE_") {
                vars.insert(format!("FM_{tail}"));
            }
        }
    }
    assert!(
        vars.contains("FM_HOME") && vars.contains("FM_NO_NOTIFY"),
        "{vars:?}"
    );
    let md = doc("docs/CLI.md");
    for v in &vars {
        assert!(
            md.contains(&format!("`{v}`")),
            "docs/CLI.md does not name the environment variable {v}"
        );
    }
}

#[test]
fn cli_md_names_every_in_run_flag() {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fm-run/src/progress_cli.rs"),
    )
    .unwrap();
    let md = doc("docs/CLI.md");
    let inside = md
        .split("## Inside a run")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("docs/CLI.md has an `Inside a run` section");
    let mut seen = 0;
    for line in src.lines() {
        let t = line.trim();
        if !t.starts_with("\"-") || !t.contains("=>") {
            continue;
        }
        for flag in t.split("=>").next().unwrap().split('|') {
            let flag = flag.trim().trim_matches('"');
            assert!(
                inside.contains(&format!("{flag} ")) || inside.contains(&format!("{flag}`")),
                "docs/CLI.md, Inside a run, does not mention the flag {flag}"
            );
            seen += 1;
        }
    }
    assert!(seen >= 8, "found only {seen} in-run flags");
}

#[test]
fn readme_commands_are_real_commands() {
    let fx = Fx::new();
    let top = commands(&help(&fx, &[]));
    let readme = doc("README.md");
    let mut seen = 0;
    let mut in_shell = false;
    for line in readme.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_shell = t == "```sh";
            continue;
        }
        if !in_shell {
            continue;
        }
        if let Some(rest) = t.strip_prefix("fm ") {
            let verb = rest.split_whitespace().next().unwrap();
            assert!(
                verb.starts_with('-') || top.iter().any(|c| c == verb),
                "README.md runs `fm {verb}`, which fm --help does not list"
            );
            seen += 1;
        }
    }
    assert!(seen >= 7, "found only {seen} fm commands in README.md");
}

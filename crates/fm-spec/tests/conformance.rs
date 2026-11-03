//! The specification as a test suite: examples in the core pages (`tests/spec/`) marked
//! `automation` or `frontmatter` is parsed, and refused exactly as its `invalid="…"` says. Every
//! template in `tests/conformance/valid/` parses and passes discovery.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fm_model::{Discovered, Root, Scope};
use fm_spec::{
    DiscoverContext, Machine, Platform, discover, parse_automation, script_path, split_frontmatter,
};

fn conformance_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("conformance")
}

/// The specification's pages, copied from the agent-automations repository (`tests/spec/README.md`).
fn specification_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("spec")
}

#[derive(Debug)]
struct Example {
    doc: String,
    line: usize,
    flags: Vec<String>,
    attrs: HashMap<String, String>,
    code: String,
}

/// `markdown automation dir=x invalid="a \"b\""` -> (lang, flags, attrs), as the site builder reads
/// an info string.
fn parse_info(info: &str) -> (String, Vec<String>, HashMap<String, String>) {
    let mut lang = String::new();
    let mut flags = Vec::new();
    let mut attrs = HashMap::new();
    let chars: Vec<char> = info.trim().chars().collect();
    let is_key = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        if !is_key(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_key(chars[i]) {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect();
        let mut value: Option<String> = None;
        if i + 1 < chars.len() && chars[i] == '=' {
            i += 1;
            match chars[i] {
                '"' => {
                    i += 1;
                    let mut v = String::new();
                    while i < chars.len() && chars[i] != '"' {
                        if chars[i] == '\\' && i + 1 < chars.len() {
                            i += 1;
                        }
                        v.push(chars[i]);
                        i += 1;
                    }
                    i += 1;
                    value = Some(v);
                }
                '\'' => {
                    i += 1;
                    let start = i;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                    value = Some(chars[start..i].iter().collect());
                    i += 1;
                }
                _ => {
                    let start = i;
                    while i < chars.len() && !chars[i].is_whitespace() {
                        i += 1;
                    }
                    value = Some(chars[start..i].iter().collect());
                }
            }
        }
        match value {
            None if first => lang = key.to_lowercase(),
            None => flags.push(key),
            Some(v) => {
                attrs.insert(key, v);
            }
        }
        first = false;
    }
    (lang, flags, attrs)
}

/// Every fenced block marked `automation` or `frontmatter`, in order.
fn examples(doc: &str) -> Vec<Example> {
    let text = std::fs::read_to_string(specification_dir().join(doc))
        .unwrap()
        .replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start_matches(' ');
        let ch = line.chars().next().unwrap_or(' ');
        let run = line.len() - line.trim_start_matches(ch).len();
        if !(ch == '`' || ch == '~') || run < 3 {
            i += 1;
            continue;
        }
        let fence = &line[..run];
        let info = &line[run..];
        let close = (i + 1..lines.len())
            .find(|&j| {
                let l = lines[j].trim();
                l.starts_with(fence) && l.chars().all(|c| c == ch)
            })
            .unwrap_or(lines.len());
        let (_, flags, attrs) = parse_info(info);
        if flags
            .iter()
            .any(|f| f == "automation" || f == "frontmatter")
        {
            out.push(Example {
                doc: doc.to_string(),
                line: i + 1,
                flags,
                attrs,
                code: lines[i + 1..close.min(lines.len())].join("\n"),
            });
        }
        i = close + 1;
    }
    out
}

/// The file and folder name a host would see for an example, as the reference host's site test
/// builds them.
fn as_automation(ex: &Example) -> (String, String) {
    let text = if ex.flags.iter().any(|f| f == "frontmatter") {
        format!("---\n{}\n---\n\nRun.\n", ex.code)
    } else {
        ex.code.clone()
    };
    let named = split_frontmatter(&text).and_then(|f| {
        f.yaml.lines().find_map(|l| {
            let rest = l.strip_prefix("name:")?.trim_start();
            let rest = rest.trim_start_matches(['"', '\'']);
            let name: String = rest
                .chars()
                .take_while(|c| !matches!(c, '"' | '\'' | '#') && !c.is_whitespace())
                .collect();
            (!name.is_empty()).then_some(name)
        })
    });
    let dir = ex
        .attrs
        .get("dir")
        .cloned()
        .or(named)
        .unwrap_or_else(|| "example".to_string());
    (text, dir)
}

/// A message from the reference host's YAML library: the specification lets a host word these its
/// own way, after `frontmatter: `.
fn is_yaml_library_message(expected: &str) -> bool {
    expected.starts_with("frontmatter: ") && expected.contains(" at line ")
}

fn check(ex: &Example) -> Result<(), String> {
    let (text, dir) = as_automation(ex);
    let at = format!("{}:{}", ex.doc, ex.line);
    let result = parse_automation(&text, &dir)
        .map(|_| ())
        .map_err(|r| r.to_string());
    let Some(expected) = ex.attrs.get("invalid") else {
        return result.map_err(|e| format!("{at}: should parse, got: {e}"));
    };
    let Err(actual) = result else {
        return Err(format!("{at}: should be refused with: {expected}"));
    };
    let field = |s: &str| s.split(':').next().unwrap_or("").to_string();
    if field(&actual) != field(expected) {
        return Err(format!(
            "{at}: field path differs\n  expected: {expected}\n  actual:   {actual}"
        ));
    }
    if is_yaml_library_message(expected) {
        if let Some((_, fix)) = expected.rsplit_once(" — ")
            && !actual.ends_with(&format!(" — {fix}"))
        {
            return Err(format!(
                "{at}: fix \"{fix}\" missing\n  expected: {expected}\n  actual:   {actual}"
            ));
        }
        let line = expected
            .split(" at line ")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .unwrap_or("");
        if !actual.contains(&format!("line {line},")) {
            return Err(format!(
                "{at}: points at another line\n  expected: {expected}\n  actual:   {actual}"
            ));
        }
        return Ok(());
    }
    if actual != *expected {
        return Err(format!(
            "{at}: message differs\n  expected: {expected}\n  actual:   {actual}"
        ));
    }
    Ok(())
}

/// Check every example of a page; (examples, refusals among them).
fn run(doc: &str) -> (usize, usize) {
    let all = examples(doc);
    let failures: Vec<String> = all.iter().filter_map(|ex| check(ex).err()).collect();
    assert!(
        failures.is_empty(),
        "{} of {} examples in {doc} fail:\n{}",
        failures.len(),
        all.len(),
        failures.join("\n")
    );
    let invalid = all
        .iter()
        .filter(|e| e.attrs.contains_key("invalid"))
        .count();
    (all.len(), invalid)
}

#[test]
fn validation_md_every_refusal() {
    let (all, invalid) = run("validation.md");
    assert_eq!(invalid, 66, "validation.md lists 66 refusals");
    assert_eq!(all, 66);
}

#[test]
fn specification_md_examples() {
    let (all, invalid) = run("specification.md");
    assert_eq!((all, invalid), (3, 2));
}

#[test]
fn flowcharts_md_examples() {
    let (all, invalid) = run("flowcharts.md");
    assert_eq!((all, invalid), (9, 6));
}

#[test]
fn other_pages_examples_parse() {
    for doc in ["adding-support.md", "watch-scripts.md"] {
        run(doc);
    }
}

#[test]
fn info_strings_unescape_quotes() {
    let (lang, flags, attrs) =
        parse_info(r#"yaml frontmatter dir=x invalid="name: \"A\" is bad" title='a b'"#);
    assert_eq!(lang, "yaml");
    assert_eq!(flags, ["frontmatter"]);
    assert_eq!(attrs["dir"], "x");
    assert_eq!(attrs["invalid"], r#"name: "A" is bad"#);
    assert_eq!(attrs["title"], "a b");
}

#[test]
fn valid_templates_parse_and_pass_discovery() {
    let root = conformance_dir().join("valid");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().join("AUTOMATION.md").is_file())
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names.len(), 4, "{names:?}");

    let mut triggers = (false, false, false);
    for name in &names {
        let dir = root.join(name);
        let text = std::fs::read_to_string(dir.join("AUTOMATION.md")).unwrap();
        let (a, _) = parse_automation(&text, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        triggers.0 |= a.on.schedule.is_some();
        triggers.1 |= a.on.watch.is_some();
        triggers.2 |= a.on.manual && a.on.schedule.is_none() && a.on.watch.is_none();
        for s in fm_spec::script_files(&a) {
            assert!(
                script_path(&dir, &s.script).is_file(),
                "{name}: {} missing",
                s.script
            );
        }
    }
    assert_eq!(triggers, (true, true, true), "a template for each trigger");

    let node = |b: &str| (b == "node").then(|| PathBuf::from("/usr/bin/node"));
    for platform in [Platform::Windows, Platform::MacOs, Platform::Linux] {
        let machine = Machine {
            platform,
            which: &node,
            max_timeout: None,
            agent_check: None,
        };
        let found = discover(
            &[Root {
                path: root.clone(),
                scope: Scope::User,
            }],
            &DiscoverContext {
                machine: Some(&machine),
            },
        );
        assert_eq!(found.len(), 4);
        for d in &found {
            if let Discovered::Invalid { error, .. } = d {
                panic!("{platform:?}: {error}");
            }
        }
    }
}

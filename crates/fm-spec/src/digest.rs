//! The consent pin: one hash over `AUTOMATION.md`, every script it can run and the local modules
//! those scripts import (paths included), and one per file so a change can be named.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fm_model::{Automation, Hook, Located};
use sha2::{Digest as _, Sha256};

use crate::refusal::{Refusal, quote};

/// A script an automation can run, with the field it was declared in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRef {
    /// `on.watch.script`, `hooks.pre[0].script`, …
    pub field: String,
    /// The path as written, relative to the automation directory.
    pub script: String,
}

impl ScriptRef {
    /// The field without its `.script`: `hooks.pre[0]`, `on.watch`.
    pub fn owner(&self) -> &str {
        self.field.strip_suffix(".script").unwrap_or(&self.field)
    }
}

/// Every script, in run order: the watch script first, then the pre hooks, then the post hooks.
pub fn script_files(a: &Automation) -> Vec<ScriptRef> {
    let mut out = Vec::new();
    if let Some(w) = &a.on.watch {
        out.push(ScriptRef {
            field: "on.watch.script".to_string(),
            script: w.script.clone(),
        });
    }
    for (phase, hooks) in [("pre", &a.hooks.pre), ("post", &a.hooks.post)] {
        for (i, hook) in hooks.iter().enumerate() {
            if let Hook::Script(s) = hook {
                out.push(ScriptRef {
                    field: format!("hooks.{phase}[{i}].script"),
                    script: s.clone(),
                });
            }
        }
    }
    out
}

/// `dir` joined with a relative script path written with `/` (or `\`).
pub fn script_path(dir: &Path, script: &str) -> PathBuf {
    let mut p = dir.to_path_buf();
    for part in script.split(['/', '\\']) {
        match part {
            "" | "." => {}
            other => p.push(other),
        }
    }
    p
}

/// Scripts whose local imports run with them, so consent covers those files too.
const MODULE_SCRIPTS: &[&str] = &["mjs", "js", "cjs"];

/// The local files the scripts import, directly or through each other: what `import … from`,
/// `import(…)`, `export … from` and `require(…)` name with a `./` or `../` path in a `.mjs`, `.js`
/// or `.cjs` file. Relative paths written with `/`, sorted, never a script itself. A package
/// (`node:fs`, `yaml`), a file outside the automation directory and a file that does not exist
/// are left out.
fn module_files(l: &Located) -> Vec<String> {
    let scripts: BTreeSet<Vec<String>> = script_files(&l.automation)
        .iter()
        .filter_map(|s| resolve(&[], &s.script))
        .collect();
    let mut found: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut queue: Vec<Vec<String>> = scripts.iter().cloned().collect();
    while let Some(file) = queue.pop() {
        let is_module_script = file
            .last()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(_, ext)| MODULE_SCRIPTS.contains(&ext));
        if !is_module_script {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(script_path(&l.dir, &file.join("/"))) else {
            continue;
        };
        for spec in local_specifiers(&source) {
            let Some(target) = resolve(&file[..file.len() - 1], spec) else {
                continue;
            };
            // `require("./lib")` may leave the extension out.
            let Some(target) = ["", ".js", ".mjs", ".cjs"].iter().find_map(|ext| {
                let mut t = target.clone();
                *t.last_mut()? += ext;
                script_path(&l.dir, &t.join("/")).is_file().then_some(t)
            }) else {
                continue;
            };
            if !scripts.contains(&target) && found.insert(target.clone()) {
                queue.push(target);
            }
        }
    }
    found.into_iter().map(|parts| parts.join("/")).collect()
}

/// `path` (with `/` or `\`) taken from the directory `from`, as parts below the automation
/// directory; `None` when it climbs out of it.
fn resolve(from: &[String], path: &str) -> Option<Vec<String>> {
    let mut parts = from.to_vec();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other.to_string()),
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// The `./` and `../` paths a JavaScript source imports. Comments are skipped, a string ends on
/// its own line, and it counts only right after `from`, `import`, `import(` or `require(`.
fn local_specifiers(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let line_end = |from: usize| source[from..].find('\n').map_or(bytes.len(), |n| from + n);
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = line_end(i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = source[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + n + 4);
            }
            quote @ (b'"' | b'\'') => {
                let Some(len) = source[i + 1..line_end(i + 1)].find(char::from(quote)) else {
                    i += 1;
                    continue;
                };
                let literal = &source[i + 1..i + 1 + len];
                if (literal.starts_with("./") || literal.starts_with("../"))
                    && follows_import(&source[..i])
                {
                    out.push(literal);
                }
                i += len + 2;
            }
            _ => i += 1,
        }
    }
    out
}

fn follows_import(before: &str) -> bool {
    let before = before.trim_end();
    let before = before.strip_suffix('(').map_or(before, str::trim_end);
    ["from", "import", "require"].iter().any(|word| {
        before.strip_suffix(word).is_some_and(|head| {
            !head.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '.')
        })
    })
}

/// The consent pin of one automation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    /// `sha256:<hex>` over `AUTOMATION.md`, every script and every module they import.
    pub digest: String,
    /// `AUTOMATION.md` and each script's and module's relative path -> `sha256:<hex>`.
    pub files: BTreeMap<String, Option<String>>,
}

fn read_or_refuse(file: &Path, field: &str, shown: &str) -> Result<Vec<u8>, Refusal> {
    std::fs::read(file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Refusal::new(field, format!("{shown} not found"))
        } else {
            Refusal::new(field, format!("{shown} cannot be read — {e}"))
        }
    })
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Hash `AUTOMATION.md` and every script it can run: the file, then for each script its relative
/// path, NUL, its bytes, NUL, then the same for each module the scripts import. Without imports it
/// is the same scheme as pi-automations. A missing script is refused, naming the field.
pub fn digest_of(l: &Located) -> Result<Digest, Refusal> {
    let mut h = Sha256::new();
    let mut files = BTreeMap::new();
    let text = read_or_refuse(&l.file, "AUTOMATION.md", "AUTOMATION.md")?;
    h.update(&text);
    files.insert("AUTOMATION.md".to_string(), Some(hash_bytes(&text)));
    let mut add = |path: &str, bytes: &[u8]| {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update(bytes);
        h.update([0u8]);
        files.insert(path.to_string(), Some(hash_bytes(bytes)));
    };
    for s in script_files(&l.automation) {
        let bytes = read_or_refuse(&script_path(&l.dir, &s.script), &s.field, &quote(&s.script))?;
        add(&s.script, &bytes);
    }
    for module in module_files(l) {
        // Found a moment ago; one removed since is simply no longer imported.
        if let Ok(bytes) = std::fs::read(script_path(&l.dir, &module)) {
            add(&module, &bytes);
        }
    }
    Ok(Digest {
        digest: format!("sha256:{}", hex::encode(h.finalize())),
        files,
    })
}

/// Per-file hashes, never failing: a file that cannot be read is `None`.
pub fn file_hashes_of(l: &Located) -> BTreeMap<String, Option<String>> {
    let hash = |p: &Path| std::fs::read(p).ok().map(|b| hash_bytes(&b));
    let mut out = BTreeMap::new();
    out.insert("AUTOMATION.md".to_string(), hash(&l.file));
    for s in script_files(&l.automation) {
        out.insert(s.script.clone(), hash(&script_path(&l.dir, &s.script)));
    }
    for module in module_files(l) {
        out.insert(module.clone(), hash(&script_path(&l.dir, &module)));
    }
    out
}

/// Every file whose hash differs between consent (`old`) and now (`new`), added and removed ones
/// included, sorted.
pub fn changed_files(
    old: &BTreeMap<String, Option<String>>,
    new: &BTreeMap<String, Option<String>>,
) -> Vec<String> {
    let mut out: Vec<String> = old
        .iter()
        .filter(|(k, v)| new.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    out.extend(new.keys().filter(|k| !old.contains_key(*k)).cloned());
    out.sort();
    out.dedup();
    out
}

/// The changed files as one phrase (`scripts/a.sh, scripts/b.sh`), or `None` when nothing changed.
pub fn changed_file(
    old: &BTreeMap<String, Option<String>>,
    new: &BTreeMap<String, Option<String>>,
) -> Option<String> {
    let files = changed_files(old, new);
    (!files.is_empty()).then(|| files.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{DiscoverContext, discover, located};
    use fm_model::{Root, Scope};

    const DAILY: &str = "---\nname: daily-briefing\ndescription: d\non:\n  manual: true\nhooks:\n  pre:\n    - script: scripts/overnight_counts.js\n  post:\n    - script: scripts/mark_run.js\n---\n\nBody.\n";

    fn setup(name: &str, text: &str, scripts: &[(&str, &str)]) -> (tempfile::TempDir, Located) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("automations");
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("AUTOMATION.md"), text).unwrap();
        for (rel, body) in scripts {
            std::fs::write(script_path(&dir, rel), body).unwrap();
        }
        let found = discover(
            &[Root {
                path: root,
                scope: Scope::User,
            }],
            &DiscoverContext::default(),
        );
        let l = located(&found).into_iter().next().expect("valid").clone();
        (tmp, l)
    }

    fn daily() -> (tempfile::TempDir, Located) {
        setup(
            "daily-briefing",
            DAILY,
            &[
                ("scripts/overnight_counts.js", "console.log('12 new');\n"),
                ("scripts/mark_run.js", "process.stdin.resume();\n"),
            ],
        )
    }

    #[test]
    fn is_stable_and_prefixed() {
        let (_t, l) = daily();
        let a = digest_of(&l).unwrap();
        assert!(a.digest.starts_with("sha256:") && a.digest.len() == 7 + 64);
        assert_eq!(digest_of(&l).unwrap(), a);
        assert_eq!(
            a.files.keys().collect::<Vec<_>>(),
            [
                "AUTOMATION.md",
                "scripts/mark_run.js",
                "scripts/overnight_counts.js"
            ]
        );
        assert_eq!(a.files, file_hashes_of(&l));
    }

    #[test]
    fn matches_the_reference_scheme() {
        let (_t, l) = daily();
        let mut h = Sha256::new();
        h.update(DAILY.as_bytes());
        for (rel, body) in [
            ("scripts/overnight_counts.js", "console.log('12 new');\n"),
            ("scripts/mark_run.js", "process.stdin.resume();\n"),
        ] {
            h.update(rel.as_bytes());
            h.update([0]);
            h.update(body.as_bytes());
            h.update([0]);
        }
        assert_eq!(
            digest_of(&l).unwrap().digest,
            format!("sha256:{}", hex::encode(h.finalize()))
        );
    }

    #[test]
    fn changes_with_the_body_or_a_script_but_not_the_mtime() {
        let (_t, l) = daily();
        let before = digest_of(&l).unwrap();
        let script = script_path(&l.dir, "scripts/overnight_counts.js");
        std::fs::write(&script, "console.log('12 new');\n").unwrap();
        assert_eq!(digest_of(&l).unwrap().digest, before.digest);
        std::fs::write(&script, "console.log('different');\n").unwrap();
        let after = digest_of(&l).unwrap();
        assert_ne!(after.digest, before.digest);
        assert_eq!(
            changed_file(&before.files, &after.files).as_deref(),
            Some("scripts/overnight_counts.js")
        );
        std::fs::write(&l.file, format!("{DAILY}\nOne more.\n")).unwrap();
        assert_eq!(
            changed_files(&before.files, &digest_of(&l).unwrap().files),
            ["AUTOMATION.md", "scripts/overnight_counts.js"]
        );
        assert_eq!(changed_file(&before.files, &before.files), None);
    }

    #[test]
    fn covers_scripts_in_run_order() {
        let (_t, l) = daily();
        let refs: Vec<String> = script_files(&l.automation)
            .iter()
            .map(|s| format!("{} {}", s.field, s.script))
            .collect();
        assert_eq!(
            refs,
            [
                "hooks.pre[0].script scripts/overnight_counts.js",
                "hooks.post[0].script scripts/mark_run.js"
            ]
        );
        let mut swapped = l.clone();
        std::mem::swap(
            &mut swapped.automation.hooks.pre,
            &mut swapped.automation.hooks.post,
        );
        assert_ne!(
            digest_of(&swapped).unwrap().digest,
            digest_of(&l).unwrap().digest
        );
    }

    #[test]
    fn a_missing_script_is_refused_by_digest_and_none_per_file() {
        let (_t, l) = setup(
            "gap",
            "---\nname: gap\ndescription: d\non:\n  manual: true\nhooks:\n  pre:\n    - script: scripts/missing.js\n---\n\nBody.\n",
            &[],
        );
        assert_eq!(
            digest_of(&l).unwrap_err().to_string(),
            "hooks.pre[0].script: \"scripts/missing.js\" not found"
        );
        assert_eq!(file_hashes_of(&l)["scripts/missing.js"], None);
    }

    const WATCHED: &str = "---\nname: watched\ndescription: d\non:\n  watch:\n    script: scripts/watch.mjs\n    every: 5m\n  manual: true\nhooks:\n  pre:\n    - script: scripts/pick.mjs\n---\n\nBody.\n";

    fn watched() -> (tempfile::TempDir, Located) {
        setup(
            "watched",
            WATCHED,
            &[
                (
                    "scripts/watch.mjs",
                    "// GitHub's clock and ours can disagree.\nimport { waiting } from \"./queue.mjs\";\nimport fs from \"node:fs\";\n",
                ),
                (
                    "scripts/pick.mjs",
                    "import { waiting } from './queue.mjs';\nimport \"./pick.mjs\";\n",
                ),
                (
                    "scripts/queue.mjs",
                    "const { gh } = require(\"./gh\");\nexport * from \"../shared/paths.mjs\";\nimport(\"../../outside.mjs\");\n",
                ),
                ("scripts/gh.js", "module.exports = {};\n"),
            ],
        )
    }

    #[test]
    fn pins_the_local_modules_the_scripts_import_through_each_other() {
        let (_t, l) = watched();
        std::fs::create_dir_all(l.dir.join("shared")).unwrap();
        std::fs::write(l.dir.join("shared/paths.mjs"), "export const a = 1;\n").unwrap();
        let before = digest_of(&l).unwrap();
        assert_eq!(
            before.files.keys().collect::<Vec<_>>(),
            [
                "AUTOMATION.md",
                "scripts/gh.js",
                "scripts/pick.mjs",
                "scripts/queue.mjs",
                "scripts/watch.mjs",
                "shared/paths.mjs"
            ]
        );
        assert_eq!(before.files, file_hashes_of(&l));

        std::fs::write(l.dir.join("shared/paths.mjs"), "export const a = 2;\n").unwrap();
        let after = digest_of(&l).unwrap();
        assert_ne!(after.digest, before.digest);
        assert_eq!(
            changed_file(&before.files, &after.files).as_deref(),
            Some("shared/paths.mjs")
        );
    }

    #[test]
    fn without_imports_the_digest_is_unchanged() {
        // The daily scripts import nothing, so `matches_the_reference_scheme` still holds; a
        // script that imports only packages pins only itself.
        let (_t, l) = setup(
            "packages",
            "---\nname: packages\ndescription: d\non:\n  manual: true\nhooks:\n  pre:\n    - script: scripts/a.mjs\n---\n\nBody.\n",
            &[(
                "scripts/a.mjs",
                "import fs from \"node:fs\";\nimport yaml from \"yaml\";\n",
            )],
        );
        assert_eq!(
            digest_of(&l).unwrap().files.keys().collect::<Vec<_>>(),
            ["AUTOMATION.md", "scripts/a.mjs"]
        );
    }

    #[test]
    fn finds_every_import_form_and_nothing_else() {
        let source = "import a from \"./a.mjs\";\n\
            import \"./b.mjs\";\n\
            export { c } from './c.mjs';\n\
            const d = await import ( \"./d.mjs\" );\n\
            const e = require('../e.cjs');\n\
            // import x from \"./commented.mjs\"\n\
            /* import y from \"./blocked.mjs\" */\n\
            const path = \"./just-a-string.mjs\";\n\
            const wherefrom = 1; somewherefrom \"./not.mjs\";\n\
            log(\"it's\" + 'fine');\n\
            import f from \"node:fs\";\n";
        assert_eq!(
            local_specifiers(source),
            ["./a.mjs", "./b.mjs", "./c.mjs", "./d.mjs", "../e.cjs"]
        );
    }

    #[test]
    fn names_added_and_removed_files() {
        let old: BTreeMap<String, Option<String>> = [
            ("AUTOMATION.md".to_string(), Some("a".to_string())),
            ("x.sh".to_string(), Some("b".to_string())),
        ]
        .into();
        let new: BTreeMap<String, Option<String>> = [
            ("AUTOMATION.md".to_string(), Some("a".to_string())),
            ("y.sh".to_string(), Some("c".to_string())),
        ]
        .into();
        assert_eq!(changed_files(&old, &new), ["x.sh", "y.sh"]);
    }
}

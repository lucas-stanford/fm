//! Find the automations, and the machine checks: scripts inside their folder and runnable here,
//! timeout within the host maximum, model available (through a hook the agent adapters provide).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use fm_model::{Discovered, Located, Root, Scope};

use crate::digest::{script_files, script_path};
use crate::duration::{format_duration, parse_duration};
use crate::interpreters::{Interpreter, Platform, Which, resolve_interpreter, which};
use crate::parse::parse_automation;
use crate::refusal::{Refusal, quote};

/// Absolute and lexically clean (`.` and `..` folded), without following symbolic links.
pub fn normalize(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn automations_in(dir: &Path) -> PathBuf {
    dir.join(".agents").join("automations")
}

/// `~/.agents/automations`, the user scope. Discovery skips it when it does not exist.
pub fn user_root(home: &Path) -> Root {
    Root {
        path: normalize(&automations_in(home)),
        scope: Scope::User,
    }
}

/// The nearest directory at or above `cwd` with a `.git` entry (a clone or a worktree).
pub fn find_repo_root(cwd: &Path) -> Option<PathBuf> {
    normalize(cwd)
        .ancestors()
        .find(|d| fm_model::has_git_entry(d))
        .map(Path::to_path_buf)
}

/// Every existing `.agents/automations` from `cwd` up to the repository root, nearest first.
/// Empty outside a repository.
pub fn repository_roots(cwd: &Path) -> Vec<Root> {
    let Some(top) = find_repo_root(cwd) else {
        return Vec::new();
    };
    let start = normalize(cwd);
    let mut roots = Vec::new();
    for dir in start.ancestors() {
        let p = automations_in(dir);
        if p.is_dir() {
            roots.push(Root {
                path: p,
                scope: Scope::Repository,
            });
        }
        if dir == top {
            break;
        }
    }
    roots
}

/// The root of a repository remembered for tick time, when there is no working directory.
pub fn roots_for_registered(repo_root: &Path) -> Vec<Root> {
    let p = automations_in(&normalize(repo_root));
    if p.is_dir() {
        vec![Root {
            path: p,
            scope: Scope::Repository,
        }]
    } else {
        Vec::new()
    }
}

/// `<scope>:<root path>:<name>`, the stable identity used in every state file.
pub fn automation_key(scope: Scope, root: &Path, name: &str) -> String {
    let scope = match scope {
        Scope::User => "user",
        Scope::Repository => "repository",
    };
    format!("{scope}:{}:{name}", normalize(root).display())
}

/// Checks the agent adapter adds, such as the model resolving. Gets the automation, returns the
/// one-line refusal (`model: … — …`).
pub type AgentCheck<'a> = &'a dyn Fn(&Located) -> Result<(), String>;

/// What the machine checks need to know about this machine.
pub struct Machine<'a> {
    pub platform: Platform,
    pub which: Which<'a>,
    /// The host's maximum `timeout`, if it has one.
    pub max_timeout: Option<Duration>,
    /// Model resolution and other agent checks; `None` skips them.
    pub agent_check: Option<AgentCheck<'a>>,
}

impl Machine<'static> {
    /// This machine: its OS and `PATH`, no maximum timeout, no agent check.
    pub fn current() -> Machine<'static> {
        Machine {
            platform: Platform::current(),
            which: &which,
            max_timeout: None,
            agent_check: None,
        }
    }
}

/// What passing the machine checks found: how each script starts, by relative path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineReport {
    pub interpreters: BTreeMap<String, Interpreter>,
}

impl MachineReport {
    /// Script -> the exact command shown at consent, as kept in the consent record.
    pub fn commands(&self) -> BTreeMap<String, String> {
        self.interpreters
            .iter()
            .map(|(k, v)| (k.clone(), v.display.clone()))
            .collect()
    }
}

/// The checks that need the machine as well as the file, in the reference host's order: the
/// timeout maximum, every script found and runnable, then `machine.agent_check`. The
/// specification runs these only for an automation that would otherwise be armed.
pub fn check_machine(l: &Located, machine: &Machine<'_>) -> Result<MachineReport, Refusal> {
    if let (Some(max), Some(t)) = (machine.max_timeout, &l.automation.timeout)
        && parse_duration(t).is_some_and(|d| d > max)
    {
        return Err(Refusal::new(
            "timeout",
            format!(
                "{t} exceeds this host's maximum of {}",
                format_duration(max)
            ),
        ));
    }

    let mut report = MachineReport::default();
    for s in script_files(&l.automation) {
        let abs = script_path(&l.dir, &s.script);
        if !abs.is_file() {
            return Err(Refusal::new(
                &s.field,
                format!("{} not found — add it, or fix the path", quote(&s.script)),
            ));
        }
        match resolve_interpreter(&abs, &s.script, machine.platform, machine.which) {
            Ok(i) => {
                report.interpreters.insert(s.script.clone(), i);
            }
            Err(reason) => {
                return Err(Refusal::new(s.owner(), format!("{} {reason}", s.script)));
            }
        }
    }

    if let Some(check) = machine.agent_check {
        check(l).map_err(|line| Refusal::from_line(&line))?;
    }
    Ok(report)
}

/// Script -> the command the consent preview shows, for every script that can start here.
pub fn interpreter_map(
    l: &Located,
    platform: Platform,
    which: Which<'_>,
) -> BTreeMap<String, String> {
    script_files(&l.automation)
        .into_iter()
        .filter_map(|s| {
            let abs = script_path(&l.dir, &s.script);
            resolve_interpreter(&abs, &s.script, platform, which)
                .ok()
                .map(|i| (s.script, i.display))
        })
        .collect()
}

fn is_inside(child: &Path, parent: &Path) -> bool {
    child != parent && child.starts_with(parent)
}

/// A script that resolves outside its folder, through a symbolic link for example. Missing
/// scripts are left to the machine check.
pub fn find_escaping_script(l: &Located) -> Option<Refusal> {
    let real_dir = std::fs::canonicalize(&l.dir).ok()?;
    script_files(&l.automation).into_iter().find_map(|s| {
        let real = std::fs::canonicalize(script_path(&l.dir, &s.script)).ok()?;
        (!is_inside(&real, &real_dir)).then(|| {
            Refusal::new(
                &s.field,
                format!(
                    "{} resolves outside the automation directory",
                    quote(&s.script)
                ),
            )
        })
    })
}

/// Options for [`discover`].
#[derive(Default)]
pub struct DiscoverContext<'a> {
    /// Run [`check_machine`] on every valid automation, turning a failure into `Invalid`. Leave it
    /// `None` to decide states in the specification's order and call `check_machine` yourself.
    pub machine: Option<&'a Machine<'a>>,
}

fn invalid(dir: PathBuf, file: PathBuf, root: &Root, key: String, r: Refusal) -> Discovered {
    Discovered::Invalid {
        dir,
        file,
        root: root.clone(),
        key,
        error: r.to_string(),
        problems: r.problems,
    }
}

/// Scan each root one level deep, nearest first. A folder with an `AUTOMATION.md` is an
/// automation, valid or refused with one line; one without is skipped silently, as is a root that
/// does not exist. The first root to claim a key keeps it. Reads only; never fails.
pub fn discover(roots: &[Root], ctx: &DiscoverContext<'_>) -> Vec<Discovered> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root.path) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();

        let repo_root = match root.scope {
            Scope::Repository => find_repo_root(&root.path),
            Scope::User => None,
        };

        for name in names {
            let dir = normalize(&root.path.join(&name));
            let file = dir.join("AUTOMATION.md");
            let key = automation_key(root.scope, &root.path, &name);
            if seen.contains(&key) {
                continue;
            }
            let text = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                // A folder without an AUTOMATION.md is not an automation.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => continue,
                Err(e) => {
                    seen.insert(key.clone());
                    let r = Refusal::new("AUTOMATION.md", format!("cannot read — {e}"));
                    out.push(invalid(dir, file, root, key, r));
                    continue;
                }
            };
            seen.insert(key.clone());
            let Ok(text) = String::from_utf8(text) else {
                let r = Refusal::new("AUTOMATION.md", "is not UTF-8 text — save it as UTF-8");
                out.push(invalid(dir, file, root, key, r));
                continue;
            };
            let automation = match parse_automation(&text, &name) {
                Ok((a, _chart)) => a,
                Err(r) => {
                    out.push(invalid(dir, file, root, key, r));
                    continue;
                }
            };
            let located = Located {
                automation,
                dir: dir.clone(),
                file: file.clone(),
                root: root.clone(),
                repo_root: repo_root.clone(),
                key: key.clone(),
            };
            if let Some(r) = find_escaping_script(&located) {
                out.push(invalid(dir, file, root, key, r));
                continue;
            }
            if let Some(machine) = ctx.machine
                && let Err(r) = check_machine(&located, machine)
            {
                out.push(invalid(dir, file, root, key, r));
                continue;
            }
            out.push(Discovered::Valid(Box::new(located)));
        }
    }
    out
}

/// Only the automations that parsed.
pub fn located(found: &[Discovered]) -> Vec<&Located> {
    found
        .iter()
        .filter_map(|d| match d {
            Discovered::Valid(l) => Some(l.as_ref()),
            Discovered::Invalid { .. } => None,
        })
        .collect()
}

/// One discovered automation by key.
pub fn by_key<'a>(found: &'a [Discovered], key: &str) -> Option<&'a Discovered> {
    found.iter().find(|d| d.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = script_path(dir, rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn manual(name: &str, extra: &str) -> String {
        format!("---\nname: {name}\ndescription: d\non:\n  manual: true\n{extra}---\n\nBody.\n")
    }

    fn user(path: &Path) -> Root {
        Root {
            path: path.to_path_buf(),
            scope: Scope::User,
        }
    }

    fn errors(found: &[Discovered]) -> Vec<String> {
        found
            .iter()
            .filter_map(|d| match d {
                Discovered::Invalid { error, .. } => Some(error.clone()),
                Discovered::Valid(_) => None,
            })
            .collect()
    }

    #[test]
    fn finds_automations_keyed_by_scope_root_and_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("automations");
        write(&root, "b-one/AUTOMATION.md", &manual("b-one", ""));
        write(
            &root,
            "a-two/AUTOMATION.md",
            &manual("a-two", "enabled: false\n"),
        );
        write(&root, "not-an-automation/notes.md", "hello");
        write(&root, "loose-file.md", "hello");
        let found = discover(&[user(&root)], &DiscoverContext::default());
        let names: Vec<&str> = located(&found)
            .iter()
            .map(|l| l.automation.name.as_str())
            .collect();
        assert_eq!(names, ["a-two", "b-one"]);
        let one = located(&found)[1];
        assert_eq!(one.key, automation_key(Scope::User, &root, "b-one"));
        assert!(one.key.starts_with("user:") && one.key.ends_with(":b-one"));
        assert!(one.file.ends_with(Path::new("b-one").join("AUTOMATION.md")));
        assert!(!located(&found)[0].automation.enabled);
        assert_eq!(one.repo_root, None);
        assert!(by_key(&found, &one.key).is_some());
        assert!(by_key(&found, "nope").is_none());
    }

    #[test]
    fn reports_a_refused_file_as_one_line_and_never_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("automations");
        write(&root, "broken/AUTOMATION.md", "not frontmatter at all");
        write(
            &root,
            "bad-cron/AUTOMATION.md",
            "---\nname: bad-cron\ndescription: d\non:\n  schedule:\n    cron: \"0 7 * * MON-FRI *\"\n---\n\nBody.\n",
        );
        let found = discover(&[user(&root)], &DiscoverContext::default());
        assert_eq!(
            errors(&found),
            [
                "on.schedule.cron: \"0 7 * * MON-FRI *\" is not a 5-field cron expression",
                "frontmatter: missing — AUTOMATION.md starts with a --- line, then YAML, then another --- line",
            ]
        );
        assert_eq!(found[1].key(), automation_key(Scope::User, &root, "broken"));
    }

    #[test]
    fn skips_missing_roots_and_duplicate_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone");
        assert!(discover(&[user(&gone)], &DiscoverContext::default()).is_empty());
        let root = tmp.path().join("automations");
        write(&root, "x/AUTOMATION.md", &manual("x", ""));
        let twice = discover(&[user(&root), user(&root)], &DiscoverContext::default());
        assert_eq!(twice.len(), 1);
    }

    #[test]
    fn strict_flow_problems_are_kept_one_by_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("automations");
        write(
            &root,
            "review/AUTOMATION.md",
            "---\nname: review\ndescription: d\nflow: strict\non:\n  manual: true\n---\n\n# Review\n\n```mermaid\nflowchart TD\n  read[\"Read the change\"] --> post[\"Post the comment\"]\n```\n\n## Read it {#read}\n\n## Notes\n",
        );
        let found = discover(&[user(&root)], &DiscoverContext::default());
        let Discovered::Invalid {
            error, problems, ..
        } = &found[0]
        else {
            panic!("should be refused");
        };
        assert_eq!(problems.len(), 3);
        assert_eq!(*error, format!("flow: {}", problems.join("; ")));
    }

    #[test]
    fn repository_roots_walk_up_to_the_git_entry_nearest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let sub = repo.join("packages").join("app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(automations_in(&repo)).unwrap();
        std::fs::create_dir_all(automations_in(&sub)).unwrap();
        std::fs::create_dir_all(automations_in(tmp.path())).unwrap();
        let roots = repository_roots(&sub);
        assert_eq!(
            roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            [
                normalize(&automations_in(&sub)),
                normalize(&automations_in(&repo))
            ]
        );
        assert!(roots.iter().all(|r| r.scope == Scope::Repository));
        assert_eq!(find_repo_root(&sub), Some(normalize(&repo)));
        assert_eq!(roots_for_registered(&repo).len(), 1);
        assert!(roots_for_registered(&sub.join("nothing")).is_empty());

        let outside = tempfile::tempdir().unwrap();
        if find_repo_root(outside.path()).is_none() {
            assert!(repository_roots(outside.path()).is_empty());
        }

        write(&automations_in(&repo), "x/AUTOMATION.md", &manual("x", ""));
        let found = discover(&roots, &DiscoverContext::default());
        let l = located(&found)[0];
        assert_eq!(l.repo_root, Some(normalize(&repo)));
        assert!(l.key.starts_with("repository:"));
    }

    #[test]
    fn user_root_and_keys_are_normalized() {
        let home = Path::new("home-dir");
        let r = user_root(home);
        assert!(r.path.is_absolute());
        assert!(r.path.ends_with(Path::new(".agents").join("automations")));
        let a = automation_key(Scope::User, &home.join("x").join(".."), "n");
        let b = automation_key(Scope::User, home, "n");
        assert_eq!(a, b);
    }

    fn symlinks_work(dir: &Path) -> bool {
        let target = dir.join("probe-target");
        std::fs::write(&target, "x").unwrap();
        symlink(&target, &dir.join("probe-link")).is_ok()
    }

    #[cfg(unix)]
    fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[test]
    fn refuses_a_script_that_links_out_of_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        if !symlinks_work(tmp.path()) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let root = tmp.path().join("automations");
        let outside = tmp.path().join("outside");
        write(&outside, "other.js", "console.log('ok');\n");
        let hook = "hooks:\n  pre:\n    - script: scripts/helper.js\n";
        write(&root, "linked/AUTOMATION.md", &manual("linked", hook));
        std::fs::create_dir_all(root.join("linked").join("scripts")).unwrap();
        symlink(
            &outside.join("other.js"),
            &script_path(&root.join("linked"), "scripts/helper.js"),
        )
        .unwrap();
        write(&root, "fine/AUTOMATION.md", &manual("fine", hook));
        write(&root, "fine/real.js", "console.log('ok');\n");
        std::fs::create_dir_all(root.join("fine").join("scripts")).unwrap();
        symlink(
            &root.join("fine").join("real.js"),
            &script_path(&root.join("fine"), "scripts/helper.js"),
        )
        .unwrap();

        let found = discover(&[user(&root)], &DiscoverContext::default());
        assert_eq!(located(&found).len(), 1);
        assert_eq!(located(&found)[0].automation.name, "fine");
        assert_eq!(
            errors(&found),
            [
                "hooks.pre[0].script: \"scripts/helper.js\" resolves outside the automation directory"
            ]
        );
    }

    #[test]
    fn machine_checks_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("automations");
        write(
            &root,
            "count/AUTOMATION.md",
            &manual(
                "count",
                "timeout: 6h\nhooks:\n  pre:\n    - script: scripts/count.ps1\n  post:\n    - script: scripts/save.js\n",
            ),
        );
        write(&root, "count/scripts/count.ps1", "Write-Output 1\n");
        let found = discover(&[user(&root)], &DiscoverContext::default());
        let l = located(&found)[0].clone();

        let none = |_: &str| None;
        let tools = |b: &str| match b {
            "pwsh" => Some(PathBuf::from("/usr/bin/pwsh")),
            "node" => Some(PathBuf::from("/usr/bin/node")),
            _ => None,
        };
        let mut m = Machine {
            platform: Platform::Linux,
            which: &none,
            max_timeout: Some(Duration::from_secs(4 * 3600)),
            agent_check: None,
        };
        let check = |m: &Machine<'_>| check_machine(&l, m).map_err(|r| r.to_string());
        assert_eq!(
            check(&m).unwrap_err(),
            "timeout: 6h exceeds this host's maximum of 4h"
        );
        m.max_timeout = None;
        assert_eq!(
            check(&m).unwrap_err(),
            "hooks.pre[0]: scripts/count.ps1 cannot run here — needs pwsh (PowerShell 7) on this OS"
        );
        m.which = &tools;
        assert_eq!(
            check(&m).unwrap_err(),
            "hooks.post[0].script: \"scripts/save.js\" not found — add it, or fix the path"
        );
        write(&root, "count/scripts/save.js", "process.stdin.resume();\n");
        let report = check(&m).unwrap();
        assert_eq!(
            report.commands().keys().collect::<Vec<_>>(),
            ["scripts/count.ps1", "scripts/save.js"]
        );
        assert_eq!(
            report.interpreters["scripts/count.ps1"].args,
            ["-NoProfile", "-File"]
        );
        assert_eq!(
            interpreter_map(&l, Platform::Linux, &tools),
            report.commands()
        );

        let refuse = |_: &Located| {
            Err("model: no credentials for anthropic — log in to anthropic".to_string())
        };
        m.agent_check = Some(&refuse);
        let r = check_machine(&l, &m).unwrap_err();
        assert_eq!(r.field, "model");
        assert_eq!(
            r.to_string(),
            "model: no credentials for anthropic — log in to anthropic"
        );

        let ctx = DiscoverContext { machine: Some(&m) };
        assert_eq!(
            errors(&discover(&[user(&root)], &ctx)),
            ["model: no credentials for anthropic — log in to anthropic"]
        );
    }
}

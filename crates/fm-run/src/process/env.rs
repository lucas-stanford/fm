//! The environment a child gets: credentials out, `AUTOMATION_*` in.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use fm_model::{Located, Outcome, Phase, Scope, Trigger};

use super::EnvMode;

const CREDENTIAL_SUFFIXES: [&str; 6] = [
    "_API_KEY",
    "_TOKEN",
    "_SECRET",
    "_PASSWORD",
    "_CREDENTIAL",
    "_CREDENTIALS",
];

/// A name ending in `_API_KEY`, `_TOKEN`, `_SECRET`, `_PASSWORD` or `_CREDENTIAL(S)`, in any case.
/// The spec says such a variable never reaches a hook or watch script.
pub fn is_credential_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    CREDENTIAL_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

/// A name a script never inherits: a credential, or an `AUTOMATION_*` name left over from whatever
/// started this process (a hook running `fm`, say), which would otherwise leak into the wrong run.
pub fn is_scrubbed_name(name: &str) -> bool {
    is_credential_name(name)
        || name
            .get(..11)
            .is_some_and(|p| p.eq_ignore_ascii_case("AUTOMATION_"))
}

/// What a script is running for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptContext {
    /// A pre or post hook of a run.
    Hook {
        run_id: String,
        trigger: Trigger,
        late: bool,
        /// ISO 8601, when the run was scheduled.
        scheduled_for: Option<String>,
        phase: Phase,
        index: usize,
        /// Post hooks only: the outcome so far.
        outcome: Option<Outcome>,
        /// Post hooks only: why, when the outcome is not `ok`.
        reason: Option<String>,
    },
    /// An `on.watch` check.
    Watch {
        /// ISO 8601 of the last check that ran to an exit code.
        last_checked: Option<String>,
    },
}

/// Everything the `AUTOMATION_*` variables are made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationVars {
    pub name: String,
    pub key: String,
    pub scope: Scope,
    pub dir: PathBuf,
    pub root: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub context: ScriptContext,
    /// Added last, e.g. the run's progress identity (`AUTOMATION_RUN`, `AUTOMATION_NODE`, `AUTOMATION_STATE_DIR`).
    pub extra: Vec<(String, String)>,
}

impl AutomationVars {
    pub fn new(l: &Located, context: ScriptContext) -> Self {
        Self {
            name: l.automation.name.clone(),
            key: l.key.clone(),
            scope: l.root.scope,
            dir: l.dir.clone(),
            root: l.root.path.clone(),
            repo_root: l.repo_root.clone(),
            context,
            extra: Vec::new(),
        }
    }
}

fn scope_str(s: Scope) -> &'static str {
    match s {
        Scope::User => "user",
        Scope::Repository => "repository",
    }
}

fn trigger_str(t: Trigger) -> &'static str {
    match t {
        Trigger::Schedule => "schedule",
        Trigger::Manual => "manual",
        Trigger::Watch => "watch",
    }
}

fn phase_str(p: Phase) -> &'static str {
    match p {
        Phase::Pre => "pre",
        Phase::Post => "post",
    }
}

fn outcome_str(o: Outcome) -> &'static str {
    match o {
        Outcome::Ok => "ok",
        Outcome::Skipped => "skipped",
        Outcome::Failed => "failed",
        Outcome::Missed => "missed",
        Outcome::Aborted => "aborted",
    }
}

/// The `AUTOMATION_*` variables (then `extra`), set exactly when pi-automations sets them.
pub fn automation_env(v: &AutomationVars) -> Vec<(String, String)> {
    let path = |p: &PathBuf| p.to_string_lossy().into_owned();
    let mut env: Vec<(String, String)> = vec![
        ("AUTOMATION_NAME".into(), v.name.clone()),
        ("AUTOMATION_KEY".into(), v.key.clone()),
        ("AUTOMATION_SCOPE".into(), scope_str(v.scope).into()),
        ("AUTOMATION_DIR".into(), path(&v.dir)),
        ("AUTOMATION_ROOT".into(), path(&v.root)),
    ];
    match &v.context {
        ScriptContext::Hook {
            run_id,
            trigger,
            late,
            scheduled_for,
            phase,
            index,
            outcome,
            reason,
        } => {
            env.push(("AUTOMATION_RUN_ID".into(), run_id.clone()));
            env.push(("AUTOMATION_TRIGGER".into(), trigger_str(*trigger).into()));
            env.push(("AUTOMATION_LATE".into(), late.to_string()));
            env.push(("AUTOMATION_PHASE".into(), phase_str(*phase).into()));
            env.push(("AUTOMATION_HOOK_INDEX".into(), index.to_string()));
            if let Some(r) = &v.repo_root {
                env.push(("AUTOMATION_REPO_ROOT".into(), path(r)));
            }
            if let Some(s) = scheduled_for.as_ref().filter(|s| !s.is_empty()) {
                env.push(("AUTOMATION_SCHEDULED_FOR".into(), s.clone()));
            }
            if *phase == Phase::Post {
                if let Some(o) = outcome {
                    env.push(("AUTOMATION_OUTCOME".into(), outcome_str(*o).into()));
                }
                if let Some(r) = reason.as_ref().filter(|r| !r.is_empty()) {
                    env.push(("AUTOMATION_REASON".into(), r.clone()));
                }
            }
        }
        ScriptContext::Watch { last_checked } => {
            env.push(("AUTOMATION_TRIGGER".into(), "watch".into()));
            env.push(("AUTOMATION_PHASE".into(), "watch".into()));
            if let Some(r) = &v.repo_root {
                env.push(("AUTOMATION_REPO_ROOT".into(), path(r)));
            }
            if let Some(t) = last_checked.as_ref().filter(|t| !t.is_empty()) {
                env.push(("AUTOMATION_LAST_CHECKED".into(), t.clone()));
            }
        }
    }
    env.extend(v.extra.iter().cloned());
    env
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// The complete environment of a hook or watch script: `base` (usually this process's) without
/// credential-shaped or inherited `AUTOMATION_*` names, then [`automation_env`]. Use it with
/// [`EnvMode::Exact`]. To scrub this process's own environment, prefer
/// `EnvMode::Scrubbed { overlay: automation_env(vars) }`, which also passes non-UTF-8 variables.
pub fn hook_env(
    base: impl IntoIterator<Item = (String, String)>,
    vars: &AutomationVars,
) -> Vec<(String, String)> {
    let overlay = automation_env(vars);
    let mut env: Vec<(String, String)> = base
        .into_iter()
        .filter(|(k, _)| !is_scrubbed_name(k))
        .filter(|(k, _)| !overlay.iter().any(|(o, _)| same_name(o, k)))
        .collect();
    env.extend(overlay);
    env
}

/// Set `cmd`'s environment for `mode`, from this process's environment.
pub(super) fn apply(cmd: &mut Command, mode: &EnvMode) {
    apply_from(cmd, mode, std::env::vars_os());
}

/// Set `cmd`'s environment for `mode`, from `parent`. Non-UTF-8 names and values pass untouched.
pub(super) fn apply_from(
    cmd: &mut Command,
    mode: &EnvMode,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) {
    match mode {
        EnvMode::Scrubbed { overlay } => {
            for (k, _) in parent {
                if is_scrubbed_name(&k.to_string_lossy()) {
                    cmd.env_remove(k);
                }
            }
            cmd.envs(overlay.iter().map(|(k, v)| (k, v)));
        }
        EnvMode::Inherit { overlay } => {
            cmd.envs(overlay.iter().map(|(k, v)| (k, v)));
        }
        EnvMode::Exact(vars) => {
            cmd.env_clear();
            cmd.envs(vars.iter().map(|(k, v)| (k, v)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    fn hook_vars(phase: Phase) -> AutomationVars {
        AutomationVars {
            name: "pr-review".into(),
            key: "user:/home/u/.agents/automations:pr-review".into(),
            scope: Scope::User,
            dir: PathBuf::from("dir"),
            root: PathBuf::from("root"),
            repo_root: None,
            context: ScriptContext::Hook {
                run_id: "run-1".into(),
                trigger: Trigger::Manual,
                late: false,
                scheduled_for: None,
                phase,
                index: 2,
                outcome: Some(Outcome::Failed),
                reason: Some("pre[0] s.js exited 3".into()),
            },
            extra: vec![],
        }
    }

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
        env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn credential_names_in_any_case() {
        for n in [
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "my_secret",
            "DB_PASSWORD",
            "AZURE_CREDENTIALS",
            "GCP_CREDENTIAL",
            "Npm_Token",
        ] {
            assert!(is_credential_name(n), "{n}");
        }
        for n in [
            "PATH",
            "HOME",
            "TOKEN",
            "SECRET",
            "TOKEN_FILE",
            "API_KEYS",
            "PASSWORD_HINT",
        ] {
            assert!(!is_credential_name(n), "{n}");
        }
    }

    #[test]
    fn hook_env_strips_credentials_and_keeps_the_rest() {
        let base = vec![
            s("GITHUB_TOKEN", "t"),
            s("OPENAI_API_KEY", "k"),
            s("my_secret", "s"),
            s("DB_PASSWORD", "p"),
            s("AZURE_CREDENTIALS", "c"),
            s("PATH", "/bin"),
            s("HOME", "/home/u"),
        ];
        let env = hook_env(base, &hook_vars(Phase::Pre));
        for gone in [
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "my_secret",
            "DB_PASSWORD",
            "AZURE_CREDENTIALS",
        ] {
            assert_eq!(get(&env, gone), None, "{gone}");
        }
        assert_eq!(get(&env, "PATH"), Some("/bin"));
        assert_eq!(get(&env, "HOME"), Some("/home/u"));
        assert_eq!(get(&env, "AUTOMATION_NAME"), Some("pr-review"));
    }

    #[test]
    fn inherited_automation_names_never_leak() {
        let base = vec![s("AUTOMATION_OUTCOME", "ok"), s("automation_name", "other")];
        let env = hook_env(base, &hook_vars(Phase::Pre));
        assert_eq!(get(&env, "AUTOMATION_OUTCOME"), None);
        assert_eq!(get(&env, "automation_name"), None);
        assert_eq!(
            env.iter().filter(|(k, _)| k == "AUTOMATION_NAME").count(),
            1
        );
    }

    #[test]
    fn pre_hook_variables_as_pi_automations_sets_them() {
        let env = automation_env(&hook_vars(Phase::Pre));
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "AUTOMATION_NAME",
                "AUTOMATION_KEY",
                "AUTOMATION_SCOPE",
                "AUTOMATION_DIR",
                "AUTOMATION_ROOT",
                "AUTOMATION_RUN_ID",
                "AUTOMATION_TRIGGER",
                "AUTOMATION_LATE",
                "AUTOMATION_PHASE",
                "AUTOMATION_HOOK_INDEX",
            ]
        );
        assert_eq!(get(&env, "AUTOMATION_SCOPE"), Some("user"));
        assert_eq!(get(&env, "AUTOMATION_TRIGGER"), Some("manual"));
        assert_eq!(get(&env, "AUTOMATION_LATE"), Some("false"));
        assert_eq!(get(&env, "AUTOMATION_PHASE"), Some("pre"));
        assert_eq!(get(&env, "AUTOMATION_HOOK_INDEX"), Some("2"));
        // Outcome and reason are for post hooks only.
        assert_eq!(get(&env, "AUTOMATION_OUTCOME"), None);
        assert_eq!(get(&env, "AUTOMATION_REASON"), None);
    }

    #[test]
    fn post_hooks_get_the_outcome_and_optional_fields_when_present() {
        let mut v = hook_vars(Phase::Post);
        v.scope = Scope::Repository;
        v.repo_root = Some(PathBuf::from("repo"));
        if let ScriptContext::Hook {
            scheduled_for,
            late,
            trigger,
            ..
        } = &mut v.context
        {
            *scheduled_for = Some("2026-10-29T09:00:00-07:00".into());
            *late = true;
            *trigger = Trigger::Schedule;
        }
        v.extra = vec![s("AUTOMATION_RUN", "run-1")];
        let env = automation_env(&v);
        assert_eq!(get(&env, "AUTOMATION_SCOPE"), Some("repository"));
        assert_eq!(get(&env, "AUTOMATION_REPO_ROOT"), Some("repo"));
        assert_eq!(
            get(&env, "AUTOMATION_SCHEDULED_FOR"),
            Some("2026-10-29T09:00:00-07:00")
        );
        assert_eq!(get(&env, "AUTOMATION_LATE"), Some("true"));
        assert_eq!(get(&env, "AUTOMATION_TRIGGER"), Some("schedule"));
        assert_eq!(get(&env, "AUTOMATION_OUTCOME"), Some("failed"));
        assert_eq!(get(&env, "AUTOMATION_REASON"), Some("pre[0] s.js exited 3"));
        assert_eq!(env.last(), Some(&s("AUTOMATION_RUN", "run-1")));
    }

    #[test]
    fn watch_variables() {
        let mut v = hook_vars(Phase::Pre);
        v.context = ScriptContext::Watch {
            last_checked: Some("2026-10-29T09:00:00-07:00".into()),
        };
        let env = automation_env(&v);
        assert_eq!(get(&env, "AUTOMATION_TRIGGER"), Some("watch"));
        assert_eq!(get(&env, "AUTOMATION_PHASE"), Some("watch"));
        assert_eq!(
            get(&env, "AUTOMATION_LAST_CHECKED"),
            Some("2026-10-29T09:00:00-07:00")
        );
        assert_eq!(get(&env, "AUTOMATION_RUN_ID"), None);
        assert_eq!(get(&env, "AUTOMATION_HOOK_INDEX"), None);

        v.context = ScriptContext::Watch { last_checked: None };
        assert_eq!(get(&automation_env(&v), "AUTOMATION_LAST_CHECKED"), None);
    }

    fn parent() -> Vec<(OsString, OsString)> {
        [
            ("GITHUB_TOKEN", "t"),
            ("OPENAI_API_KEY", "k"),
            ("my_secret", "s"),
            ("DB_PASSWORD", "p"),
            ("AZURE_CREDENTIALS", "c"),
            ("AUTOMATION_PHASE", "post"),
            ("PATH", "/bin"),
            ("HOME", "/home/u"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }

    /// What `cmd` was told: `Some` set, `None` removed.
    fn told(cmd: &Command) -> Vec<(String, Option<String>)> {
        cmd.get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn removed(cmd: &Command, name: &str) -> bool {
        told(cmd)
            .iter()
            .any(|(k, v)| same_name(k, name) && v.is_none())
    }

    #[test]
    fn scrubbed_removes_credentials_from_the_parent_and_adds_the_overlay() {
        let mut cmd = Command::new("x");
        let mode = EnvMode::Scrubbed {
            overlay: vec![s("AUTOMATION_NAME", "n")],
        };
        apply_from(&mut cmd, &mode, parent());
        for gone in [
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "my_secret",
            "DB_PASSWORD",
            "AZURE_CREDENTIALS",
            "AUTOMATION_PHASE",
        ] {
            assert!(removed(&cmd, gone), "{gone}");
        }
        for kept in ["PATH", "HOME"] {
            assert!(
                !told(&cmd).iter().any(|(k, _)| same_name(k, kept)),
                "{kept}"
            );
        }
        assert!(
            told(&cmd)
                .iter()
                .any(|(k, v)| k == "AUTOMATION_NAME" && v.as_deref() == Some("n"))
        );
    }

    #[test]
    fn inherit_keeps_the_login_environment_for_agent_turns() {
        let mut cmd = Command::new("x");
        let mode = EnvMode::Inherit {
            overlay: vec![s("AUTOMATION_RUN", "r")],
        };
        apply_from(&mut cmd, &mode, parent());
        assert_eq!(
            told(&cmd),
            vec![("AUTOMATION_RUN".into(), Some("r".into()))]
        );
    }
}

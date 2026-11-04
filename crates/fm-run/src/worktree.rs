//! `[automations."<key>"] worktree = true`: the body runs in a throwaway git worktree of the
//! repository, made for the run and removed after it. A worktree that cannot be made refuses the
//! run with a record saying why; it never falls back to the checkout.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::live::one_line;
use crate::process::{self, EnvMode, ProcessOutcome, ProcessSpec};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// A worktree this run made.
#[derive(Debug)]
pub struct Worktree {
    git: PathBuf,
    repo: PathBuf,
    pub path: PathBuf,
}

/// Why a worktree could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeFailure {
    pub repository: PathBuf,
    pub intended: Option<PathBuf>,
    pub detail: String,
}

fn git(git: &Path, repo: &Path, args: &[&str]) -> Result<(), String> {
    let mut spec = ProcessSpec::new(git, repo).args(args.iter().copied());
    spec.env_mode = EnvMode::Inherit {
        overlay: Vec::new(),
    };
    spec.timeout = GIT_TIMEOUT;
    match process::run(&spec, |_| {}) {
        ProcessOutcome::Exited { exit: 0, .. } => Ok(()),
        ProcessOutcome::Exited { exit, output } => {
            let lines: Vec<&str> = output
                .stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            let diagnosis = lines
                .iter()
                .find(|l| {
                    let lower = l.to_ascii_lowercase();
                    lower.starts_with("fatal:") || lower.starts_with("error:")
                })
                .or(lines.first())
                .map_or_else(|| format!("git exited {exit}"), |l| l.to_string());
            Err(one_line(&diagnosis, 160))
        }
        ProcessOutcome::StartFailed { message } => Err(one_line(&message, 160)),
        ProcessOutcome::TimedOut { after, .. } => Err(format!(
            "git did not finish within {}",
            process::format_duration(after)
        )),
        ProcessOutcome::Stopped { .. } => Err("git was stopped".into()),
        ProcessOutcome::Signaled { signal, .. } => Err(format!("git was killed by {signal}")),
    }
}

/// `git worktree add --detach <temp>/fm-wt-<run id> HEAD` in `repo`.
pub fn prepare(repo: &Path, run_id: &str) -> Result<Worktree, WorktreeFailure> {
    let fail = |intended: Option<PathBuf>, detail: String| WorktreeFailure {
        repository: repo.to_path_buf(),
        intended,
        detail,
    };
    let Some(git_bin) = fm_spec::which("git") else {
        return Err(fail(None, "git is not installed (not on PATH)".into()));
    };
    let path = std::env::temp_dir().join(format!("fm-wt-{run_id}"));
    let shown = path.to_string_lossy().into_owned();
    match git(
        &git_bin,
        repo,
        &["worktree", "add", "--detach", &shown, "HEAD"],
    ) {
        Ok(()) => Ok(Worktree {
            git: git_bin,
            repo: repo.to_path_buf(),
            path,
        }),
        Err(detail) => {
            let _ = std::fs::remove_dir_all(&path);
            Err(fail(Some(path), detail))
        }
    }
}

impl Worktree {
    /// Remove it. A leftover worktree is untidy, not dangerous: this never fails.
    pub fn release(self) {
        let shown = self.path.to_string_lossy().into_owned();
        if git(
            &self.git,
            &self.repo,
            &["worktree", "remove", "--force", &shown],
        )
        .is_err()
        {
            let _ = std::fs::remove_dir_all(&self.path);
            let _ = git(&self.git, &self.repo, &["worktree", "prune"]);
        }
    }
}

/// A short cause, so a clipped summary still tells failures apart.
fn cause(detail: &str) -> String {
    let d = detail.to_lowercase();
    if d.contains("git is not installed") {
        return "git is not installed".into();
    }
    if d.contains("already exists") {
        return "the worktree path already exists".into();
    }
    if d.contains("not a git repository") {
        return "not a git repository".into();
    }
    if d.contains("not a valid object name") || d.contains("invalid reference") {
        return "the repository has no usable HEAD".into();
    }
    if d.contains("permission denied") {
        return "permission denied".into();
    }
    if d.contains("is already checked out") {
        return "that commit is already checked out".into();
    }
    one_line(detail, 44)
}

/// The one-line refusal, as pi-automations writes it: cause, fix, then where and git's line.
pub fn refusal_message(key: &str, f: &WorktreeFailure) -> String {
    let detail = one_line(&f.detail, 160);
    let where_ = f
        .intended
        .as_ref()
        .map(|p| format!(" at {}", p.display()))
        .unwrap_or_default();
    format!(
        "workspace: {} — set worktree = false for this automation in config.toml to run in the checkout — could not prepare the git worktree for {}{where_}: {detail} ({})",
        cause(&detail),
        f.repository.display(),
        serde_json::to_string(key).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_cause_first() {
        let f = WorktreeFailure {
            repository: PathBuf::from("repo"),
            intended: Some(PathBuf::from("wt")),
            detail: "fatal: not a git repository (or any of the parent directories): .git".into(),
        };
        let m = refusal_message("repository:/r:x", &f);
        assert!(m.starts_with("workspace: not a git repository — "), "{m}");
        assert!(
            m.contains(" for repo at wt: fatal: not a git repository"),
            "{m}"
        );
        assert!(m.ends_with("(\"repository:/r:x\")"), "{m}");
        assert!(!m.contains('\n'));
    }

    #[test]
    fn makes_and_removes_a_worktree_or_refuses_outside_a_repository() {
        let Some(git_bin) = fm_spec::which("git") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let refused = prepare(&repo, "20260917-140000-nore").unwrap_err();
        assert_eq!(cause(&refused.detail), "not a git repository");
        assert!(!refused.intended.as_ref().unwrap().exists());

        let run = |args: &[&str]| git(&git_bin, &repo, args).unwrap();
        run(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        run(&["add", "a.txt"]);
        run(&[
            "-c",
            "user.name=fm",
            "-c",
            "user.email=fm@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "a",
        ]);
        let wt = prepare(&repo, "20260917-140000-wt01").unwrap();
        assert!(wt.path.join("a.txt").is_file());
        let path = wt.path.clone();
        wt.release();
        assert!(!path.exists());
    }
}

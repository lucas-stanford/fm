//! The layout of fm's state folder. Resolving the folder itself is `fm-state`'s job.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatePaths {
    pub root: PathBuf,
}

impl StatePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        StatePaths { root: root.into() }
    }
    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn trust(&self) -> PathBuf {
        self.root.join("trust.json")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state.json")
    }
    pub fn tick_lock(&self) -> PathBuf {
        self.root.join("tick.lock")
    }
    /// `runs/<slug>.jsonl`: one record per line.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }
    /// `locks/<slug>.lock`, `.live`, `.stop`: same lifetime as a run.
    pub fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }
    /// `progress/<run id>.json`: what a tracked run reported.
    pub fn progress_dir(&self) -> PathBuf {
        self.root.join("progress")
    }
    /// `sessions/<run id>.jsonl`: each run's raw agent output.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn records_file(&self, key: &str) -> PathBuf {
        self.runs_dir().join(format!("{}.jsonl", slug(key)))
    }
    pub fn lock_file(&self, key: &str) -> PathBuf {
        self.locks_dir().join(format!("{}.lock", slug(key)))
    }
    pub fn live_file(&self, key: &str) -> PathBuf {
        self.locks_dir().join(format!("{}.live", slug(key)))
    }
    pub fn stop_file(&self, key: &str) -> PathBuf {
        self.locks_dir().join(format!("{}.stop", slug(key)))
    }
    pub fn progress_file(&self, run_id: &str) -> PathBuf {
        self.progress_dir().join(format!("{run_id}.json"))
    }
    pub fn session_file(&self, run_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{run_id}.jsonl"))
    }
}

/// A file-name-safe, stable, collision-resistant name for a key: readable part plus a hash.
pub fn slug(key: &str) -> String {
    let name = key.rsplit(':').next().unwrap_or(key);
    let readable: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    format!("{readable}-{:08x}", fnv1a(key))
}

/// 32-bit FNV-1a, as pi-automations uses for stagger and slugs.
pub fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// True when `p` has a `.git` entry (a clone or a worktree).
pub fn has_git_entry(p: &Path) -> bool {
    p.join(".git").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_stable_and_safe() {
        let k = "user:/home/me/.agents/automations:daily-notes";
        assert_eq!(slug(k), slug(k));
        assert!(slug(k).starts_with("daily-notes-"));
        assert_ne!(
            slug(k),
            slug("repository:/x/.agents/automations:daily-notes")
        );
    }

    #[test]
    fn fnv1a_matches_reference() {
        assert_eq!(fnv1a(""), 0x811c_9dc5);
        assert_eq!(fnv1a("a"), 0xe40c_292c);
    }
}

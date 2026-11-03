//! The environment the watcher job carries: an allow-list from the shell that ran
//! `fm watcher start` (so a scheduled tick finds the same CLIs), never a credential.

use std::path::Path;

use fm_run::process::is_credential_name;

/// Names a job carries when they are set: safe, and useful to a scheduled process.
pub const CARRIED: [&str; 8] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "TZ",
    "SystemRoot",
    "ComSpec",
];

/// What the job file will set, and what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvSnapshot {
    /// `FM_HOME` (the state folder) first, then the carried names that were set.
    pub vars: Vec<(String, String)>,
    /// Credential-shaped names that were present and deliberately not carried, sorted.
    pub refused: Vec<String>,
}

impl EnvSnapshot {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The carried subset of `source`, plus `FM_HOME` pinned to `state_dir` so the job ticks the same
/// state folder. Names match case-insensitively (`Path` on Windows is `PATH`).
pub fn snapshot_env(source: &[(String, String)], state_dir: &Path) -> EnvSnapshot {
    let mut vars = vec![(
        "FM_HOME".to_string(),
        state_dir.to_string_lossy().into_owned(),
    )];
    for name in CARRIED {
        let value = source
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone());
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            vars.push((name.to_string(), v));
        }
    }
    let mut refused: Vec<String> = source
        .iter()
        .filter(|(k, _)| is_credential_name(k))
        .map(|(k, _)| k.clone())
        .collect();
    refused.sort();
    refused.dedup();
    EnvSnapshot { vars, refused }
}

/// Refuse a job file that contains the value of a credential-shaped variable.
pub fn assert_no_secrets(text: &str, source: &[(String, String)]) -> Result<(), String> {
    for (name, value) in source {
        if is_credential_name(name) && value.len() >= 8 && text.contains(value.as_str()) {
            return Err(format!(
                "watcher: the job file would contain the value of {name} — unset it and try again"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn carries_only_the_names_a_job_needs() {
        let s = snapshot_env(
            &src(&[
                ("PATH", "/usr/bin"),
                ("HOME", "/home/me"),
                ("EDITOR", "vim"),
                ("TERM", "xterm"),
            ]),
            Path::new("/state"),
        );
        assert_eq!(
            s.vars,
            src(&[
                ("FM_HOME", "/state"),
                ("PATH", "/usr/bin"),
                ("HOME", "/home/me")
            ])
        );
    }

    #[test]
    fn never_carries_a_credential_and_says_which_it_refused() {
        let s = snapshot_env(
            &src(&[
                ("PATH", "/usr/bin"),
                ("GITHUB_TOKEN", "ghp_x"),
                ("OPENAI_API_KEY", "sk"),
            ]),
            Path::new("/state"),
        );
        assert!(s.get("GITHUB_TOKEN").is_none());
        assert_eq!(s.refused, ["GITHUB_TOKEN", "OPENAI_API_KEY"]);
    }

    #[test]
    fn matches_names_case_insensitively_and_skips_empty_values() {
        let s = snapshot_env(
            &src(&[("Path", "C:\\bin"), ("LANG", "")]),
            Path::new("C:\\state"),
        );
        assert_eq!(s.get("PATH"), Some("C:\\bin"));
        assert_eq!(s.vars[1].0, "PATH");
        assert!(s.get("LANG").is_none());
    }

    #[test]
    fn refuses_a_job_file_holding_a_secret_value() {
        let source = src(&[("MY_TOKEN", "supersecretvalue")]);
        assert!(assert_no_secrets("PATH=/usr/bin", &source).is_ok());
        let err = assert_no_secrets("x supersecretvalue y", &source).unwrap_err();
        assert!(err.contains("MY_TOKEN"), "{err}");
        // Short values are too common to mean anything.
        assert!(assert_no_secrets("abc", &src(&[("A_TOKEN", "abc")])).is_ok());
    }
}

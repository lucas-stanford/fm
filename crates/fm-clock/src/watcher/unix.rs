//! The shell wrapper the macOS and Linux jobs run: the carried environment, then
//! `exec <fm> tick --by job`; when the executable is gone, it removes the job instead.

use super::JobSpec;

/// One shell word: single-quoted, with `'` written as `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The wrapper. `unregister` are shell lines that remove the job; they run when the executable is
/// no longer there, after `watcher.json` is removed.
pub fn tick_sh(spec: &JobSpec, unregister: &[String]) -> String {
    let mut out = vec![
        "#!/bin/sh".to_string(),
        "# Written by fm watcher start: one tick of the watcher, which fires schedules and runs"
            .to_string(),
        "# watch checks. When the executable is gone it removes the job instead.".to_string(),
    ];
    for (k, v) in &spec.env {
        if k == "PATH" {
            // The shell's PATH first, then whatever the scheduler gives.
            out.push(format!("PATH={}:\"$PATH\"", sh_quote(v)));
        } else {
            out.push(format!("{k}={}", sh_quote(v)));
        }
        out.push(format!("export {k}"));
    }
    let exe = sh_quote(&spec.exe.to_string_lossy());
    out.push(format!("if [ ! -x {exe} ]; then"));
    out.push(format!(
        "  rm -f {}",
        sh_quote(&spec.record.to_string_lossy())
    ));
    for line in unregister {
        out.push(format!("  {line}"));
    }
    out.push("  exit 0".to_string());
    out.push("fi".to_string());
    out.push(format!("exec {exe} tick --by job"));
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::Names;
    use std::path::PathBuf;

    #[test]
    fn quotes_any_path() {
        assert_eq!(sh_quote("/a b/c"), "'/a b/c'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("$HOME`x`"), "'$HOME`x`'");
    }

    #[test]
    fn exports_the_environment_and_execs_the_absolute_binary() {
        let spec = JobSpec {
            exe: PathBuf::from("/opt/my tools/fm"),
            state_dir: PathBuf::from("/home/me/.local/state/agent-automations"),
            env: vec![
                (
                    "FM_HOME".into(),
                    "/home/me/.local/state/agent-automations".into(),
                ),
                ("PATH".into(), "/home/me/.cargo/bin:/usr/bin".into()),
            ],
            names: Names::default(),
            script: PathBuf::from("/s/fm-tick.sh"),
            log: PathBuf::from("/s/logs/tick.log"),
            record: PathBuf::from("/s/watcher.json"),
        };
        let text = tick_sh(
            &spec,
            &["systemctl --user disable --now fm-tick.timer".into()],
        );
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(
            text.contains("FM_HOME='/home/me/.local/state/agent-automations'\nexport FM_HOME\n")
        );
        assert!(text.contains("PATH='/home/me/.cargo/bin:/usr/bin':\"$PATH\"\nexport PATH\n"));
        assert!(text.contains("if [ ! -x '/opt/my tools/fm' ]; then\n  rm -f '/s/watcher.json'\n  systemctl --user disable --now fm-tick.timer\n  exit 0\nfi\n"));
        assert!(text.ends_with("exec '/opt/my tools/fm' tick --by job\n"));
    }
}

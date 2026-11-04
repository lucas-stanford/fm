//! macOS: a LaunchAgent in `~/Library/LaunchAgents` that runs one tick every minute.
//!
//! An empty `StartCalendarInterval` fires on every minute, on the minute (as pi-automations does,
//! and as the Windows and Linux jobs do), rather than `StartInterval 60`, which counts from when
//! the agent was loaded; launchd coalesces the minutes missed while the Mac slept into one. The
//! job runs `/bin/sh <state>/watcher/fm-tick.sh` (see [`super::unix`]) in the Background band,
//! with the carried environment also in `EnvironmentVariables`, and output appended to
//! `logs/tick.log`. `AbandonProcessGroup` keeps launchd from killing the runs the tick started
//! when the tick exits (they lead process groups of their own anyway).

use std::path::Path;

use super::windows::xml_escape;
use super::{Cmd, JobSpec};

/// The property list.
pub fn plist(spec: &JobSpec) -> String {
    let env: String = spec
        .env
        .iter()
        .map(|(k, v)| {
            format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml_escape(k),
                xml_escape(v)
            )
        })
        .collect();
    let log = xml_escape(&spec.log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>{script}</string>
  </array>
  <key>StartCalendarInterval</key>
  <dict/>
  <key>RunAtLoad</key>
  <false/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>AbandonProcessGroup</key>
  <true/>
  <key>EnvironmentVariables</key>
  <dict>
{env}  </dict>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        label = xml_escape(&spec.names.label),
        script = xml_escape(&spec.script.to_string_lossy()),
    )
}

/// `gui/<uid>`, the person's login session domain.
pub fn domain(uid: u32) -> String {
    format!("gui/{uid}")
}

fn target(uid: u32, label: &str) -> String {
    format!("{}/{label}", domain(uid))
}

fn launchctl(args: Vec<String>) -> Cmd {
    Cmd::new("launchctl", args)
}

/// Shell lines the wrapper runs to remove the job once the executable is gone. `bootout` ends the wrapper
/// too, so it comes last.
pub fn unregister_lines(label: &str, plist_file: &Path) -> Vec<String> {
    vec![
        format!(
            "rm -f {}",
            super::unix::sh_quote(&plist_file.to_string_lossy())
        ),
        format!(
            "launchctl bootout \"gui/$(id -u)\"/{}",
            super::unix::sh_quote(label)
        ),
    ]
}

/// Load the agent. A loaded copy is booted out first: `bootstrap` refuses a label already loaded.
pub fn register(uid: u32, label: &str, plist_file: &Path) -> (Cmd, Cmd) {
    (
        launchctl(vec!["bootout".into(), target(uid, label)]),
        launchctl(vec![
            "bootstrap".into(),
            domain(uid),
            plist_file.to_string_lossy().into_owned(),
        ]),
    )
}

pub fn run_now(uid: u32, label: &str) -> Cmd {
    launchctl(vec!["kickstart".into(), target(uid, label)])
}

pub fn remove(uid: u32, label: &str) -> Vec<Cmd> {
    vec![launchctl(vec!["bootout".into(), target(uid, label)])]
}

pub fn query(uid: u32, label: &str) -> Cmd {
    launchctl(vec!["print".into(), target(uid, label)])
}

/// `id -u`, for the session domain.
pub fn uid_command() -> Cmd {
    Cmd::new("id", vec!["-u".into()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::{JobSpec, Names};
    use std::path::PathBuf;

    fn spec() -> JobSpec {
        JobSpec {
            exe: PathBuf::from("/Users/me/.cargo/bin/fm"),
            state_dir: PathBuf::from("/Users/me/Library/Application Support/agent-automations"),
            env: vec![
                (
                    "FM_HOME".into(),
                    "/Users/me/Library/Application Support/agent-automations".into(),
                ),
                ("PATH".into(), "/opt/homebrew/bin:/usr/bin".into()),
                ("LANG".into(), "en_US.UTF-8 & <more>".into()),
            ],
            names: Names::default(),
            script: PathBuf::from(
                "/Users/me/Library/Application Support/agent-automations/watcher/fm-tick.sh",
            ),
            log: PathBuf::from(
                "/Users/me/Library/Application Support/agent-automations/logs/tick.log",
            ),
            record: PathBuf::from(
                "/Users/me/Library/Application Support/agent-automations/watcher.json",
            ),
        }
    }

    #[test]
    fn fires_every_minute_in_the_background_band() {
        let p = plist(&spec());
        assert!(p.contains("<key>Label</key>\n  <string>io.agentautomations.fm.tick</string>"));
        assert!(p.contains("<key>StartCalendarInterval</key>\n  <dict/>"));
        assert!(p.contains("<key>RunAtLoad</key>\n  <false/>"));
        assert!(p.contains("<key>ProcessType</key>\n  <string>Background</string>"));
        assert!(p.contains("<key>AbandonProcessGroup</key>\n  <true/>"));
        assert!(!p.contains("StartInterval</key>\n  <integer>"));
    }

    #[test]
    fn runs_the_wrapper_with_the_path_snapshot() {
        let p = plist(&spec());
        assert!(p.contains(
            "<string>/bin/sh</string>\n    <string>/Users/me/Library/Application Support/agent-automations/watcher/fm-tick.sh</string>"
        ));
        assert!(p.contains("<key>PATH</key>\n    <string>/opt/homebrew/bin:/usr/bin</string>"));
        assert!(p.contains("<key>FM_HOME</key>"));
        assert!(p.contains(
            "<key>StandardOutPath</key>\n  <string>/Users/me/Library/Application Support/agent-automations/logs/tick.log</string>"
        ));
    }

    #[test]
    fn escapes_xml() {
        let p = plist(&spec());
        assert!(p.contains("<string>en_US.UTF-8 &amp; &lt;more&gt;</string>"));
    }

    #[test]
    fn launchctl_commands_use_the_login_session() {
        let f = Path::new("/Users/me/Library/LaunchAgents/io.agentautomations.fm.tick.plist");
        let (out, boot) = register(501, "io.agentautomations.fm.tick", f);
        assert_eq!(out.args, ["bootout", "gui/501/io.agentautomations.fm.tick"]);
        assert_eq!(boot.args, ["bootstrap", "gui/501", &*f.to_string_lossy()]);
        assert_eq!(run_now(501, "x").args, ["kickstart", "gui/501/x"]);
        assert_eq!(remove(501, "x")[0].args, ["bootout", "gui/501/x"]);
        assert_eq!(query(501, "x").args, ["print", "gui/501/x"]);
        let lines = unregister_lines("x", f);
        assert!(lines[1].starts_with("launchctl bootout \"gui/$(id -u)\"/'x'"));
    }
}

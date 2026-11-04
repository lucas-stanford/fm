//! Windows: a per-user Task Scheduler task that runs one tick every minute.
//!
//! The task runs `conhost.exe --headless cmd.exe /d /c <state>\watcher\fm-tick.cmd`. `fm` is a
//! console program, so a task that started it directly would flash a console window every
//! minute. pi-automations hides it with a `wscript.exe` shim running a VBScript; VBScript is being
//! removed from Windows, so fm uses the console host's own headless mode instead (Windows 10 1809
//! and later): no window is ever created, and every process the tick starts inherits the hidden
//! console. Without `conhost.exe` the task runs `cmd.exe` directly (a window may flash).
//!
//! The `.cmd` wrapper sets the carried environment (`FM_HOME`, `PATH`, …), runs
//! `"<fm.exe>" tick --by job` with its output appended to `logs\tick.log`, and, when `fm.exe` is
//! gone, deletes the task and `watcher.json` instead: a removed binary leaves no job failing every
//! minute.
//!
//! The task uses the interactive token (the person's own credentials, so CLIs find their logins),
//! runs only while they are logged on, never starts a second instance while one runs, and starts
//! a missed run as soon as it can.

use std::path::Path;

use super::{Cmd, JobSpec};

/// Characters cmd.exe reads as syntax in an unquoted (or rule-1 quoted) command line.
const CMD_SPECIAL: [char; 10] = ['&', '<', '>', '(', ')', '@', '^', '|', '%', '"'];

/// What the task's `Exec` action runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub command: String,
    pub arguments: String,
}

fn quote_arg(s: &str) -> String {
    if s.contains(char::is_whitespace) {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

/// The action for a wrapper at `cmd_file`. Refused, in one line, for a path cmd.exe cannot start.
pub fn launcher(system_root: &Path, cmd_file: &Path, headless: bool) -> Result<Launcher, String> {
    let file = cmd_file.to_string_lossy();
    if file.contains(CMD_SPECIAL) {
        return Err(format!(
            "watcher: cmd.exe cannot start {file} (it has one of & < > ( ) @ ^ | %) — set FM_HOME to a folder without them"
        ));
    }
    // Joined as text, not with `Path::join`: the task is a Windows file even when it is generated
    // (in tests) on another OS.
    let root = system_root.to_string_lossy();
    let system32 = format!("{}\\System32", root.trim_end_matches(['\\', '/']));
    let cmd = format!("{system32}\\cmd.exe");
    let tail = format!("/d /c {}", quote_arg(&file));
    Ok(if headless {
        Launcher {
            command: format!("{system32}\\conhost.exe"),
            arguments: format!("--headless {} {tail}", quote_arg(&cmd)),
        }
    } else {
        Launcher {
            command: cmd,
            arguments: tail,
        }
    })
}

/// XML text content.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The Task Scheduler definition. Written as UTF-16 with a byte order mark ([`utf16_with_bom`]),
/// which is what `schtasks /XML` reads.
pub fn task_xml(spec: &JobSpec, launcher: &Launcher) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Agent Automations (fm): the watcher, which fires schedules and runs watch checks once a minute.</Description>
    <URI>\{uri}</URI>
  </RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <StartBoundary>2020-01-01T00:00:00</StartBoundary>
      <Repetition>
        <Interval>PT1M</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT10M</ExecutionTimeLimit>
    <WakeToRun>false</WakeToRun>
    <AllowHardTerminate>true</AllowHardTerminate>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        uri = xml_escape(&spec.names.task),
        command = xml_escape(&launcher.command),
        arguments = xml_escape(&launcher.arguments),
    )
}

/// `text` as UTF-16LE with a byte order mark.
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// A value inside a batch file: `%` doubled so it is not expanded.
fn batch(s: &str) -> String {
    s.replace('%', "%%")
}

/// The longest line cmd.exe reads.
const BATCH_LINE_MAX: usize = 8191;

/// The wrapper the task runs. CRLF line endings.
pub fn tick_cmd(spec: &JobSpec) -> String {
    let exe = batch(&spec.exe.to_string_lossy());
    let log = batch(&spec.log.to_string_lossy());
    let record = batch(&spec.record.to_string_lossy());
    let mut lines = vec![
        "@echo off".to_string(),
        "rem Written by fm watcher start: one tick of the watcher, which fires schedules and runs"
            .to_string(),
        "rem watch checks. When fm.exe is gone it removes the task instead.".to_string(),
    ];
    for (k, v) in &spec.env {
        if v.contains(['"', '\r', '\n']) {
            continue;
        }
        let line = format!("set \"{k}={}\"", batch(v));
        if line.len() < BATCH_LINE_MAX {
            lines.push(line);
        }
    }
    lines.extend([
        format!("if not exist \"{exe}\" goto gone"),
        format!("\"{exe}\" tick --by job >> \"{log}\" 2>&1"),
        "exit /b %ERRORLEVEL%".to_string(),
        ":gone".to_string(),
        format!("del /q \"{record}\" >nul 2>&1"),
        format!(
            "schtasks /Delete /TN \"{}\" /F >nul 2>&1",
            batch(&spec.names.task)
        ),
        "exit /b 0".to_string(),
    ]);
    let mut text = lines.join("\r\n");
    text.push_str("\r\n");
    text
}

fn schtasks(args: &[&str]) -> Cmd {
    Cmd::new("schtasks", args.iter().map(|s| s.to_string()).collect())
}

pub fn register(task: &str, xml: &Path) -> Vec<Cmd> {
    vec![schtasks(&[
        "/Create",
        "/TN",
        task,
        "/XML",
        &xml.to_string_lossy(),
        "/F",
    ])]
}

pub fn run_now(task: &str) -> Cmd {
    schtasks(&["/Run", "/TN", task])
}

pub fn remove(task: &str) -> Vec<Cmd> {
    vec![schtasks(&["/Delete", "/TN", task, "/F"])]
}

pub fn query(task: &str) -> Cmd {
    schtasks(&["/Query", "/TN", task])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::{JobSpec, Names};
    use std::path::PathBuf;

    fn spec() -> JobSpec {
        JobSpec {
            exe: PathBuf::from(r"C:\Program Files\fm\fm.exe"),
            state_dir: PathBuf::from(r"C:\Users\me\AppData\Local\agent-automations"),
            env: vec![
                (
                    "FM_HOME".into(),
                    r"C:\Users\me\AppData\Local\agent-automations".into(),
                ),
                ("PATH".into(), r"C:\bin;C:\100%\tools".into()),
            ],
            names: Names::default(),
            script: PathBuf::from(
                r"C:\Users\me\AppData\Local\agent-automations\watcher\fm-tick.cmd",
            ),
            log: PathBuf::from(r"C:\Users\me\AppData\Local\agent-automations\logs\tick.log"),
            record: PathBuf::from(r"C:\Users\me\AppData\Local\agent-automations\watcher.json"),
        }
    }

    #[test]
    fn the_task_sets_every_option_a_minute_by_minute_job_needs() {
        let s = spec();
        let l = launcher(Path::new(r"C:\Windows"), &s.script, true).unwrap();
        let xml = task_xml(&s, &l);
        for needle in [
            "<Interval>PT1M</Interval>",
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            "<StartWhenAvailable>true</StartWhenAvailable>",
            "<Hidden>true</Hidden>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            r"<URI>\agent-automations\tick</URI>",
        ] {
            assert!(xml.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn launches_through_headless_conhost_so_no_window_flashes() {
        let s = spec();
        let l = launcher(Path::new(r"C:\Windows"), &s.script, true).unwrap();
        assert_eq!(l.command, r"C:\Windows\System32\conhost.exe");
        assert_eq!(
            l.arguments,
            r"--headless C:\Windows\System32\cmd.exe /d /c C:\Users\me\AppData\Local\agent-automations\watcher\fm-tick.cmd"
        );
        let xml = task_xml(&s, &l);
        assert!(xml.contains(r"<Command>C:\Windows\System32\conhost.exe</Command>"));

        let plain = launcher(Path::new(r"C:\Windows"), &s.script, false).unwrap();
        assert_eq!(plain.command, r"C:\Windows\System32\cmd.exe");
        assert!(plain.arguments.starts_with("/d /c "));
    }

    #[test]
    fn quotes_a_wrapper_path_with_spaces_and_refuses_cmd_syntax() {
        let l = launcher(
            Path::new(r"C:\Windows"),
            Path::new(r"C:\Users\Jo Smith\fm\watcher\fm-tick.cmd"),
            true,
        )
        .unwrap();
        assert!(
            l.arguments
                .ends_with(r#"/c "C:\Users\Jo Smith\fm\watcher\fm-tick.cmd""#)
        );
        let xml = task_xml(&spec(), &l);
        assert!(xml.contains("&quot;C:\\Users\\Jo Smith"));
        let err = launcher(
            Path::new(r"C:\Windows"),
            Path::new(r"C:\Users\A&B (x)\fm-tick.cmd"),
            true,
        )
        .unwrap_err();
        assert!(err.starts_with("watcher: cmd.exe cannot start"), "{err}");
        assert!(err.contains("— set FM_HOME"), "{err}");
    }

    #[test]
    fn the_wrapper_runs_the_absolute_exe_with_the_carried_environment() {
        let text = tick_cmd(&spec());
        assert!(text.contains("\r\n"));
        assert!(text.contains(r#"set "FM_HOME=C:\Users\me\AppData\Local\agent-automations""#));
        // A literal % survives cmd.exe's expansion.
        assert!(text.contains(r#"set "PATH=C:\bin;C:\100%%\tools""#));
        assert!(text.contains(r#"if not exist "C:\Program Files\fm\fm.exe" goto gone"#));
        assert!(text.contains(
            r#""C:\Program Files\fm\fm.exe" tick --by job >> "C:\Users\me\AppData\Local\agent-automations\logs\tick.log" 2>&1"#
        ));
    }

    #[test]
    fn the_wrapper_removes_the_task_when_fm_is_gone() {
        let text = tick_cmd(&spec());
        let gone = text.split(":gone").nth(1).unwrap();
        assert!(gone.contains(r#"schtasks /Delete /TN "agent-automations\tick" /F"#));
        assert!(
            gone.contains(r#"del /q "C:\Users\me\AppData\Local\agent-automations\watcher.json""#)
        );
    }

    #[test]
    fn the_definition_is_utf16_with_a_bom() {
        let bytes = utf16_with_bom("<a/>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(&bytes[2..4], &[b'<', 0]);
        assert_eq!(bytes.len(), 2 + 4 * 2);
    }

    #[test]
    fn scheduler_commands_name_only_this_task() {
        let task = "fm-test-1234";
        let reg = register(task, Path::new(r"C:\s\tick.xml"));
        assert_eq!(reg[0].program, "schtasks");
        assert_eq!(
            reg[0].args,
            ["/Create", "/TN", task, "/XML", r"C:\s\tick.xml", "/F"]
        );
        assert_eq!(remove(task)[0].args, ["/Delete", "/TN", task, "/F"]);
        assert_eq!(query(task).args, ["/Query", "/TN", task]);
        assert_eq!(run_now(task).args, ["/Run", "/TN", task]);
    }
}

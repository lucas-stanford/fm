//! Starting a run. The tick never runs one itself: it hands the automation key to a [`StartRun`],
//! which the `fm` binary wires to [`DetachedSpawn`] (a detached `fm fire …` process).
//!
//! The fire contract, which the `fm` binary implements (see `docs/ARCHITECTURE.md`):
//!
//! ```text
//! <exe> fire --key <automation key> --trigger <schedule|watch|manual> [--scheduled-for <iso>] [--late]
//! ```

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fm_model::{Trigger, slug};

/// Starts one run of an automation, somewhere else. `Ok` means it started; `Err` says why it did
/// not, and the tick writes one `failed` record for it.
pub trait StartRun {
    fn start(
        &self,
        key: &str,
        trigger: Trigger,
        scheduled_for: Option<String>,
        late: bool,
    ) -> Result<(), String>;
}

pub fn trigger_word(t: Trigger) -> &'static str {
    match t {
        Trigger::Schedule => "schedule",
        Trigger::Manual => "manual",
        Trigger::Watch => "watch",
    }
}

/// The arguments of `fm fire` for one run, after the executable.
pub fn fire_args(
    key: &str,
    trigger: Trigger,
    scheduled_for: Option<&str>,
    late: bool,
) -> Vec<String> {
    let mut args = vec![
        "fire".to_string(),
        "--key".to_string(),
        key.to_string(),
        "--trigger".to_string(),
        trigger_word(trigger).to_string(),
    ];
    if let Some(s) = scheduled_for {
        args.push("--scheduled-for".into());
        args.push(s.to_string());
    }
    if late {
        args.push("--late".into());
    }
    args
}

/// Starts `<exe> fire …` as a detached process that outlives the tick: no console and its own
/// process group on Windows (breaking away from the tick's job object when allowed), a process
/// group of its own and no terminal on Unix. It is never waited for by the tick, except by a
/// thread that reaps it, so a long `fm serve` leaves no zombies.
#[derive(Debug, Clone)]
pub struct DetachedSpawn {
    pub exe: PathBuf,
    /// Standard output and error go to `<log_dir>/<slug>.log` (appended); `None` discards them.
    pub log_dir: Option<PathBuf>,
    /// Added to the child's environment (this process's environment is inherited).
    pub env: Vec<(String, String)>,
}

impl DetachedSpawn {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        DetachedSpawn {
            exe: exe.into(),
            log_dir: None,
            env: Vec::new(),
        }
    }

    pub fn with_log_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.log_dir = Some(dir.into());
        self
    }

    fn output(&self, key: &str) -> (Stdio, Stdio) {
        let Some(dir) = &self.log_dir else {
            return (Stdio::null(), Stdio::null());
        };
        let _ = std::fs::create_dir_all(dir);
        let file = dir.join(format!("{}.log", slug(key)));
        match OpenOptions::new().create(true).append(true).open(&file) {
            Ok(f) => match f.try_clone() {
                Ok(g) => (Stdio::from(f), Stdio::from(g)),
                Err(_) => (Stdio::from(f), Stdio::null()),
            },
            Err(_) => (Stdio::null(), Stdio::null()),
        }
    }

    fn command(&self, args: &[String], key: &str) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args.iter().map(OsString::from))
            .stdin(Stdio::null())
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        let (out, err) = self.output(key);
        cmd.stdout(out).stderr(err);
        if let Some(dir) = self.exe.parent().filter(|d| d.is_dir()) {
            cmd.current_dir(dir);
        }
        cmd
    }
}

#[cfg(windows)]
mod os {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    pub fn detach(cmd: &mut Command, breakaway: bool) {
        let mut flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        if breakaway {
            flags |= CREATE_BREAKAWAY_FROM_JOB;
        }
        cmd.creation_flags(flags);
    }

    pub const TRY_BREAKAWAY: bool = true;
}

#[cfg(unix)]
mod os {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub fn detach(cmd: &mut Command, _breakaway: bool) {
        cmd.process_group(0);
    }

    pub const TRY_BREAKAWAY: bool = false;
}

#[cfg(not(any(windows, unix)))]
mod os {
    pub fn detach(_cmd: &mut std::process::Command, _breakaway: bool) {}
    pub const TRY_BREAKAWAY: bool = false;
}

/// Spawn `exe args` detached; the pid on success.
pub fn spawn_detached(spawn: &DetachedSpawn, args: &[String], key: &str) -> Result<u32, String> {
    if !Path::new(&spawn.exe).is_file() {
        return Err(format!("{} not found", spawn.exe.display()));
    }
    let attempt = |breakaway: bool| {
        let mut cmd = spawn.command(args, key);
        os::detach(&mut cmd, breakaway);
        cmd.spawn()
    };
    // A job that forbids breaking away refuses the flag; then start inside it.
    let child = match attempt(os::TRY_BREAKAWAY) {
        Ok(c) => Ok(c),
        Err(_) if os::TRY_BREAKAWAY => attempt(false),
        Err(e) => Err(e),
    }
    .map_err(|e| format!("{}: {e}", spawn.exe.display()))?;
    let pid = child.id();
    let mut child = child;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

impl StartRun for DetachedSpawn {
    fn start(
        &self,
        key: &str,
        trigger: Trigger,
        scheduled_for: Option<String>,
        late: bool,
    ) -> Result<(), String> {
        let args = fire_args(key, trigger, scheduled_for.as_deref(), late);
        spawn_detached(self, &args, key).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_arguments_follow_the_contract() {
        assert_eq!(
            fire_args(
                "user:/r:a",
                Trigger::Schedule,
                Some("2026-09-17T07:00:00.000Z"),
                true
            ),
            [
                "fire",
                "--key",
                "user:/r:a",
                "--trigger",
                "schedule",
                "--scheduled-for",
                "2026-09-17T07:00:00.000Z",
                "--late"
            ]
        );
        assert_eq!(
            fire_args("k", Trigger::Watch, None, false),
            ["fire", "--key", "k", "--trigger", "watch"]
        );
        assert_eq!(trigger_word(Trigger::Manual), "manual");
    }

    #[test]
    fn a_missing_executable_is_a_failed_start() {
        let s = DetachedSpawn::new("definitely-not-here/fm-missing");
        let err = s.start("k", Trigger::Watch, None, false).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }
}

//! Finding an agent CLI and its version.
//!
//! The only place this crate starts a process: `<cli> --version`, with a short timeout.
//!
//! ## Windows shims
//!
//! npm installs CLIs on Windows as `.cmd` and `.ps1` shims next to an extension-less `sh`
//! script. PATH lookup honours `PATHEXT` (default `.COM;.EXE;.BAT;.CMD`, then `.PS1`), so the
//! `.cmd` shim wins, and the extension-less script is never picked on Windows.
//!
//! - `.exe`, `.com`: spawned directly.
//! - `.cmd`, `.bat`: spawned directly too. Rust's `std::process::Command` (1.77+) runs batch
//!   files through `cmd.exe /d /c` with cmd-safe quoting, and refuses an argument it cannot quote
//!   safely rather than passing it on. The prompt never goes through the command line (it is
//!   written to stdin), so only flags, ids and `extra-args` are subject to that.
//! - `.ps1`: run as `powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass
//!   -File <script> …`, because `CreateProcess` cannot start a script. A `.cmd` beside it is
//!   preferred when present.
//!
//! Resolving the shim's node target was rejected: shim layouts differ between npm, pnpm, volta
//! and hand-written shims (pi's own picks a newer node), and the shim is what the user runs.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fm_model::{CliId, CliInstall, CliSettings};

use crate::adapter;
use crate::common::one_line;

/// How long `<cli> --version` may take. Node CLIs can be slow to start on a cold cache.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The host for `.ps1` scripts: Windows PowerShell, which every Windows has; `pwsh` elsewhere.
const POWERSHELL: &str = if cfg!(windows) {
    "powershell.exe"
} else {
    "pwsh"
};

/// The program and leading arguments that start `program`. Only `.ps1` needs a host.
pub fn launcher(program: &Path) -> (PathBuf, Vec<String>) {
    let ext = program
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase);
    if ext.as_deref() == Some("ps1") {
        let cmd = program.with_extension("cmd");
        if cmd.is_file() {
            return (cmd, Vec::new());
        }
        return (
            PathBuf::from(POWERSHELL),
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                program.display().to_string(),
            ],
        );
    }
    (program.to_path_buf(), Vec::new())
}

/// Find `name` on `path` (a `PATH` value). On Windows each `PATHEXT` extension is tried in
/// order, then `.PS1`; a name that already has one of those extensions is tried as is.
pub fn find_in_path(name: &str, path: &OsStr, pathext: Option<&OsStr>) -> Option<PathBuf> {
    let exts = if cfg!(windows) {
        windows_exts(pathext)
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = find_in_dir(&dir, name, &exts) {
            return Some(found);
        }
    }
    None
}

fn windows_exts(pathext: Option<&OsStr>) -> Vec<String> {
    let raw = pathext
        .and_then(OsStr::to_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_PATHEXT);
    let mut exts: Vec<String> = raw
        .split(';')
        .map(str::trim)
        .filter(|e| e.starts_with('.') && e.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect();
    if !exts.iter().any(|e| e == ".ps1") {
        exts.push(".ps1".into());
    }
    exts
}

fn find_in_dir(dir: &Path, name: &str, exts: &[String]) -> Option<PathBuf> {
    if exts.is_empty() {
        let candidate = dir.join(name);
        return is_executable(&candidate).then_some(candidate);
    }
    let lower = name.to_ascii_lowercase();
    if exts.iter().any(|e| lower.ends_with(e.as_str())) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    exts.iter()
        .map(|ext| dir.join(format!("{name}{ext}")))
        .find(|c| c.is_file())
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Find the CLI (`settings.path`, else PATH) and ask it for its version.
pub fn detect(id: CliId, settings: &CliSettings) -> Result<CliInstall, String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var_os("PATHEXT");
    detect_with(id, settings, &path, pathext.as_deref(), VERSION_TIMEOUT)
}

/// [`detect`] with the environment and timeout passed in, for tests.
pub fn detect_with(
    id: CliId,
    settings: &CliSettings,
    path: &OsStr,
    pathext: Option<&OsStr>,
    timeout: Duration,
) -> Result<CliInstall, String> {
    let cli = adapter(id);
    let program = locate(id, settings, path, pathext)?;
    let (launch, mut args) = launcher(&program);
    args.extend(cli.version_args().iter().map(|a| a.to_string()));
    let output = run_bounded(&launch, &args, timeout).map_err(|e| {
        one_line(
            &format!(
                "{id}: {} could not be started ({e}) — check the file, or set agents.{id}.path in config.toml",
                program.display()
            ),
            400,
        )
    })?;
    Ok(CliInstall {
        path: program,
        version: output.as_deref().and_then(parse_version),
    })
}

fn locate(
    id: CliId,
    settings: &CliSettings,
    path: &OsStr,
    pathext: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(p) = &settings.path {
        if p.is_file() {
            return Ok(p.clone());
        }
        return Err(format!(
            "agents.{id}.path: {} is not a file — fix the path in config.toml, or remove it to search PATH",
            p.display()
        ));
    }
    let name = adapter(id).program_name();
    find_in_path(name, path, pathext).ok_or_else(|| {
        format!(
            "{id}: {name} is not on PATH — install {}, or set agents.{id}.path in config.toml",
            id.title()
        )
    })
}

/// Run a short command; stdout and stderr together, or `None` if it did not finish in time.
/// Kills only the process it started.
fn run_bounded(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> std::io::Result<Option<String>> {
    let mut attempt = 0;
    let mut child = loop {
        let spawned = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        match spawned {
            // ETXTBSY: a just-written script still open in a process forked meanwhile.
            Err(e) if cfg!(unix) && e.raw_os_error() == Some(26) && attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => break other?,
        }
    };
    let (tx, rx) = mpsc::channel::<String>();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.take(64 * 1024).read_to_end(&mut buf);
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
        });
    }
    drop(tx);

    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // A shim's grandchild can hold the pipes open after the shim exits; do not wait for it.
    let mut out = String::new();
    let until = Instant::now() + Duration::from_millis(1500);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(s) => out.push_str(&s),
            Err(_) => break,
        }
    }
    Ok(Some(out))
}

/// The first `1.2.3`-like token: `GitHub Copilot CLI 1.0.104.` → `1.0.104`,
/// `2.1.122 (Claude Code)` → `2.1.122`, `codex-cli 0.165.0` → `0.165.0`.
pub fn parse_version(output: &str) -> Option<String> {
    for token in output.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',') {
        let t = token.trim_start_matches(['v', 'V']).trim_end_matches('.');
        let mut parts = t.split('.');
        let numeric_head = parts
            .next()
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        if numeric_head
            && t.contains('.')
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        {
            return Some(t.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(
            parse_version("GitHub Copilot CLI 1.0.104.\nRun 'copilot update'"),
            Some("1.0.104".into())
        );
        assert_eq!(
            parse_version("2.1.122 (Claude Code)"),
            Some("2.1.122".into())
        );
        assert_eq!(parse_version("codex-cli 0.165.0"), Some("0.165.0".into()));
        assert_eq!(parse_version("1.2.0\n"), Some("1.2.0".into()));
        assert_eq!(parse_version("v0.66.0"), Some("0.66.0".into()));
        assert_eq!(
            parse_version("fm-fake-agent 0.1.0-dev"),
            Some("0.1.0-dev".into())
        );
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn ps1_goes_through_powershell_unless_a_cmd_sits_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let ps1 = dir.path().join("tool.ps1");
        std::fs::write(&ps1, "").unwrap();
        let (prog, args) = launcher(&ps1);
        assert_eq!(prog, PathBuf::from(POWERSHELL));
        assert_eq!(
            args.last().map(String::as_str),
            Some(ps1.display().to_string().as_str())
        );
        assert!(args.contains(&"-File".to_string()));

        std::fs::write(dir.path().join("tool.cmd"), "").unwrap();
        let (prog, args) = launcher(&ps1);
        assert_eq!(prog, dir.path().join("tool.cmd"));
        assert!(args.is_empty());

        let exe = dir.path().join("tool.exe");
        assert_eq!(launcher(&exe), (exe.clone(), Vec::new()));
    }

    #[test]
    fn windows_exts_default_and_ps1_last() {
        let exts = windows_exts(None);
        assert_eq!(exts, [".com", ".exe", ".bat", ".cmd", ".ps1"]);
        let exts = windows_exts(Some(OsStr::new(".EXE;.PS1;.CMD")));
        assert_eq!(exts, [".exe", ".ps1", ".cmd"]);
    }
}

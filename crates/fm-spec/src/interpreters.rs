//! Which command starts a script, resolved to an absolute path: a scheduled job has no shell `PATH`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The OS whose interpreter table applies. Injected so every table is testable on any machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    /// The OS this binary was built for (other Unixes count as Linux).
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }

    pub fn is_windows(self) -> bool {
        self == Platform::Windows
    }
}

/// How to start one script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpreter {
    /// Absolute path of the executable to start.
    pub program: PathBuf,
    /// Arguments before the script, such as `-NoProfile -File` or `/c`.
    pub args: Vec<String>,
    /// The script, absolute. Empty args and `program == script` for an executable file.
    pub script: PathBuf,
    /// What the consent preview shows, e.g. `/usr/bin/python3 scripts/post.py`.
    pub display: String,
}

impl Interpreter {
    /// Every argument to pass to `program`: `args`, then the script unless it is the program.
    pub fn command_args(&self) -> Vec<OsString> {
        let mut out: Vec<OsString> = self.args.iter().map(OsString::from).collect();
        if self.program != self.script {
            out.push(self.script.clone().into_os_string());
        }
        out
    }
}

/// A `which` lookup: a program name to an absolute path, or `None`.
pub type Which<'a> = &'a dyn Fn(&str) -> Option<PathBuf>;

fn is_executable_file(p: &Path, windows: bool) -> bool {
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => windows || has_executable_bit(p),
        _ => false,
    }
}

/// True when the file has any executable bit. Always false where there are none (Windows).
pub fn has_executable_bit(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

/// Find `bin` in the directories of `path_var`, trying each of `pathext` on Windows.
pub fn which_in(
    bin: &str,
    path_var: &std::ffi::OsStr,
    pathext: Option<&str>,
    platform: Platform,
) -> Option<PathBuf> {
    let windows = platform.is_windows();
    if bin.contains('/') || bin.contains('\\') {
        let p = Path::new(bin);
        return is_executable_file(p, windows)
            .then(|| std::path::absolute(p).ok())
            .flatten();
    }
    let exts: Vec<String> = if windows {
        pathext
            .unwrap_or(".COM;.EXE;.BAT;.CMD")
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(path_var) {
        let dir_text = dir.to_string_lossy();
        let dir = PathBuf::from(dir_text.trim_matches('"'));
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &exts {
            let candidate = dir.join(format!("{bin}{ext}"));
            if is_executable_file(&candidate, windows) {
                return std::path::absolute(&candidate).ok();
            }
        }
    }
    None
}

/// Find `bin` on this process's `PATH`, without starting `which` or `where`.
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").ok();
    which_in(bin, &path, pathext.as_deref(), Platform::current())
}

fn first_of(which: Which<'_>, bins: &[&str]) -> Option<PathBuf> {
    bins.iter().find_map(|b| which(b))
}

/// Decide how to start `abs_script`; `rel` is the path as written, for the display. `Err` is the
/// reason that completes `hooks.pre[0]: <script> `, e.g. `cannot run here — needs pwsh (PowerShell 7)
/// on this OS`.
pub fn resolve_interpreter(
    abs_script: &Path,
    rel: &str,
    platform: Platform,
    which: Which<'_>,
) -> Result<Interpreter, String> {
    let windows = platform.is_windows();
    let ext = abs_script
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();

    let ok = |program: PathBuf, args: &[&str]| -> Result<Interpreter, String> {
        let mut display = vec![program.display().to_string()];
        display.extend(args.iter().map(|a| a.to_string()));
        display.push(rel.to_string());
        Ok(Interpreter {
            program,
            args: args.iter().map(|a| a.to_string()).collect(),
            script: abs_script.to_path_buf(),
            display: display.join(" "),
        })
    };

    match ext.as_str() {
        ".js" | ".mjs" | ".cjs" => match first_of(which, &["node"]) {
            Some(node) => ok(node, &[]),
            None => Err("cannot run here — needs node (Node.js) on this OS".to_string()),
        },
        ".py" => {
            let bins: &[&str] = if windows {
                &["py", "python3", "python"]
            } else {
                &["python3", "python"]
            };
            let Some(py) = first_of(which, bins) else {
                return Err("cannot run here — needs python3 on this OS".to_string());
            };
            // `py` is the Windows launcher; `-3` picks Python 3 rather than whatever is first.
            // The injected platform can differ from the host running this code or its tests.
            let launcher = py
                .to_string_lossy()
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|name| name.to_ascii_lowercase().starts_with("py."));
            if windows && launcher {
                ok(py, &["-3"])
            } else {
                ok(py, &[])
            }
        }
        ".sh" => {
            let bins: &[&str] = if windows { &["bash"] } else { &["bash", "sh"] };
            match first_of(which, bins) {
                Some(sh) => ok(sh, &[]),
                None if windows => Err("cannot run here — needs bash on this OS, so install Git Bash or use a .ps1 or .cmd script".to_string()),
                None => Err("cannot run here — needs bash or sh on this OS".to_string()),
            }
        }
        ".ps1" => {
            if let Some(pwsh) = which("pwsh") {
                return ok(pwsh, &["-NoProfile", "-File"]);
            }
            if windows && let Some(ps) = which("powershell") {
                return ok(ps, &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            }
            Err("cannot run here — needs pwsh (PowerShell 7) on this OS".to_string())
        }
        ".cmd" | ".bat" => {
            if !windows {
                return Err("cannot run here — .cmd and .bat need Windows".to_string());
            }
            let cmd = which("cmd").or_else(|| {
                std::env::var_os("ComSpec")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
            });
            match cmd {
                Some(cmd) => ok(cmd, &["/c"]),
                None => Err("cannot run here — cmd.exe was not found".to_string()),
            }
        }
        "" => {
            if windows {
                return Err(
                    "cannot run here — Windows needs a file extension to know how to run a script"
                        .to_string(),
                );
            }
            if !abs_script.exists() {
                return Err("cannot run here — the file is missing".to_string());
            }
            if !has_executable_bit(abs_script) {
                return Err(
                    "cannot run here — it has no extension and is not executable, so chmod +x it"
                        .to_string(),
                );
            }
            Ok(Interpreter {
                program: abs_script.to_path_buf(),
                args: Vec::new(),
                script: abs_script.to_path_buf(),
                display: rel.to_string(),
            })
        }
        other => Err(format!(
            "cannot run here — no interpreter for \"{other}\" on this OS"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn table() -> HashMap<&'static str, PathBuf> {
        [
            ("node", "/usr/local/bin/node"),
            ("python3", "/usr/bin/python3"),
            ("python", "/usr/bin/python"),
            ("py", "C:\\Windows\\py.exe"),
            ("bash", "/bin/bash"),
            ("sh", "/bin/sh"),
            ("pwsh", "/usr/local/bin/pwsh"),
            (
                "powershell",
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            ),
            ("cmd", "C:\\Windows\\System32\\cmd.exe"),
        ]
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect()
    }

    const ALL: [Platform; 3] = [Platform::Windows, Platform::MacOs, Platform::Linux];

    fn resolve(
        script: &str,
        platform: Platform,
        map: &HashMap<&'static str, PathBuf>,
    ) -> Result<Interpreter, String> {
        let which = |b: &str| map.get(b).cloned();
        resolve_interpreter(Path::new(script), script, platform, &which)
    }

    fn none() -> HashMap<&'static str, PathBuf> {
        HashMap::new()
    }

    fn only(bin: &'static str, path: &str) -> HashMap<&'static str, PathBuf> {
        [(bin, PathBuf::from(path))].into_iter().collect()
    }

    #[test]
    fn javascript_runs_with_node_everywhere() {
        for p in ALL {
            for ext in ["js", "mjs", "cjs"] {
                let r = resolve(&format!("s.{ext}"), p, &table()).unwrap();
                assert_eq!(r.program, PathBuf::from("/usr/local/bin/node"));
                assert!(r.args.is_empty());
                assert_eq!(r.command_args(), [OsString::from(format!("s.{ext}"))]);
            }
            assert!(
                resolve("s.js", p, &none())
                    .unwrap_err()
                    .contains("needs node")
            );
        }
    }

    #[test]
    fn python_prefers_python3_and_the_py_launcher_on_windows() {
        for p in [Platform::Linux, Platform::MacOs] {
            let r = resolve("scripts/s.py", p, &table()).unwrap();
            assert_eq!(r.program, PathBuf::from("/usr/bin/python3"));
            assert_eq!(
                r.display,
                format!("{} scripts/s.py", Path::new("/usr/bin/python3").display())
            );
            let fallback = resolve("s.py", p, &only("python", "/usr/bin/python")).unwrap();
            assert_eq!(fallback.program, PathBuf::from("/usr/bin/python"));
            assert!(
                resolve("s.py", p, &none())
                    .unwrap_err()
                    .contains("needs python3")
            );
        }
        let win = resolve("s.py", Platform::Windows, &table()).unwrap();
        assert_eq!(win.program, PathBuf::from("C:\\Windows\\py.exe"));
        assert_eq!(win.args, ["-3"]);
        let no_launcher = resolve(
            "s.py",
            Platform::Windows,
            &only("python", "C:\\Python\\python.exe"),
        )
        .unwrap();
        assert!(no_launcher.args.is_empty());
    }

    #[test]
    fn windows_python_launcher_recognizes_both_path_separators_and_case() {
        for path in [
            r"C:\Windows\py.exe",
            r"C:\Tools\PY.EXE",
            "C:/Windows/py.exe",
            r"\\server\share\Py.EXE",
        ] {
            let resolved = resolve("s.py", Platform::Windows, &only("py", path)).unwrap();
            assert_eq!(resolved.args, ["-3"], "{path}");
        }

        for path in [r"C:\Python\python.exe", r"C:\Tools\pypy.exe"] {
            let resolved = resolve("s.py", Platform::Windows, &only("python", path)).unwrap();
            assert!(resolved.args.is_empty(), "{path}");
        }
    }

    #[test]
    fn shell_scripts_need_bash_on_windows() {
        for p in [Platform::Linux, Platform::MacOs] {
            assert_eq!(
                resolve("s.sh", p, &table()).unwrap().program,
                PathBuf::from("/bin/bash")
            );
            assert_eq!(
                resolve("s.sh", p, &only("sh", "/bin/sh")).unwrap().program,
                PathBuf::from("/bin/sh")
            );
            assert!(
                resolve("s.sh", p, &none())
                    .unwrap_err()
                    .contains("bash or sh")
            );
        }
        assert!(
            resolve("s.sh", Platform::Windows, &only("sh", "C:\\sh.exe"))
                .unwrap_err()
                .contains("Git Bash")
        );
        assert_eq!(
            resolve(
                "s.sh",
                Platform::Windows,
                &only("bash", "C:\\Git\\bin\\bash.exe")
            )
            .unwrap()
            .program,
            PathBuf::from("C:\\Git\\bin\\bash.exe")
        );
    }

    #[test]
    fn powershell_prefers_pwsh_and_falls_back_on_windows_only() {
        for p in ALL {
            let r = resolve("s.ps1", p, &table()).unwrap();
            assert_eq!(r.program, PathBuf::from("/usr/local/bin/pwsh"));
            assert_eq!(r.args, ["-NoProfile", "-File"]);
        }
        let ps = table()["powershell"].clone();
        let win = resolve(
            "s.ps1",
            Platform::Windows,
            &only("powershell", &ps.to_string_lossy()),
        )
        .unwrap();
        assert_eq!(win.program, ps);
        assert_eq!(
            win.args,
            ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
        );
        for p in [Platform::Linux, Platform::MacOs] {
            assert_eq!(
                resolve(
                    "scripts/count.ps1",
                    p,
                    &only("powershell", &ps.to_string_lossy())
                )
                .unwrap_err(),
                "cannot run here — needs pwsh (PowerShell 7) on this OS"
            );
        }
    }

    #[test]
    fn cmd_and_bat_are_windows_only() {
        for ext in ["cmd", "bat"] {
            let win = resolve(&format!("s.{ext}"), Platform::Windows, &table()).unwrap();
            assert_eq!(win.args, ["/c"]);
            assert_eq!(win.program, PathBuf::from("C:\\Windows\\System32\\cmd.exe"));
            for p in [Platform::Linux, Platform::MacOs] {
                assert!(
                    resolve(&format!("s.{ext}"), p, &table())
                        .unwrap_err()
                        .contains("need Windows")
                );
            }
        }
    }

    #[test]
    fn unknown_extensions_are_named() {
        for p in ALL {
            assert_eq!(
                resolve("s.rb", p, &table()).unwrap_err(),
                "cannot run here — no interpreter for \".rb\" on this OS"
            );
        }
        assert!(resolve("S.PY", Platform::Linux, &table()).is_ok());
    }

    #[test]
    fn extensionless_files_run_directly_when_executable() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("runner");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let which = |_: &str| None;
        let r = resolve_interpreter(&script, "runner", Platform::Linux, &which);
        if has_executable_bit(&script) {
            let r = r.unwrap();
            assert_eq!(r.program, script);
            assert!(r.command_args().is_empty());
            assert_eq!(r.display, "runner");
        } else {
            assert!(r.unwrap_err().contains("chmod +x"));
        }
        assert!(
            resolve_interpreter(&script, "runner", Platform::Windows, &which)
                .unwrap_err()
                .contains("file extension")
        );
        assert!(
            resolve_interpreter(
                &dir.path().join("missing"),
                "missing",
                Platform::Linux,
                &which
            )
            .unwrap_err()
            .contains("missing")
        );
    }

    #[test]
    fn which_reads_path_and_pathext() {
        let dir = tempfile::tempdir().unwrap();
        let platform = Platform::current();
        let name = if platform.is_windows() {
            "tool.EXE"
        } else {
            "tool"
        };
        let file = dir.path().join(name);
        std::fs::write(&file, "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        let found = which_in("tool", &path, Some(".COM;.EXE"), platform).unwrap();
        assert!(found.is_absolute());
        assert!(found.ends_with(name));
        assert_eq!(which_in("not-there-zzz", &path, None, platform), None);
        assert!(which_in(&file.to_string_lossy(), &path, None, platform).is_some());
        assert_eq!(which("definitely-not-a-real-binary-zzz"), None);
    }
}

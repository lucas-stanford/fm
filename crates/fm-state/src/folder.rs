//! Where the state folder is: `FM_HOME`, else the per-OS folder. The environment is injectable so
//! tests need no particular home directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fm_model::StatePaths;

use crate::error::{Result, StateError};
use crate::fsio::ensure_dir;
use crate::keys::normalize_path;

/// What the folder depends on: environment variables and the home directory.
pub trait Env {
    /// The variable's value; empty or whitespace counts as unset.
    fn var(&self, name: &str) -> Option<String>;
    fn home_dir(&self) -> Option<PathBuf>;
}

/// This process's environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.trim().is_empty())
    }
    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }
}

/// An environment built by hand, for tests.
#[derive(Debug, Clone, Default)]
pub struct FakeEnv {
    pub vars: BTreeMap<String, String>,
    pub home: Option<PathBuf>,
}

impl FakeEnv {
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        FakeEnv {
            vars: BTreeMap::new(),
            home: Some(home.into()),
        }
    }
    pub fn set(mut self, name: &str, value: impl Into<String>) -> Self {
        self.vars.insert(name.to_string(), value.into());
        self
    }
}

impl Env for FakeEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.vars
            .get(name)
            .filter(|v| !v.trim().is_empty())
            .cloned()
    }
    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// Which convention the folder follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    /// Linux and every other Unix: the XDG convention.
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

const FOLDER: &str = "agent-automations";

/// The state folder for this environment, not created.
pub fn state_dir(env: &dyn Env, os: Os) -> Result<PathBuf> {
    if let Some(home) = env.var("FM_HOME") {
        return Ok(normalize_path(Path::new(home.trim())));
    }
    let home = || {
        env.home_dir().ok_or_else(|| {
            StateError::Folder(
                "cannot find the home directory — set FM_HOME to a folder for fm's state".into(),
            )
        })
    };
    let dir = match os {
        Os::Windows => match env.var("LOCALAPPDATA") {
            Some(local) => PathBuf::from(local).join(FOLDER),
            None => home()?.join("AppData").join("Local").join(FOLDER),
        },
        Os::MacOs => home()?
            .join("Library")
            .join("Application Support")
            .join(FOLDER),
        Os::Linux => match env.var("XDG_STATE_HOME").map(PathBuf::from) {
            // The XDG spec says a relative path is invalid and must be ignored.
            Some(xdg) if xdg.is_absolute() => xdg.join(FOLDER),
            _ => home()?.join(".local").join("state").join(FOLDER),
        },
    };
    Ok(normalize_path(&dir))
}

/// The state folder of this process, created and laid out.
pub fn state_paths() -> Result<StatePaths> {
    let paths = StatePaths::new(state_dir(&ProcessEnv, Os::current())?);
    open_state(&paths)?;
    Ok(paths)
}

/// Create the folder and its subfolders, owner-only (0700) on Unix, tightening an existing root.
pub fn open_state(paths: &StatePaths) -> Result<()> {
    ensure_dir(&paths.root)?;
    #[cfg(unix)]
    {
        use crate::error::IoContext;
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&paths.root).at(&paths.root)?;
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(&paths.root, std::fs::Permissions::from_mode(0o700))
                .at(&paths.root)?;
        }
    }
    for dir in [
        paths.runs_dir(),
        paths.locks_dir(),
        paths.progress_dir(),
        paths.sessions_dir(),
        paths.logs_dir(),
    ] {
        ensure_dir(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        std::env::temp_dir().join("fm-home-test")
    }

    #[test]
    fn fm_home_wins_everywhere() {
        let target = std::env::temp_dir().join("fm-explicit");
        let env = FakeEnv::with_home(home()).set("FM_HOME", target.to_string_lossy());
        for os in [Os::Windows, Os::MacOs, Os::Linux] {
            assert_eq!(state_dir(&env, os).unwrap(), normalize_path(&target));
        }
    }

    #[test]
    fn windows_uses_local_app_data() {
        let local = std::env::temp_dir().join("Local");
        let env = FakeEnv::with_home(home()).set("LOCALAPPDATA", local.to_string_lossy());
        assert_eq!(
            state_dir(&env, Os::Windows).unwrap(),
            normalize_path(&local.join("agent-automations"))
        );
        let bare = FakeEnv::with_home(home());
        assert_eq!(
            state_dir(&bare, Os::Windows).unwrap(),
            normalize_path(
                &home()
                    .join("AppData")
                    .join("Local")
                    .join("agent-automations")
            )
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let env = FakeEnv::with_home(home());
        assert_eq!(
            state_dir(&env, Os::MacOs).unwrap(),
            normalize_path(
                &home()
                    .join("Library")
                    .join("Application Support")
                    .join("agent-automations")
            )
        );
    }

    #[test]
    fn linux_uses_xdg_state_home_else_local_state() {
        let xdg = std::env::temp_dir().join("xdg-state");
        let env = FakeEnv::with_home(home()).set("XDG_STATE_HOME", xdg.to_string_lossy());
        assert_eq!(
            state_dir(&env, Os::Linux).unwrap(),
            normalize_path(&xdg.join("agent-automations"))
        );
        let relative = FakeEnv::with_home(home()).set("XDG_STATE_HOME", "relative/state");
        let fallback = normalize_path(
            &home()
                .join(".local")
                .join("state")
                .join("agent-automations"),
        );
        assert_eq!(state_dir(&relative, Os::Linux).unwrap(), fallback);
        let blank = FakeEnv::with_home(home()).set("XDG_STATE_HOME", "  ");
        assert_eq!(state_dir(&blank, Os::Linux).unwrap(), fallback);
    }

    #[test]
    fn no_home_is_one_line_naming_fm_home() {
        let err = state_dir(&FakeEnv::default(), Os::Linux)
            .unwrap_err()
            .to_string();
        assert!(err.contains("FM_HOME") && !err.contains('\n'), "{err}");
    }

    #[test]
    fn open_state_lays_out_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path().join("fm"));
        open_state(&paths).unwrap();
        open_state(&paths).unwrap();
        for d in [paths.runs_dir(), paths.locks_dir(), paths.progress_dir()] {
            assert!(d.is_dir(), "{}", d.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&paths.root).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }
}

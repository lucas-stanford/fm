//! Everything the host reads from its environment, gathered once so tests can pass their own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fm_model::StatePaths;
use fm_state::{Env, Os, open_state, pa_trust_path, state_dir};

/// The home directory as Node's `os.homedir()` finds it, which is what pi-automations uses:
/// `USERPROFILE` on Windows, `HOME` elsewhere, then the system's answer.
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            #[allow(deprecated)]
            std::env::home_dir()
        })
}

/// This process's variables, with the home directory from [`home_dir`].
struct ProcessVars {
    home: Option<PathBuf>,
}

impl Env for ProcessVars {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.trim().is_empty())
    }
    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// Where the host looks for things.
#[derive(Debug, Clone)]
pub struct HostEnv {
    /// `~`: the user scope is `~/.agents/automations`.
    pub home: PathBuf,
    /// Repository roots are found from here up.
    pub cwd: PathBuf,
    pub state: StatePaths,
    /// pi-automations' `trust.json`, read only.
    pub pa_trust: Option<PathBuf>,
    /// `PATH`, for interpreters, agent CLIs and fm.
    pub path_var: OsString,
    pub pathext: Option<String>,
    /// This program, to start `fm fire`.
    pub exe: PathBuf,
}

impl HostEnv {
    /// This process: `FM_HOME` or the per-OS state folder (created), the working directory, PATH.
    pub fn from_process() -> anyhow::Result<HostEnv> {
        let home = home_dir().ok_or_else(|| {
            anyhow::anyhow!("cannot find the home directory — set HOME (USERPROFILE on Windows)")
        })?;
        let vars = ProcessVars {
            home: Some(home.clone()),
        };
        let state = StatePaths::new(state_dir(&vars, Os::current())?);
        open_state(&state)?;
        Ok(HostEnv {
            pa_trust: pa_trust_path(&vars),
            cwd: std::env::current_dir()?,
            home,
            state,
            path_var: std::env::var_os("PATH").unwrap_or_default(),
            pathext: std::env::var("PATHEXT").ok(),
            exe: std::env::current_exe()?,
        })
    }

    /// For tests: everything under folders the caller owns, an empty PATH, no pa.
    pub fn isolated(home: &Path, cwd: &Path, state: &Path) -> HostEnv {
        HostEnv {
            home: home.to_path_buf(),
            cwd: cwd.to_path_buf(),
            state: StatePaths::new(state),
            pa_trust: None,
            path_var: OsString::new(),
            pathext: None,
            exe: std::env::current_exe().unwrap_or_default(),
        }
    }
}

//! Fixtures for the `fm` tests: a home, a working directory and a state folder under the cargo
//! target folder, never the real `~/.agents/automations` or state folder.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use fm::env::HostEnv;
use fm::host::RealHost;

pub struct Fx {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub state: PathBuf,
    /// `PI_AGENT_DIR`.
    pub pa: PathBuf,
    /// An empty folder used as `PATH`.
    pub bin: PathBuf,
}

impl Fx {
    pub fn new() -> Fx {
        let dir = tempfile::Builder::new()
            .prefix("fm-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let root = fm_spec::normalize(dir.path());
        let fx = Fx {
            home: root.join("home"),
            cwd: root.join("work"),
            state: root.join("state"),
            pa: root.join("pa"),
            bin: root.join("bin"),
            root,
            _dir: dir,
        };
        for d in [&fx.home, &fx.cwd, &fx.state, &fx.pa, &fx.bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        fx
    }

    pub fn user_root(&self) -> PathBuf {
        self.home.join(".agents").join("automations")
    }

    pub fn user_key(&self, name: &str) -> String {
        format!("user:{}:{name}", self.user_root().display())
    }

    /// Make the working directory a repository with its own automations folder.
    pub fn repo_root(&self) -> PathBuf {
        std::fs::create_dir_all(self.cwd.join(".git")).unwrap();
        self.cwd.join(".agents").join("automations")
    }

    pub fn repo_key(&self, name: &str) -> String {
        format!("repository:{}:{name}", self.repo_root().display())
    }

    /// Write `<root>/<name>/AUTOMATION.md` with extra frontmatter lines.
    pub fn write(&self, root: &Path, name: &str, extra: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("AUTOMATION.md");
        let text = format!(
            "---\nname: {name}\ndescription: Says hello for {name}.\n{extra}---\n\nSay hello.\n"
        );
        std::fs::write(&file, text).unwrap();
        file
    }

    pub fn manual(&self, name: &str) -> PathBuf {
        self.write(&self.user_root(), name, "on:\n  manual: true\n")
    }

    pub fn config(&self, text: &str) {
        std::fs::write(self.state.join("config.toml"), text).unwrap();
    }

    pub fn config_text(&self) -> String {
        std::fs::read_to_string(self.state.join("config.toml")).unwrap_or_default()
    }

    /// pa's trust.json, with these keys armed.
    pub fn pa_armed(&self, keys: &[String]) {
        let dir = self.pa.join("pi-automations");
        std::fs::create_dir_all(&dir).unwrap();
        let records: serde_json::Map<String, serde_json::Value> = keys
            .iter()
            .map(|k| {
                (
                    k.clone(),
                    serde_json::json!({ "key": k, "digest": "sha256:x", "armedAt": "2026-01-01T00:00:00.000Z" }),
                )
            })
            .collect();
        std::fs::write(
            dir.join("trust.json"),
            serde_json::json!({ "version": 1, "records": records }).to_string(),
        )
        .unwrap();
    }

    pub fn env(&self) -> HostEnv {
        let mut env = HostEnv::isolated(&self.home, &self.cwd, &self.state);
        env.pa_trust = Some(self.pa.join("pi-automations").join("trust.json"));
        env.path_var = self.bin.clone().into_os_string();
        env.exe = fm_exe();
        env
    }

    pub fn host(&self) -> RealHost {
        RealHost::new(self.env())
    }

    /// `fm` with every folder pointed into this fixture.
    pub fn fm(&self) -> assert_cmd::Command {
        let mut c = assert_cmd::Command::new(fm_exe());
        c.current_dir(&self.cwd)
            .env("FM_HOME", &self.state)
            .env("FM_NO_NOTIFY", "1")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("PI_AGENT_DIR", &self.pa)
            .env("PATH", &self.bin)
            .env_remove("FM_BIN")
            .env_remove("FM_RUN");
        c
    }
}

/// The `fm` binary: config can point an agent CLI at it, since `fm --version` prints a version.
pub fn fm_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fm"))
}

/// TOML literal string of a path.
pub fn toml_path(p: &Path) -> String {
    format!("'{}'", p.display())
}

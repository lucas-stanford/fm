//! `config.toml` in the state folder, and the agent CLIs fm knows.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An agent CLI fm can run a body through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliId {
    Pi,
    Copilot,
    Claude,
    Codex,
    Gemini,
    Opencode,
}

impl CliId {
    pub const ALL: [CliId; 6] = [
        CliId::Pi,
        CliId::Copilot,
        CliId::Claude,
        CliId::Codex,
        CliId::Gemini,
        CliId::Opencode,
    ];

    /// The name used in config, records and the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            CliId::Pi => "pi",
            CliId::Copilot => "copilot",
            CliId::Claude => "claude",
            CliId::Codex => "codex",
            CliId::Gemini => "gemini",
            CliId::Opencode => "opencode",
        }
    }

    /// The name people know it by.
    pub fn title(self) -> &'static str {
        match self {
            CliId::Pi => "pi",
            CliId::Copilot => "GitHub Copilot CLI",
            CliId::Claude => "Claude Code",
            CliId::Codex => "Codex CLI",
            CliId::Gemini => "Gemini CLI",
            CliId::Opencode => "OpenCode",
        }
    }
}

impl fmt::Display for CliId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CliId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CliId::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| {
                let names: Vec<_> = CliId::ALL.iter().map(|c| c.as_str()).collect();
                format!(
                    "unknown agent CLI \"{s}\" — use one of {}",
                    names.join(", ")
                )
            })
    }
}

/// Per-CLI settings. Everything optional: absent means the adapter's documented default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct CliSettings {
    /// Absolute path to the executable, for a scheduled job that has no PATH.
    pub path: Option<PathBuf>,
    /// Extra arguments, appended after fm's own. Shown in the consent preview.
    pub extra_args: Vec<String>,
    /// Let the agent use tools without asking (each CLI's own flag). Off unless set.
    pub allow_all_tools: bool,
    /// Model used when an automation has no `model:`. Absent: the CLI's own default.
    pub default_model: Option<String>,
}

/// Per-automation settings, keyed by automation key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct AutomationSettings {
    /// Overrides `default-agent` for this automation.
    pub agent: Option<CliId>,
    /// Run inside a temporary git worktree.
    pub worktree: bool,
}

/// `config.toml`. Durations are raw strings such as `90s`, `7d`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    /// The CLI that runs a body unless an automation overrides it.
    pub default_agent: Option<CliId>,
    /// How late a tick may be and still count as on time.
    pub grace: String,
    /// How far back `catch-up: once` looks.
    pub catch_up_window: String,
    pub default_timeout: String,
    pub max_timeout: String,
    pub hook_timeout: String,
    pub hook_max_output_bytes: u64,
    pub stagger_max_seconds: u32,
    /// Repository consent expires after this long.
    pub consent_expiry: String,
    pub retention: Retention,
    pub notify_on_failed: bool,
    pub agents: BTreeMap<CliId, CliSettings>,
    pub automations: BTreeMap<String, AutomationSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Retention {
    pub max_lines_per_automation: u32,
    pub keep_lines: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            max_lines_per_automation: 2000,
            keep_lines: 1000,
        }
    }
}

impl Default for Config {
    /// The same defaults pi-automations documents.
    fn default() -> Self {
        Config {
            default_agent: None,
            grace: "5m".into(),
            catch_up_window: "7d".into(),
            default_timeout: "30m".into(),
            max_timeout: "4h".into(),
            hook_timeout: "5m".into(),
            hook_max_output_bytes: 65536,
            stagger_max_seconds: 30,
            consent_expiry: "30d".into(),
            retention: Retention::default(),
            notify_on_failed: true,
            agents: BTreeMap::new(),
            automations: BTreeMap::new(),
        }
    }
}

impl Config {
    /// The CLI for this automation: its override, else the default.
    pub fn agent_for(&self, key: &str) -> Option<CliId> {
        self.automations
            .get(key)
            .and_then(|a| a.agent)
            .or(self.default_agent)
    }

    pub fn agent_settings(&self, cli: CliId) -> CliSettings {
        self.agents.get(&cli).cloned().unwrap_or_default()
    }
}

//! `config.toml` in the state folder: host policy, agent CLIs, per-automation settings.
//!
//! No file means the defaults. A file fm cannot honour is refused whole, in one line naming the key
//! (`config.toml: unknown key "x" — …`), rather than half applied.

use std::path::Path;

use fm_model::{CliId, Config, StatePaths};

use crate::duration::parse_duration;
use crate::error::{Result, StateError};
use crate::fsio::{with_lock, write_atomic};

const TOP: &[&str] = &[
    "default-agent",
    "grace",
    "catch-up-window",
    "default-timeout",
    "max-timeout",
    "hook-timeout",
    "hook-max-output-bytes",
    "stagger-max-seconds",
    "consent-expiry",
    "retention",
    "notify-on-failed",
    "agents",
    "automations",
];
const RETENTION: &[&str] = &["max-lines-per-automation", "keep-lines"];
const AGENT: &[&str] = &["path", "extra-args", "allow-all-tools", "default-model"];
const AUTOMATION: &[&str] = &["agent", "worktree"];

/// The duration fields of [`Config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDuration {
    Grace,
    CatchUpWindow,
    DefaultTimeout,
    MaxTimeout,
    HookTimeout,
    ConsentExpiry,
}

impl ConfigDuration {
    pub const ALL: [ConfigDuration; 6] = [
        ConfigDuration::Grace,
        ConfigDuration::CatchUpWindow,
        ConfigDuration::DefaultTimeout,
        ConfigDuration::MaxTimeout,
        ConfigDuration::HookTimeout,
        ConfigDuration::ConsentExpiry,
    ];

    /// The key in `config.toml`.
    pub fn key(self) -> &'static str {
        match self {
            ConfigDuration::Grace => "grace",
            ConfigDuration::CatchUpWindow => "catch-up-window",
            ConfigDuration::DefaultTimeout => "default-timeout",
            ConfigDuration::MaxTimeout => "max-timeout",
            ConfigDuration::HookTimeout => "hook-timeout",
            ConfigDuration::ConsentExpiry => "consent-expiry",
        }
    }

    fn of(self, c: &Config) -> &str {
        match self {
            ConfigDuration::Grace => &c.grace,
            ConfigDuration::CatchUpWindow => &c.catch_up_window,
            ConfigDuration::DefaultTimeout => &c.default_timeout,
            ConfigDuration::MaxTimeout => &c.max_timeout,
            ConfigDuration::HookTimeout => &c.hook_timeout,
            ConfigDuration::ConsentExpiry => &c.consent_expiry,
        }
    }
}

/// A duration field in milliseconds. A hand-built config with a bad value gets the default's.
pub fn duration_ms(config: &Config, field: ConfigDuration) -> u64 {
    parse_duration(field.of(config), true)
        .or_else(|| parse_duration(field.of(&Config::default()), true))
        .unwrap_or(0)
}

fn refuse(msg: String) -> StateError {
    StateError::Config(format!("config.toml: {msg}"))
}

fn list(keys: &[&str]) -> String {
    keys.join(", ")
}

fn check_keys(table: &toml::Table, known: &[&str], prefix: &str) -> Result<()> {
    for k in table.keys() {
        if !known.contains(&k.as_str()) {
            return Err(refuse(format!(
                "unknown key \"{prefix}{k}\" — remove it, or use one of {}",
                list(known)
            )));
        }
    }
    Ok(())
}

fn check_cli(value: &toml::Value, path: &str) -> Result<()> {
    if let Some(s) = value.as_str() {
        s.parse::<CliId>()
            .map_err(|e| refuse(format!("{path}: {e}")))?;
    }
    Ok(())
}

/// Every key, at every level, is one fm knows.
fn check_all_keys(table: &toml::Table) -> Result<()> {
    check_keys(table, TOP, "")?;
    if let Some(v) = table.get("default-agent") {
        check_cli(v, "default-agent")?;
    }
    if let Some(toml::Value::Table(r)) = table.get("retention") {
        check_keys(r, RETENTION, "retention.")?;
    }
    if let Some(toml::Value::Table(agents)) = table.get("agents") {
        for (name, settings) in agents {
            name.parse::<CliId>()
                .map_err(|e| refuse(format!("agents.{name}: {e}")))?;
            if let toml::Value::Table(s) = settings {
                check_keys(s, AGENT, &format!("agents.{name}."))?;
            }
        }
    }
    if let Some(toml::Value::Table(autos)) = table.get("automations") {
        for (key, settings) in autos {
            if let toml::Value::Table(s) = settings {
                check_keys(s, AUTOMATION, &format!("automations.\"{key}\"."))?;
                if let Some(v) = s.get("agent") {
                    check_cli(v, &format!("automations.\"{key}\".agent"))?;
                }
            }
        }
    }
    Ok(())
}

/// Values that parse but that fm cannot use.
fn check_values(c: &Config) -> Result<()> {
    for f in ConfigDuration::ALL {
        if parse_duration(f.of(c), true).is_none() {
            return Err(refuse(format!(
                "{}: \"{}\" is not a duration — use a whole number and s, m, h or d, like {}",
                f.key(),
                f.of(c),
                f.of(&Config::default())
            )));
        }
    }
    let r = c.retention;
    for (key, n) in [
        ("max-lines-per-automation", r.max_lines_per_automation),
        ("keep-lines", r.keep_lines),
    ] {
        if n == 0 {
            return Err(refuse(format!(
                "retention.{key}: 0 is not a positive number — use 1 or more"
            )));
        }
    }
    if r.keep_lines > r.max_lines_per_automation {
        return Err(refuse(format!(
            "retention.keep-lines: {} is more than max-lines-per-automation ({}) — lower it",
            r.keep_lines, r.max_lines_per_automation
        )));
    }
    Ok(())
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Parse `config.toml` text. Refuses unknown keys, wrong types and values fm cannot use.
pub fn parse_config(text: &str) -> Result<Config> {
    let table: toml::Table = toml::from_str(text).map_err(|e| {
        let at = e
            .span()
            .map(|s| format!("line {}: ", line_of(text, s.start)))
            .unwrap_or_default();
        let msg = e.message().trim().replace('\n', " ");
        refuse(format!("{at}{msg} — fix the TOML syntax"))
    })?;
    check_all_keys(&table)?;
    let config: Config =
        serde_path_to_error::deserialize(toml::Value::Table(table)).map_err(|e| {
            let mut path = e.path().to_string();
            let mut msg = e.inner().to_string().trim().replace('\n', " ");
            // toml names the key itself, as ` in `a.b``, and spells map keys `?` cannot.
            if let Some(i) = msg.rfind(" in `").filter(|_| msg.ends_with('`')) {
                path = msg[i + 5..msg.len() - 1].to_string();
                msg.truncate(i);
            }
            refuse(format!(
                "{path}: {msg} — fix the value, or remove the key to use the default"
            ))
        })?;
    check_values(&config)?;
    Ok(config)
}

fn read_config_file(file: &Path) -> Result<Config> {
    match std::fs::read_to_string(file) {
        Ok(text) => parse_config(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(StateError::io(file, e)),
    }
}

/// Read `config.toml`: the defaults when there is none.
pub fn load_config(paths: &StatePaths) -> Result<Config> {
    read_config_file(&paths.config())
}

/// The `config.toml` text for a config: every key that has a value.
pub fn config_text(config: &Config) -> Result<String> {
    toml::to_string_pretty(config).map_err(|e| refuse(format!("{e} — this is a bug in fm")))
}

/// Write `config.toml` atomically, after checking fm would read it back.
pub fn save_config(paths: &StatePaths, config: &Config) -> Result<()> {
    check_values(config)?;
    let file = paths.config();
    let text = config_text(config)?;
    with_lock(&file, || write_atomic(&file, text.as_bytes()))
}

/// Read-change-write `config.toml` under its lock. A file fm refuses is not overwritten.
pub fn update_config<R>(paths: &StatePaths, change: impl FnOnce(&mut Config) -> R) -> Result<R> {
    let file = paths.config();
    with_lock(&file, || {
        let mut config = read_config_file(&file)?;
        let out = change(&mut config);
        check_values(&config)?;
        let text = config_text(&config)?;
        write_atomic(&file, text.as_bytes())?;
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_model::{AutomationSettings, CliSettings, Retention};
    use std::path::PathBuf;

    fn err(text: &str) -> String {
        parse_config(text).unwrap_err().to_string()
    }

    #[test]
    fn no_file_and_an_empty_file_are_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path());
        assert_eq!(load_config(&paths).unwrap(), Config::default());
        assert_eq!(parse_config("").unwrap(), Config::default());
    }

    #[test]
    fn a_partial_file_fills_the_rest_from_defaults() {
        let c = parse_config("grace = \"10m\"\nnotify-on-failed = false\n").unwrap();
        assert_eq!(c.grace, "10m");
        assert!(!c.notify_on_failed);
        assert_eq!(c.default_timeout, Config::default().default_timeout);
    }

    #[test]
    fn refuses_an_unknown_key_in_one_line() {
        let e = err("budget = 10\n");
        assert!(
            e.starts_with("config.toml: unknown key \"budget\" — "),
            "{e}"
        );
        assert!(!e.contains('\n'));
        let e = err("[retention]\nnonsense = 1\n");
        assert!(
            e.starts_with("config.toml: unknown key \"retention.nonsense\" — "),
            "{e}"
        );
        let e = err("[agents.claude]\nflags = []\n");
        assert!(
            e.starts_with("config.toml: unknown key \"agents.claude.flags\" — "),
            "{e}"
        );
        let e = err("[automations.\"user:/r:x\"]\nmodel = \"a/b\"\n");
        assert!(
            e.contains("unknown key \"automations.\"user:/r:x\".model\""),
            "{e}"
        );
    }

    #[test]
    fn refuses_an_unknown_agent_cli() {
        let e = err("default-agent = \"gpt\"\n");
        assert!(
            e.starts_with("config.toml: default-agent: unknown agent CLI \"gpt\" — "),
            "{e}"
        );
        let e = err("[agents.vim]\n");
        assert!(
            e.starts_with("config.toml: agents.vim: unknown agent CLI"),
            "{e}"
        );
    }

    #[test]
    fn refuses_a_wrong_type_naming_the_key() {
        let e = err("stagger-max-seconds = \"lots\"\n");
        assert!(e.starts_with("config.toml: stagger-max-seconds: "), "{e}");
        assert!(e.contains(" — ") && !e.contains('\n'), "{e}");
        let e = err("[retention]\nkeep-lines = -1\n");
        assert!(e.starts_with("config.toml: retention.keep-lines: "), "{e}");
        let e = err("[agents.claude]\nallow-all-tools = \"yes\"\n");
        assert_eq!(
            e,
            "config.toml: agents.claude.allow-all-tools: invalid type: string \"yes\", expected a boolean — fix the value, or remove the key to use the default"
        );
    }

    #[test]
    fn refuses_a_duration_that_is_not_one() {
        assert_eq!(
            err("grace = \"ten minutes\"\n"),
            "config.toml: grace: \"ten minutes\" is not a duration — use a whole number and s, m, h or d, like 5m"
        );
    }

    #[test]
    fn allows_days_in_durations() {
        let c = parse_config("catch-up-window = \"14d\"\nconsent-expiry = \"60d\"\n").unwrap();
        assert_eq!(
            duration_ms(&c, ConfigDuration::CatchUpWindow),
            14 * 86_400_000
        );
        assert_eq!(
            duration_ms(&c, ConfigDuration::ConsentExpiry),
            60 * 86_400_000
        );
    }

    #[test]
    fn accepts_a_zero_stagger() {
        assert_eq!(
            parse_config("stagger-max-seconds = 0\n")
                .unwrap()
                .stagger_max_seconds,
            0
        );
    }

    #[test]
    fn refuses_incoherent_retention() {
        let e = err("[retention]\nmax-lines-per-automation = 100\nkeep-lines = 900\n");
        assert!(
            e.contains("is more than max-lines-per-automation (100)"),
            "{e}"
        );
        let e = err("[retention]\nkeep-lines = 0\n");
        assert!(
            e.contains("retention.keep-lines: 0 is not a positive number"),
            "{e}"
        );
    }

    #[test]
    fn refuses_broken_toml_with_a_line_number() {
        let e = err("grace = \"5m\"\n[[[\n");
        assert!(e.starts_with("config.toml: line 2: "), "{e}");
        assert!(
            e.ends_with("— fix the TOML syntax") && !e.contains('\n'),
            "{e}"
        );
    }

    #[test]
    fn duration_ms_converts_and_falls_back() {
        let d = Config::default();
        assert_eq!(duration_ms(&d, ConfigDuration::Grace), 300_000);
        assert_eq!(duration_ms(&d, ConfigDuration::MaxTimeout), 14_400_000);
        assert_eq!(
            duration_ms(&d, ConfigDuration::ConsentExpiry),
            2_592_000_000
        );
        let bad = Config {
            grace: "nonsense".into(),
            ..Config::default()
        };
        assert_eq!(duration_ms(&bad, ConfigDuration::Grace), 300_000);
    }

    fn full() -> Config {
        let mut c = Config {
            default_agent: Some(CliId::Claude),
            grace: "10m".into(),
            catch_up_window: "14d".into(),
            hook_max_output_bytes: 1024,
            retention: Retention {
                max_lines_per_automation: 50,
                keep_lines: 20,
            },
            notify_on_failed: false,
            ..Config::default()
        };
        c.agents.insert(
            CliId::Copilot,
            CliSettings {
                path: Some(PathBuf::from("copilot")),
                extra_args: vec!["--x".into()],
                allow_all_tools: true,
                default_model: Some("openai/gpt-5".into()),
            },
        );
        c.automations.insert(
            "repository:C:\\r\\.agents\\automations:x".into(),
            AutomationSettings {
                agent: Some(CliId::Codex),
                worktree: true,
            },
        );
        c
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path());
        save_config(&paths, &full()).unwrap();
        assert_eq!(load_config(&paths).unwrap(), full());
        save_config(&paths, &Config::default()).unwrap();
        assert_eq!(load_config(&paths).unwrap(), Config::default());
    }

    #[test]
    fn update_config_changes_one_thing_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path());
        save_config(&paths, &full()).unwrap();
        update_config(&paths, |c| c.grace = "1m".into()).unwrap();
        let after = load_config(&paths).unwrap();
        assert_eq!(after.grace, "1m");
        assert_eq!(after.agents, full().agents);
    }

    #[test]
    fn update_config_does_not_overwrite_a_refused_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StatePaths::new(dir.path());
        std::fs::write(paths.config(), "budget = 1\n").unwrap();
        assert!(update_config(&paths, |c| c.grace = "1m".into()).is_err());
        assert_eq!(
            std::fs::read_to_string(paths.config()).unwrap(),
            "budget = 1\n"
        );
    }

    #[test]
    fn the_known_keys_are_exactly_what_the_model_writes() {
        // A full config written out uses every known key, and nothing else, at every level.
        let text = config_text(&full()).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        let mut top: Vec<_> = table.keys().map(String::as_str).collect();
        top.sort_unstable();
        let mut want = TOP.to_vec();
        want.sort_unstable();
        assert_eq!(top, want);
        let agent = table["agents"]["copilot"].as_table().unwrap();
        assert_eq!(agent.len(), AGENT.len());
        let auto = table["automations"]["repository:C:\\r\\.agents\\automations:x"]
            .as_table()
            .unwrap();
        assert_eq!(auto.len(), AUTOMATION.len());
        assert_eq!(
            table["retention"].as_table().unwrap().len(),
            RETENTION.len()
        );
    }
}

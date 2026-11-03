//! The real [`Host`]: discovery, consent and state on disk, agent CLIs on this machine.
//!
//! The state of an automation is decided in pi-automations' order (`src/status.ts`): invalid, then
//! disabled, then off, then armed. The machine checks (interpreters, timeout maximum, the agent's
//! model) run only for an automation that would otherwise be armed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use fm_agents::{adapter, detect_with, find_in_path};
use fm_model::{
    ActionResult, AutomationSettings, AutomationState, CatchUp, CliId, CliInstall, CliSettings,
    Config, ConsentPreview, Discovered, Hook, Host, LastRun, LiveEvent, LiveRun, Located,
    RunLockInfo, RunProgress, RunRecord, Scope, StateFile, StatePaths, Status, TurnRequest,
};
use fm_spec::{
    CronSchedule, DiscoverContext, Machine, Platform, Refusal, by_key, check_machine,
    describe_next, digest_of, discover, file_hashes_of, interpreter_map, parse_duration,
    read_chart, repository_roots, roots_for_registered, set_enabled_in_place, user_root, which_in,
};
use fm_state::keys::name_of;
use fm_state::time::parse_iso;
use fm_state::{
    ArmShown, ConfigDuration, Consent, OffReason, PaTrust, Pin, StateStore, StopOutcome,
    SystemProbe, duration_ms, is_armed_in_pa, is_run_lock_stale, last_record, load_config,
    progress_of, read_progress, read_records, read_run_lock, stop_run, update_config,
};

use crate::env::HostEnv;
use crate::fm;
use crate::proc::spawn_detached;

/// How long detection waits for `<cli> --version`.
const DETECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The live view keeps this many events.
const LIVE_EVENTS: usize = 500;

/// One CLI and whether it was found here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFound {
    pub cli: CliId,
    pub found: Result<CliInstall, String>,
}

pub struct RealHost {
    env: HostEnv,
    found: Vec<Discovered>,
    agents: Option<Vec<AgentFound>>,
    fixed_now: Option<DateTime<Utc>>,
}

/// Everything one listing reads once.
struct Ctx<'a> {
    paths: &'a StatePaths,
    config: &'a Config,
    consent: Option<&'a Consent>,
    state: &'a StateFile,
    pa: &'a BTreeSet<String>,
    machine: &'a Machine<'a>,
    now: DateTime<Utc>,
}

/// The agent's part of the machine check: the model resolves through the chosen CLI, and prompt
/// hooks can continue its session.
pub fn agent_check(config: &Config, l: &Located) -> Result<(), String> {
    // A step handed to an agent fm cannot run, or a model that agent cannot serve, cannot run here.
    fm_run::node::plan_nodes(config, l)?;
    let Some(cli) = config.agent_for(&l.key) else {
        return Ok(());
    };
    let a = adapter(cli);
    a.resolve_model(l.automation.model.as_ref(), &config.agent_settings(cli))
        .map_err(|r| r.message)?;
    if !a.supports_resume() {
        for (phase, hooks) in [
            ("pre", &l.automation.hooks.pre),
            ("post", &l.automation.hooks.post),
        ] {
            if let Some(i) = hooks.iter().position(|h| matches!(h, Hook::Prompt(_))) {
                return Err(format!(
                    "hooks.{phase}[{i}].prompt: {cli} cannot continue a session — use a script hook, or another agent"
                ));
            }
        }
    }
    Ok(())
}

/// What consent is pinned to now. A file that cannot be read pins as missing.
pub fn pin_of(l: &Located) -> Pin {
    match digest_of(l) {
        Ok(d) => Pin {
            digest: d.digest,
            files: d.files,
        },
        Err(_) => Pin {
            digest: String::new(),
            files: file_hashes_of(l),
        },
    }
}

/// Seconds a scheduled fire of `key` is delayed.
pub fn stagger_of(key: &str, cron: &CronSchedule, config: &Config, now: DateTime<Utc>) -> u64 {
    fm_clock::stagger_for(key, cron, config.stagger_max_seconds, now)
}

fn running_of(paths: &StatePaths, key: &str, now: DateTime<Utc>) -> Option<RunLockInfo> {
    read_run_lock(paths, key)
        .filter(|l| !is_run_lock_stale(l, &SystemProbe, now))
        .map(|l| l.info)
}

fn last_of(paths: &StatePaths, key: &str) -> Option<LastRun> {
    let rec = last_record(paths, key)?;
    let duration_ms = match (
        rec.started_at.as_deref().and_then(parse_iso),
        rec.ended_at.as_deref().and_then(parse_iso),
    ) {
        (Some(a), Some(b)) => u64::try_from((b - a).num_milliseconds()).ok(),
        _ => None,
    };
    Some(LastRun {
        outcome: rec.outcome,
        at: rec.ended_at.clone().unwrap_or(rec.recorded_at.clone()),
        reason: rec.reason.clone().or_else(|| {
            rec.error.as_ref().and_then(|e| {
                serde_json::to_value(e.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
            })
        }),
        duration_ms,
    })
}

fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

fn off_reason(
    consent: Option<&Consent>,
    l: &Located,
    pin: &Pin,
    now: DateTime<Utc>,
) -> Option<OffReason> {
    match consent {
        Some(c) => c.off_reason(l, pin, now),
        None => Some(OffReason::NotArmed),
    }
}

fn status_of(d: &Discovered, c: &Ctx<'_>) -> Status {
    let default_timeout = duration_ms(c.config, ConfigDuration::DefaultTimeout);
    let key = d.key().to_string();
    let base = |name: String, scope: Scope, dir: &Path| Status {
        key: key.clone(),
        name,
        scope,
        dir: dir.to_path_buf(),
        state: AutomationState::Invalid,
        reason: None,
        problems: Vec::new(),
        automation: None,
        agent: c.config.agent_for(&key).map(|a| a.as_str().to_string()),
        effective_timeout_ms: default_timeout,
        next_fire: None,
        next_check: None,
        watch: None,
        watch_paused_at: None,
        running: running_of(c.paths, &key, c.now),
        flow: false,
        last: last_of(c.paths, &key),
        also_armed_in_pa: false,
    };
    let l = match d {
        Discovered::Invalid {
            dir,
            root,
            error,
            problems,
            ..
        } => {
            let mut s = base(dir_name(dir), root.scope, dir);
            s.reason = Some(error.clone());
            s.problems = problems.clone();
            return s;
        }
        Discovered::Valid(l) => l,
    };
    let a = &l.automation;
    let mut s = base(a.name.clone(), l.root.scope, &l.dir);
    s.automation = Some(a.clone());
    s.effective_timeout_ms = a
        .timeout
        .as_deref()
        .and_then(parse_duration)
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(default_timeout);
    s.flow = read_chart(&l.dir).is_some();
    if a.on.watch.is_some() {
        s.watch = c.state.watches.get(&key).cloned();
        s.watch_paused_at = c.state.paused_watches.get(&key).cloned();
    }

    if !a.enabled {
        s.state = AutomationState::Disabled;
        return s;
    }
    if let Some(off) = off_reason(c.consent, l, &pin_of(l), c.now) {
        s.state = AutomationState::Off;
        s.reason = Some(off.message(&a.name));
        return s;
    }
    if let Err(r) = check_machine(l, c.machine) {
        s.state = AutomationState::Invalid;
        s.reason = Some(r.to_string());
        s.problems = r.problems;
        return s;
    }
    s.state = AutomationState::Armed;
    s.also_armed_in_pa = is_armed_in_pa(&key, c.pa);
    if let Some(schedule) = &a.on.schedule {
        s.next_fire = fm_clock::next_fire(schedule, &key, c.config.stagger_max_seconds, c.now);
    }
    if let Some(w) = &a.on.watch {
        s.next_check =
            fm_clock::next_check(w, s.watch.as_ref(), s.watch_paused_at.is_some(), c.now);
    }
    s
}

fn first_line(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

/// `30 days`, `12h`: how far off consent expires.
fn span_words(ms: u64) -> String {
    const DAY: u64 = 86_400_000;
    if ms >= DAY && ms.is_multiple_of(DAY) {
        let d = ms / DAY;
        format!("{d} day{}", if d == 1 { "" } else { "s" })
    } else {
        fm_spec::format_duration(Duration::from_millis(ms))
    }
}

/// The program a CLI is started as: `[agents.<cli>] path`, else PATH, else its bare name.
pub fn program_for(cli: CliId, settings: &CliSettings, env: &HostEnv) -> (PathBuf, bool) {
    if let Some(p) = &settings.path {
        return (p.clone(), p.is_file());
    }
    let name = adapter(cli).program_name();
    match find_in_path(
        name,
        &env.path_var,
        env.pathext.as_deref().map(std::ffi::OsStr::new),
    ) {
        Some(p) => (p, true),
        None => (PathBuf::from(name), false),
    }
}

impl RealHost {
    pub fn new(env: HostEnv) -> RealHost {
        RealHost {
            env,
            found: Vec::new(),
            agents: None,
            fixed_now: None,
        }
    }

    pub fn from_process() -> anyhow::Result<RealHost> {
        Ok(RealHost::new(HostEnv::from_process()?))
    }

    /// Freeze the clock, for tests.
    pub fn with_now(mut self, now: DateTime<Utc>) -> RealHost {
        self.fixed_now = Some(now);
        self
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.fixed_now.unwrap_or_else(Utc::now)
    }

    pub fn env(&self) -> &HostEnv {
        &self.env
    }

    pub fn paths(&self) -> &StatePaths {
        &self.env.state
    }

    /// `config.toml`, refused whole in one line when fm cannot use it.
    pub fn config(&self) -> Result<Config, String> {
        load_config(&self.env.state).map_err(|e| e.to_string())
    }

    fn which(&self) -> impl Fn(&str) -> Option<PathBuf> + use<> {
        let path_var = self.env.path_var.clone();
        let pathext = self.env.pathext.clone();
        move |bin: &str| which_in(bin, &path_var, pathext.as_deref(), Platform::current())
    }

    /// pa's armed keys now; empty when pa is not installed.
    pub fn pa_keys(&self) -> BTreeSet<String> {
        self.env
            .pa_trust
            .as_deref()
            .map(|p| PaTrust::load(p).armed_keys(self.now()))
            .unwrap_or_default()
    }

    /// The user root, repository roots from the working directory up, and repository roots
    /// registered in state when something there was armed.
    pub fn roots(&self) -> Vec<fm_model::Root> {
        let mut roots = vec![user_root(&self.env.home)];
        roots.extend(repository_roots(&self.env.cwd));
        for r in StateStore::new(&self.env.state).load().roots {
            for root in roots_for_registered(&r) {
                if !roots
                    .iter()
                    .any(|x| fm_state::same_path(&x.path, &root.path))
                {
                    roots.push(root);
                }
            }
        }
        roots
    }

    /// Re-discover. Reads only.
    pub fn rediscover(&mut self) -> &[Discovered] {
        self.found = discover(&self.roots(), &DiscoverContext::default());
        &self.found
    }

    fn discovered(&mut self, key: &str) -> Result<Discovered, String> {
        if by_key(&self.found, key).is_none() {
            self.rediscover();
        }
        by_key(&self.found, key)
            .cloned()
            .ok_or_else(|| format!("{} is gone — refresh the list", name_of(key)))
    }

    fn located(&mut self, key: &str) -> Result<Located, String> {
        match self.discovered(key)? {
            Discovered::Valid(l) => Ok(*l),
            Discovered::Invalid { dir, error, .. } => Err(format!("{}: {error}", dir_name(&dir))),
        }
    }

    /// Every automation with its state, re-discovered.
    pub fn statuses(&mut self) -> Vec<Status> {
        let now = self.now();
        let config = self.config().unwrap_or_default();
        self.rediscover();
        let mut consent = Consent::load(&self.env.state).ok();
        let state = StateStore::new(&self.env.state).load();
        let pa = self.pa_keys();
        let which = self.which();
        let cfg = config.clone();
        let check = move |l: &Located| agent_check(&cfg, l);
        let machine = Machine {
            platform: Platform::current(),
            which: &which,
            max_timeout: Some(Duration::from_millis(duration_ms(
                &config,
                ConfigDuration::MaxTimeout,
            ))),
            agent_check: Some(&check),
        };
        let ctx = Ctx {
            paths: &self.env.state,
            config: &config,
            consent: consent.as_ref(),
            state: &state,
            pa: &pa,
            machine: &machine,
            now,
        };
        let out: Vec<Status> = self.found.iter().map(|d| status_of(d, &ctx)).collect();
        if let Some(c) = consent.as_mut() {
            // Each sighting keeps a consent from being swept while its folder is unplugged.
            for d in &self.found {
                if let Discovered::Valid(l) = d {
                    c.touch(&l.key, now);
                }
            }
            let _ = c.save();
        }
        out
    }

    pub fn status(&mut self, key: &str) -> Option<Status> {
        self.statuses().into_iter().find(|s| s.key == key)
    }

    /// Why a manual run cannot start, or `None` when it can.
    pub fn run_refusal(s: &Status) -> Option<String> {
        let name = &s.name;
        if s.running.is_some() {
            return Some(format!(
                "{name} is already running — fm stop {name} stops it"
            ));
        }
        match s.state {
            AutomationState::Invalid => {
                return Some(format!(
                    "{name}: {}",
                    s.reason.as_deref().unwrap_or("invalid")
                ));
            }
            AutomationState::Disabled => {
                return Some(format!(
                    "{name}: enabled: false — set it true to run this automation"
                ));
            }
            _ => {}
        }
        if !s.automation.as_ref().is_some_and(|a| a.on.manual) {
            return Some(format!(
                "{name}: has no manual trigger — add \"manual: true\" under on:"
            ));
        }
        if s.state == AutomationState::Off {
            return Some(format!(
                "{name}: {}",
                s.reason.as_deref().unwrap_or("not armed")
            ));
        }
        None
    }

    /// The `fm fire` command for a manual run of `key`, in this host's state folder.
    pub fn fire_command(&self, key: &str) -> Command {
        let mut cmd = Command::new(&self.env.exe);
        cmd.args(["fire", "--key", key, "--trigger", "manual"])
            .env("FM_HOME", &self.env.state.root)
            .current_dir(&self.env.cwd);
        cmd
    }

    /// Detect every CLI, in parallel, once per host.
    pub fn detect_all(&mut self) -> Vec<AgentFound> {
        if let Some(a) = &self.agents {
            return a.clone();
        }
        let config = self.config().unwrap_or_default();
        let env = &self.env;
        let found: Vec<AgentFound> = std::thread::scope(|scope| {
            let handles: Vec<_> = CliId::ALL
                .into_iter()
                .map(|cli| {
                    let settings = config.agent_settings(cli);
                    scope.spawn(move || AgentFound {
                        cli,
                        found: detect_with(
                            cli,
                            &settings,
                            &env.path_var,
                            env.pathext.as_deref().map(std::ffi::OsStr::new),
                            DETECT_TIMEOUT,
                        ),
                    })
                })
                .collect();
            handles
                .into_iter()
                .zip(CliId::ALL)
                .map(|(h, cli)| {
                    h.join().unwrap_or(AgentFound {
                        cli,
                        found: Err("detection failed".into()),
                    })
                })
                .collect()
        });
        self.agents = Some(found.clone());
        found
    }

    /// The consent preview for a valid automation.
    pub fn preview_of(&self, l: &Located, config: &Config) -> ConsentPreview {
        let now = self.now();
        let a = &l.automation;
        let which = self.which();
        let commands = interpreter_map(l, Platform::current(), &which);
        let shown = |script: &str| {
            commands
                .get(script)
                .cloned()
                .unwrap_or_else(|| script.to_string())
        };
        let mut warnings = Vec::new();

        if is_armed_in_pa(&l.key, &self.pa_keys()) {
            warnings.push(format!(
                "also armed in pi-automations — it would run twice; disarm it there with `pa disarm {}`",
                a.name
            ));
        }

        let mut triggers = Vec::new();
        if let Some(sc) = &a.on.schedule {
            let tz = sc
                .timezone
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            let catch = match sc.catch_up {
                CatchUp::Once => "catch-up once",
                CatchUp::Skip => "no catch-up",
            };
            let next = CronSchedule::from_schedule(sc)
                .map(|c| {
                    format!(
                        " — next {}",
                        describe_next(&c, now, stagger_of(&l.key, &c, config, now))
                    )
                })
                .unwrap_or_default();
            triggers.push(format!("schedule: {}{tz}, {catch}{next}", sc.cron));
        }
        if let Some(w) = &a.on.watch {
            triggers.push(format!("watch: {} every {}", w.script, w.every));
        }
        if a.on.manual {
            triggers.push("manual: allowed".into());
        }

        let spec = a.model.as_ref().map(|m| m.display());
        let agent = config.agent_for(&l.key);
        let mut model = spec.clone().unwrap_or_else(|| "the agent's default".into());
        let mut agent_command = None;
        match agent {
            None => warnings.push("no agent CLI chosen — `fm agent default <cli>`".into()),
            Some(cli) => {
                let ad = adapter(cli);
                let settings = config.agent_settings(cli);
                let (program, exists) = program_for(cli, &settings, &self.env);
                if !exists {
                    warnings.push(format!(
                        "{cli} not found — install it, or set [agents.{cli}] path in config.toml"
                    ));
                }
                match ad.resolve_model(a.model.as_ref(), &settings) {
                    Ok(m) => {
                        model = m.display.clone();
                        let turn = TurnRequest {
                            prompt: a.body.clone(),
                            model: m,
                            cwd: l.repo_root.clone().unwrap_or_else(|| l.dir.clone()),
                            resume: None,
                            settings,
                        };
                        agent_command = Some(ad.plan_turn(program, &turn).display(&a.body));
                    }
                    Err(r) => warnings.push(r.message),
                }
            }
        }

        let which_ref: &dyn Fn(&str) -> Option<PathBuf> = &which;
        let machine = Machine {
            platform: Platform::current(),
            which: which_ref,
            max_timeout: Some(Duration::from_millis(duration_ms(
                config,
                ConfigDuration::MaxTimeout,
            ))),
            agent_check: None,
        };
        if let Err(r) = check_machine(l, &machine) {
            warnings.push(Refusal::to_string(&r));
        }
        if let Ok(consent) = Consent::load(&self.env.state)
            && let Some(OffReason::Changed(files)) = consent.off_reason(l, &pin_of(l), now)
        {
            warnings.push(format!(
                "changed since armed: {} — look at what changed before arming again",
                files.join(", ")
            ));
        }

        let mut nodes = Vec::new();
        for n in &a.nodes {
            match fm_run::node::plan_node(config, l, n) {
                Ok(p) => {
                    let (program, exists) = program_for(p.cli, &p.settings, &self.env);
                    if !exists {
                        warnings.push(format!(
                            "nodes.{}: {} not found — install it, or set [agents.{}] path in config.toml",
                            n.id, p.cli, p.cli
                        ));
                    }
                    let shown = format!("<the {} step>", n.id);
                    let turn = TurnRequest {
                        prompt: shown.clone(),
                        model: p.model.clone(),
                        cwd: l.repo_root.clone().unwrap_or_else(|| l.dir.clone()),
                        resume: None,
                        settings: p.settings.clone(),
                    };
                    let command = adapter(p.cli).plan_turn(program, &turn).display(&shown);
                    let bound = n
                        .timeout
                        .as_deref()
                        .map(|t| format!(", at most {t}"))
                        .unwrap_or_default();
                    nodes.push((
                        n.id.clone(),
                        format!("{} ({}{bound}) — {command}", p.cli, p.model.display),
                    ));
                }
                Err(e) => warnings.push(e),
            }
        }

        let mut hooks = Vec::new();
        for (phase, list) in [("pre", &a.hooks.pre), ("post", &a.hooks.post)] {
            for (i, h) in list.iter().enumerate() {
                let what = match h {
                    Hook::Script(p) => format!("script: {}", shown(p)),
                    Hook::Prompt(p) => format!("prompt: \"{}\"", first_line(p, 60)),
                };
                hooks.push((format!("{phase} {i}"), what));
            }
        }

        let expires = (l.root.scope == Scope::Repository)
            .then(|| duration_ms(config, ConfigDuration::ConsentExpiry))
            .filter(|ms| *ms > 0)
            .map(|ms| {
                let at = now + chrono::Duration::milliseconds(i64::try_from(ms).unwrap_or(0));
                format!(
                    "in {}, on {}",
                    span_words(ms),
                    at.with_timezone(&Local).format("%b %-d")
                )
            });

        let timeout = match &a.timeout {
            Some(t) => t.clone(),
            None => format!(
                "{} (host default)",
                fm_spec::format_duration(Duration::from_millis(duration_ms(
                    config,
                    ConfigDuration::DefaultTimeout
                )))
            ),
        };

        ConsentPreview {
            key: l.key.clone(),
            name: a.name.clone(),
            triggers,
            model,
            timeout,
            agent,
            agent_command,
            hooks,
            nodes,
            watch: a
                .on
                .watch
                .as_ref()
                .map(|w| format!("{} every {}", shown(&w.script), w.every)),
            files: file_hashes_of(l).into_keys().collect(),
            warnings,
            expires,
        }
    }

    /// Set or clear the CLI that runs every automation without an override.
    pub fn set_default_agent(&mut self, agent: Option<CliId>) -> ActionResult {
        update_config(&self.env.state, |c| c.default_agent = agent).map_err(|e| e.to_string())?;
        Ok(match agent {
            Some(cli) => format!("default agent is {cli} ({})", cli.title()),
            None => "no default agent — each automation needs fm agent set <name> <cli>".into(),
        })
    }
}

impl Host for RealHost {
    fn list(&mut self) -> Vec<Status> {
        self.statuses()
    }

    fn runs(&mut self, key: &str, limit: usize) -> Vec<RunRecord> {
        let mut recs = read_records(&self.env.state, key, limit);
        recs.reverse();
        recs
    }

    fn live(&mut self, key: &str) -> Option<LiveRun> {
        let lock = running_of(&self.env.state, key, self.now())?;
        let text = std::fs::read_to_string(self.env.state.live_file(key)).unwrap_or_default();
        let mut events: Vec<LiveEvent> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if events.len() > LIVE_EVENTS {
            events.drain(..events.len() - LIVE_EVENTS);
        }
        let progress = read_progress(&self.env.state, &lock.run_id).map(|s| progress_of(&s));
        Some(LiveRun {
            run_id: lock.run_id,
            events,
            progress,
        })
    }

    fn preview(&mut self, key: &str) -> Result<ConsentPreview, String> {
        let config = self.config()?;
        let l = self.located(key)?;
        Ok(self.preview_of(&l, &config))
    }

    fn arm(&mut self, key: &str) -> ActionResult {
        let now = self.now();
        let config = self.config()?;
        let l = self.located(key)?;
        let name = l.automation.name.clone();
        if !l.automation.enabled {
            return Err(format!(
                "{name}: enabled: false in the file — it is inert on every host"
            ));
        }
        let which = self.which();
        let cfg = config.clone();
        let check = move |l: &Located| agent_check(&cfg, l);
        let machine = Machine {
            platform: Platform::current(),
            which: &which,
            max_timeout: Some(Duration::from_millis(duration_ms(
                &config,
                ConfigDuration::MaxTimeout,
            ))),
            agent_check: Some(&check),
        };
        let report = check_machine(&l, &machine).map_err(|r| format!("{name}: {r}"))?;
        let digest = digest_of(&l).map_err(|r| format!("{name}: {r}"))?;
        let pin = Pin {
            digest: digest.digest,
            files: digest.files,
        };
        let shown = ArmShown {
            interpreters: report.commands(),
            agent_command: self.preview_of(&l, &config).agent_command,
        };
        let mut consent = Consent::load(&self.env.state).map_err(|e| e.to_string())?;
        consent
            .arm(
                &l,
                &pin,
                &shown,
                now,
                duration_ms(&config, ConfigDuration::ConsentExpiry),
            )
            .map_err(|e| e.to_string())?;
        if l.root.scope == Scope::Repository
            && let Some(root) = &l.repo_root
        {
            StateStore::new(&self.env.state)
                .register_root(root)
                .map_err(|e| e.to_string())?;
        }
        let next = l
            .automation
            .on
            .schedule
            .as_ref()
            .and_then(|s| CronSchedule::from_schedule(s).ok())
            .map(|c| {
                format!(
                    " · next {}",
                    describe_next(&c, now, stagger_of(key, &c, &config, now))
                )
            })
            .unwrap_or_default();
        Ok(format!("{name} armed{next}"))
    }

    fn disarm(&mut self, key: &str) -> ActionResult {
        let name = name_of(key).to_string();
        let mut consent = Consent::load(&self.env.state).map_err(|e| e.to_string())?;
        if consent.disarm(key).map_err(|e| e.to_string())? {
            Ok(format!(
                "{name} disarmed — it stays listed, and will not run"
            ))
        } else {
            Ok(format!("{name} was not armed"))
        }
    }

    fn set_enabled(&mut self, key: &str, enabled: bool) -> ActionResult {
        let now = self.now();
        let d = self.discovered(key)?;
        let (file, scope, name) = match &d {
            Discovered::Valid(l) => (l.file.clone(), l.root.scope, l.automation.name.clone()),
            Discovered::Invalid {
                file, root, dir, ..
            } => (file.clone(), root.scope, dir_name(dir)),
        };
        if scope == Scope::Repository {
            // Writing into a repository would leave an uncommitted change in a shared file.
            return Err(
                "enabled is set in the file — edit and commit it; fm disarm keeps it off on this machine"
                    .into(),
            );
        }
        let mut consent = Consent::load(&self.env.state).map_err(|e| e.to_string())?;
        let was_armed = match &d {
            Discovered::Valid(l) => consent.off_reason(l, &pin_of(l), now).is_none(),
            Discovered::Invalid { .. } => false,
        };
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("{name}: cannot read AUTOMATION.md — {e}"))?;
        let next = set_enabled_in_place(&text, enabled);
        let word = if enabled { "enabled" } else { "disabled" };
        if next == text {
            return Ok(format!("{name} is already {word}"));
        }
        std::fs::write(&file, &next)
            .map_err(|e| format!("{name}: cannot write AUTOMATION.md — {e}"))?;
        if was_armed {
            // Consent that held before this one-line edit still holds after it.
            self.rediscover();
            let repinned = match by_key(&self.found, key) {
                Some(Discovered::Valid(l)) => consent.repin(key, &pin_of(l)).unwrap_or(false),
                _ => false,
            };
            if !repinned {
                let _ = consent.disarm(key);
            }
        }
        if enabled && !was_armed {
            return Ok(format!("{name} enabled — not armed yet: fm arm {name}"));
        }
        Ok(format!("{name} {word}"))
    }

    fn run_now(&mut self, key: &str) -> ActionResult {
        let s = self
            .status(key)
            .ok_or_else(|| format!("{} is gone — refresh the list", name_of(key)))?;
        if let Some(why) = RealHost::run_refusal(&s) {
            return Err(why);
        }
        spawn_detached(self.fire_command(key))
            .map_err(|e| format!("{}: could not start fm fire — {e}", s.name))?;
        Ok(format!("{} started", s.name))
    }

    fn stop(&mut self, key: &str) -> ActionResult {
        let name = name_of(key).to_string();
        let Some(lock) = read_run_lock(&self.env.state, key) else {
            return Err(format!("{name} is not running"));
        };
        match stop_run(
            &self.env.state,
            key,
            &lock.info.run_id,
            &SystemProbe,
            self.now(),
        ) {
            Ok(StopOutcome::Requested) => Ok(format!(
                "{name}: stop requested — its post hooks run, then it records aborted"
            )),
            Ok(StopOutcome::NotRunning) => Err(format!("{name} is not running")),
            Ok(StopOutcome::OtherRun(id)) => Err(format!(
                "{name}: another run ({id}) is going now — refresh before stopping it"
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    fn pause_watch(&mut self, key: &str, paused: bool) -> ActionResult {
        let l = self.located(key)?;
        let name = &l.automation.name;
        let verb = if paused { "pause" } else { "resume" };
        if l.automation.on.watch.is_none() {
            return Err(format!("{name}: has no watch to {verb}"));
        }
        let store = StateStore::new(&self.env.state);
        let changed = if paused {
            store.pause_watch(key, self.now())
        } else {
            store.resume_watch(key)
        }
        .map_err(|e| e.to_string())?;
        Ok(match (paused, changed) {
            (true, true) => format!("{name}: watch paused — no more checks until you resume it"),
            (true, false) => format!("{name}: its watch was already paused"),
            (false, true) => format!("{name}: watch resumed — it checks at the next tick"),
            (false, false) => format!("{name}: its watch was not paused"),
        })
    }

    fn set_agent(&mut self, key: &str, agent: Option<CliId>) -> ActionResult {
        let name = name_of(key).to_string();
        let config = update_config(&self.env.state, |c| {
            match agent {
                Some(cli) => {
                    c.automations.entry(key.to_string()).or_default().agent = Some(cli);
                }
                None => {
                    if let Some(entry) = c.automations.get_mut(key) {
                        entry.agent = None;
                        if *entry == AutomationSettings::default() {
                            c.automations.remove(key);
                        }
                    }
                }
            }
            c.clone()
        })
        .map_err(|e| e.to_string())?;
        Ok(match (agent, config.default_agent) {
            (Some(cli), _) => format!("{name} runs through {cli}"),
            (None, Some(d)) => format!("{name} uses the default agent, {d}"),
            (None, None) => format!(
                "{name} has no agent — fm agent default <cli>, or fm agent set {name} <cli>"
            ),
        })
    }

    fn agents(&mut self) -> Vec<(CliId, Option<String>)> {
        self.detect_all()
            .into_iter()
            .filter_map(|a| a.found.ok().map(|i| (a.cli, i.version)))
            .collect()
    }

    fn edit_path(&mut self, key: &str) -> Option<PathBuf> {
        match self.discovered(key).ok()? {
            Discovered::Valid(l) => Some(l.file),
            Discovered::Invalid { file, .. } => Some(file),
        }
    }

    fn render_chart(
        &mut self,
        chart: &str,
        progress: Option<&RunProgress>,
        width: u16,
    ) -> Result<String, String> {
        let text = fm::render(chart, &fm::ChartState::of(progress))?;
        let need = fm::width_of(&text);
        if need > usize::from(width) {
            return Err(format!(
                "the flowchart is {need} columns wide and the pane is {width}"
            ));
        }
        Ok(text)
    }

    fn watcher_status(&mut self) -> String {
        crate::seams::watcher_summary(&self.env, self.now())
    }
}

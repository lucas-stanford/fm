//! One tick: take the tick lock, sweep abandoned runs, discover, decide each automation's state,
//! fire due schedules (claim first, then start), run due watch checks, and note the tick.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use fm_model::{
    AutomationState, ErrorKind, Located, Outcome, Root, RunError, RunRecord, StatePaths,
    TickSource, Trigger,
};
use fm_spec::{
    AgentCheck, CronSchedule, DiscoverContext, Machine, Platform, Which, discover,
    roots_for_registered, user_root,
};
use fm_state::time::{iso, parse_iso};
use fm_state::{
    ConfigDuration, Consent, ProcessProbe, StateError, StateFileExt, StateStore, SystemProbe,
    acquire_tick_lock, append_record, duration_ms, load_config, new_run_id, read_run_lock,
    sweep_stale_run_locks,
};

use crate::assess::{Assessed, assess};
use crate::decide::{Action, ScheduleInput, decide_one};
use crate::next::stagger_for;
use crate::start::StartRun;
use crate::watch::{
    RunScript, SystemScripts, WatchCheck, decide_watch, problem_check, run_spec, watch_due,
    watch_spec, watch_timeout,
};

/// A tick lock older than this is abandoned, whoever holds it: long enough for slow discovery and
/// two-minute watch scripts, short enough that a crash does not stop the clock for long.
pub const TICK_LOCK_STALE_MS: u64 = 5 * 60_000;

/// What a tick needs from the outside world. Everything that starts a process or reads the clock
/// is injected, so tests start nothing real.
pub struct TickContext<'a> {
    pub paths: StatePaths,
    /// The home directory: `~/.agents/automations` is discovered under it.
    pub home: PathBuf,
    /// Who is ticking: the watcher job, `fm serve`, or `fm tick` by hand.
    pub by: TickSource,
    pub starter: &'a dyn StartRun,
    pub probe: &'a dyn ProcessProbe,
    pub scripts: &'a dyn RunScript,
    /// Finds interpreters on PATH, for watch scripts and the machine checks.
    pub which: Which<'a>,
    /// Model resolution and other agent checks, run for automations that would be armed.
    pub agent_check: Option<AgentCheck<'a>>,
    /// Discover only here (tests). `None`: the user root and every registered repository.
    pub roots: Option<Vec<Root>>,
    /// Discovered too, after the others: the repository roots of the working directory, say.
    pub extra_roots: Vec<Root>,
}

impl<'a> TickContext<'a> {
    /// The real probe, script runner and PATH lookup; discovery from `home` and registered roots.
    pub fn new(
        paths: StatePaths,
        home: impl Into<PathBuf>,
        by: TickSource,
        starter: &'a dyn StartRun,
    ) -> Self {
        TickContext {
            paths,
            home: home.into(),
            by,
            starter,
            probe: &SystemProbe,
            scripts: &SystemScripts,
            which: &fm_spec::which,
            agent_check: None,
            roots: None,
            extra_roots: Vec::new(),
        }
    }
}

/// What one tick did, by automation key.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TickReport {
    /// Another tick held the lock; this one did nothing.
    pub locked: bool,
    /// Armed automations with a schedule.
    pub considered: usize,
    /// Runs started (by a schedule or a watch).
    pub fired: Vec<String>,
    /// Due, claimed, and confirmed not started: `(key, reason)`. Each has a `failed` record.
    pub failed: Vec<(String, String)>,
    /// Instants recorded as missed and not run.
    pub missed: Vec<String>,
    /// Due while a run was still going: a `skipped` record each.
    pub skipped: Vec<String>,
    /// Abandoned run locks cleared, each recorded `aborted`.
    pub swept: Vec<String>,
    /// Due but still inside the stagger offset.
    pub waiting: Vec<String>,
    /// Watch scripts run.
    pub checked: Vec<String>,
    /// Things that went wrong without stopping the tick, one line each.
    pub problems: Vec<String>,
}

impl TickReport {
    /// Anything worth a line in a log: something started, failed, was missed or swept.
    pub fn eventful(&self) -> bool {
        !(self.fired.is_empty()
            && self.failed.is_empty()
            && self.missed.is_empty()
            && self.skipped.is_empty()
            && self.swept.is_empty()
            && self.problems.is_empty())
    }

    /// One line: `fired 1 · missed 0 · swept 0`, plus failures and problems when there are any.
    pub fn summary(&self) -> String {
        if self.locked {
            return "another tick is running — this one did nothing".into();
        }
        let mut parts = vec![format!("fired {}", self.fired.len())];
        if !self.failed.is_empty() {
            parts.push(format!("failed {}", self.failed.len()));
        }
        if !self.skipped.is_empty() {
            parts.push(format!("skipped {}", self.skipped.len()));
        }
        parts.push(format!("missed {}", self.missed.len()));
        parts.push(format!("swept {}", self.swept.len()));
        if !self.checked.is_empty() {
            parts.push(format!("checked {}", self.checked.len()));
        }
        parts.join(" · ")
    }
}

/// A tick that could not run at all, in one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickError(pub String);

impl std::fmt::Display for TickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TickError {}

impl From<StateError> for TickError {
    fn from(e: StateError) -> Self {
        TickError(e.to_string())
    }
}

fn base_record(l: &Located, trigger: Trigger, outcome: Outcome, now: DateTime<Utc>) -> RunRecord {
    RunRecord {
        id: new_run_id(now),
        key: l.key.clone(),
        name: l.automation.name.clone(),
        scope: l.root.scope,
        root: l.root.path.clone(),
        trigger,
        scheduled_for: None,
        late: false,
        recorded_at: iso(now),
        started_at: None,
        ended_at: None,
        outcome,
        reason: None,
        error: None,
        agent: None,
        model: None,
        timeout: None,
        hooks: Vec::new(),
        output: None,
        session: None,
        chart: None,
        steps: Vec::new(),
        taken: Vec::new(),
        notes: Vec::new(),
        message: None,
        count: None,
        pid: None,
    }
}

/// One `missed` record for `count` instants, the newest `until`.
pub fn missed_record(
    l: &Located,
    count: u32,
    until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> RunRecord {
    RunRecord {
        scheduled_for: until.map(iso),
        reason: Some("host unavailable".into()),
        count: Some(count),
        ..base_record(l, Trigger::Schedule, Outcome::Missed, now)
    }
}

fn not_started_record(
    l: &Located,
    trigger: Trigger,
    scheduled_for: Option<String>,
    late: bool,
    reason: &str,
    now: DateTime<Utc>,
) -> RunRecord {
    RunRecord {
        scheduled_for,
        late,
        started_at: Some(iso(now)),
        ended_at: Some(iso(now)),
        error: Some(RunError {
            kind: ErrorKind::Host,
            message: format!("the run could not be started: {reason}"),
        }),
        ..base_record(l, trigger, Outcome::Failed, now)
    }
}

fn discovery_roots(ctx: &TickContext<'_>, registered: &[PathBuf]) -> Vec<Root> {
    let mut roots = match &ctx.roots {
        Some(r) => r.clone(),
        None => {
            let mut r = vec![user_root(&ctx.home)];
            for repo in registered {
                r.extend(roots_for_registered(repo));
            }
            r
        }
    };
    roots.extend(ctx.extra_roots.iter().cloned());
    let mut seen = HashSet::new();
    roots.retain(|r| seen.insert(r.path.clone()));
    roots
}

fn ms(d: u64) -> chrono::Duration {
    chrono::Duration::milliseconds(i64::try_from(d).unwrap_or(i64::MAX / 2))
}

/// Run one tick at `now`. Never more than one at a time: while another live tick holds the lock,
/// this returns at once with `locked` set.
pub fn tick(ctx: &TickContext<'_>, now: DateTime<Utc>) -> Result<TickReport, TickError> {
    let mut report = TickReport::default();
    fm_state::open_state(&ctx.paths)?;
    let Some(_lock) = acquire_tick_lock(&ctx.paths, TICK_LOCK_STALE_MS, ctx.probe, now)? else {
        report.locked = true;
        return Ok(report);
    };
    let config = load_config(&ctx.paths)?;
    let retention = Some(&config.retention);
    let store = StateStore::new(&ctx.paths);

    // A run whose host died mid-flight left no record; write one now.
    match sweep_stale_run_locks(&ctx.paths, ctx.probe, retention, now) {
        Ok(recs) => report.swept = recs.into_iter().map(|r| r.key).collect(),
        Err(e) => report.problems.push(format!("sweeping run locks: {e}")),
    }

    let state = store.load();
    let roots = discovery_roots(ctx, &state.roots);
    let found = discover(&roots, &DiscoverContext::default());
    let mut consent = Consent::load(&ctx.paths)?;
    let machine = Machine {
        platform: Platform::current(),
        which: ctx.which,
        max_timeout: Some(Duration::from_millis(duration_ms(
            &config,
            ConfigDuration::MaxTimeout,
        ))),
        agent_check: ctx.agent_check,
    };
    let assessed: Vec<Assessed> = found
        .iter()
        .map(|d| assess(d, &consent, Some(&machine), now))
        .collect();
    // Consent is refreshed on every sighting, so a live consent is never swept.
    for a in assessed.iter().filter(|a| a.located.is_some()) {
        consent.touch(&a.key, now);
    }
    if let Err(e) = consent.save() {
        report
            .problems
            .push(format!("saving consent sightings: {e}"));
    }

    let grace = ms(duration_ms(&config, ConfigDuration::Grace));
    let window = ms(duration_ms(&config, ConfigDuration::CatchUpWindow));
    let last_tick_at = state.last_tick_at.as_deref().and_then(parse_iso);
    let mut earliest_wait: Option<DateTime<Utc>> = None;

    let armed: Vec<&Located> = assessed
        .iter()
        .filter(|a| a.state == AutomationState::Armed)
        .filter_map(|a| a.located.as_ref())
        .collect();

    for l in armed.iter().copied() {
        let Some(schedule) = &l.automation.on.schedule else {
            continue;
        };
        report.considered += 1;
        let cron = match CronSchedule::from_schedule(schedule) {
            Ok(c) => c,
            Err(e) => {
                report.problems.push(format!("{}: {e}", l.key));
                continue;
            }
        };
        let input = ScheduleInput {
            key: l.key.clone(),
            stagger_sec: stagger_for(&l.key, &cron, config.stagger_max_seconds, now),
            claimed: state.claim_of(&l.key).and_then(parse_iso),
            armed_at: consent.record(&l.key).and_then(|r| parse_iso(&r.armed_at)),
            catch_up: schedule.catch_up,
            schedule: cron,
        };
        let d = decide_one(&input, last_tick_at, grace, window, now);
        if d.action == Action::None {
            continue;
        }
        if let Some(w) = d.waiting_from {
            earliest_wait = Some(earliest_wait.map_or(w, |e| e.min(w)));
        }
        if d.action == Action::Wait {
            report.waiting.push(l.key.clone());
            continue;
        }
        // One record per tick per automation, carrying the count.
        if d.missed_count > 0 {
            let rec = missed_record(l, d.missed_count, d.missed_until, now);
            if let Err(e) = append_record(&ctx.paths, &rec, retention) {
                report.problems.push(format!("{}: {e}", l.key));
            }
        }
        let Some(at) = d.scheduled_for.map(iso) else {
            continue;
        };
        if d.action == Action::Missed {
            if let Err(e) = store.claim(&l.key, &at) {
                report.problems.push(format!("{}: {e}", l.key));
            }
            report.missed.push(l.key.clone());
            continue;
        }

        // The claim is durable before anything starts: a crash in between costs one run, never
        // a duplicate. Losing it means another tick got there first.
        match store.claim(&l.key, &at) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                report.problems.push(format!("{}: {e}", l.key));
                continue;
            }
        }
        if let Some(held) = read_run_lock(&ctx.paths, &l.key) {
            let rec = RunRecord {
                scheduled_for: Some(at.clone()),
                late: d.late,
                reason: Some(format!("still running since {}", held.info.started_at)),
                ..base_record(l, Trigger::Schedule, Outcome::Skipped, now)
            };
            if let Err(e) = append_record(&ctx.paths, &rec, retention) {
                report.problems.push(format!("{}: {e}", l.key));
            }
            report.skipped.push(l.key.clone());
            continue;
        }
        match ctx
            .starter
            .start(&l.key, Trigger::Schedule, Some(at.clone()), d.late)
        {
            Ok(()) => report.fired.push(l.key.clone()),
            Err(reason) => {
                let rec = not_started_record(l, Trigger::Schedule, Some(at), d.late, &reason, now);
                if let Err(e) = append_record(&ctx.paths, &rec, retention) {
                    report.problems.push(format!("{}: {e}", l.key));
                }
                report.failed.push((l.key.clone(), reason));
            }
        }
    }

    check_watches(ctx, &store, &config, &armed, now, &mut report);

    // Rewind so an instant held back by its stagger stays in the next tick's window.
    let noted = match earliest_wait {
        Some(w) => now.min(w - chrono::Duration::milliseconds(1)),
        None => now,
    };
    let live: BTreeSet<String> = assessed.iter().map(|a| a.key.clone()).collect();
    store.update(|s| {
        s.note_tick(noted, ctx.by);
        s.prune(&live);
    })?;
    Ok(report)
}

fn check_watches(
    ctx: &TickContext<'_>,
    store: &StateStore,
    config: &fm_model::Config,
    armed: &[&Located],
    now: DateTime<Utc>,
    report: &mut TickReport,
) {
    let fired_here: HashSet<String> = report.fired.iter().cloned().collect();
    let state = store.load();
    let timeout = watch_timeout(Duration::from_millis(duration_ms(
        config,
        ConfigDuration::HookTimeout,
    )));

    struct Due<'l> {
        l: &'l Located,
        script: String,
        prev: Option<fm_model::WatchState>,
        spec: Result<fm_run::process::ProcessSpec, String>,
    }

    let due: Vec<Due<'_>> = armed
        .iter()
        .copied()
        .filter_map(|l| {
            let watch = l.automation.on.watch.as_ref()?;
            if state.paused_watches.contains_key(&l.key)
                || fired_here.contains(&l.key)
                || read_run_lock(&ctx.paths, &l.key).is_some()
                || !watch_due(state.watches.get(&l.key), watch, now)
            {
                return None;
            }
            let prev = state.watches.get(&l.key).cloned();
            let last_checked = prev.as_ref().and_then(|p| p.answered_at.clone());
            let spec = watch_spec(
                l,
                last_checked.as_deref(),
                timeout,
                config.hook_max_output_bytes,
                ctx.which,
            );
            Some(Due {
                l,
                script: watch.script.clone(),
                prev,
                spec,
            })
        })
        .collect();
    if due.is_empty() {
        return;
    }

    // Scripts can be slow: run them side by side, each bounded by its own timeout.
    let checks: Vec<WatchCheck> = std::thread::scope(|s| {
        let handles: Vec<_> = due
            .iter()
            .map(|d| {
                let scripts = ctx.scripts;
                s.spawn(move || match &d.spec {
                    Ok(spec) => run_spec(&d.script, spec, scripts),
                    Err(e) => problem_check(e.clone(), 0),
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| problem_check("the watch check panicked".into(), 0))
            })
            .collect()
    });

    // Re-read: a pause may have landed while the scripts ran.
    let fresh = store.load();
    let retention = Some(&config.retention);
    for (d, check) in due.iter().zip(checks) {
        let key = &d.l.key;
        report.checked.push(key.clone());
        let decision = decide_watch(d.prev.as_ref(), &check, now);

        // Paused while its script ran: start nothing and keep the finding new for the resume.
        if fresh.paused_watches.contains_key(key) {
            let mut seen = decision.next;
            seen.fired_at = d.prev.as_ref().and_then(|p| p.fired_at.clone());
            seen.fired_for = d.prev.as_ref().and_then(|p| p.fired_for.clone());
            if let Err(e) = store.record_watch(key, seen) {
                report.problems.push(format!("{key}: {e}"));
            }
            continue;
        }
        if let Err(e) = store.record_watch(key, decision.next) {
            report.problems.push(format!("{key}: {e}"));
            // Not recorded means not remembered: starting now could start it again next time.
            continue;
        }
        if !decision.fire {
            continue;
        }
        match ctx.starter.start(key, Trigger::Watch, None, false) {
            Ok(()) => report.fired.push(key.clone()),
            Err(reason) => {
                let rec = not_started_record(d.l, Trigger::Watch, None, false, &reason, now);
                if let Err(e) = append_record(&ctx.paths, &rec, retention) {
                    report.problems.push(format!("{key}: {e}"));
                }
                report.failed.push((key.clone(), reason));
            }
        }
    }
}

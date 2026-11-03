//! The `fm` command line. Mirrors pi-automations' `pa` where it applies.
//!
//! Exit codes: 0 ok, 1 usage or error, 2 refused. `fm run` in the foreground passes on the run's
//! own code (10 skipped, 20 failed, 30 aborted).

use std::io::{BufRead, IsTerminal, Write};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use fm_model::{AutomationState, CliId, Discovered, Host, Status, Trigger};
use fm_spec::{CronSchedule, Platform, by_key, digest_of, interpreter_map};
use fm_state::{is_armed_in_pa, last_record, read_records, read_run_lock};

use crate::host::{RealHost, stagger_of};
use crate::resolve::{Match, ambiguous_line, not_found_line, resolve};
use crate::seams::{self, FireArgs};
use crate::text::{
    listing, live_lines, log_lines, outcome_word, preview_lines, span, state_word, tildify,
};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_REFUSED: i32 = 2;

/// The verbs a run uses to report progress; they skip discovery entirely.
pub const IN_RUN_VERBS: [&str; 6] = ["step", "next", "say", "note", "message", "status"];

const IN_RUN_HELP: &str = "\
Inside a run (AUTOMATION_RUN is set for the body, every hook and what they start):
  fm step <node> [--status active|waiting|done|failed|skipped] [-m text]
                        set one node; active by default
  fm next <to> [--from <node>] [-m text]
                        finish <node> and start <to>; <node> defaults to AUTOMATION_NODE, else the one running node
  fm say <text> [--node <node>] [--request-id <key>]
                        add an update to a node; a request id makes a retry a no-op
  fm note <text> [--link \"label=https://…\"]… [--request-id <key>]
                        a note on the run's record, with the links it published
  fm message <text> [--link \"label=https://…\"]… [--request-id <key>]
                        the run's one message for its reader; the last one wins
  fm node <id> [-m <text>] [--cwd <dir>]
                        hand a step listed in nodes: to its own agent, and print its answer
  fm status             where the run is: every node, the arrows taken, the notes, the message
                        every in-run command takes --json: one object on stdout, ok or not

Exit codes: 0 ok · 1 usage or error · 2 refused · 10 run skipped · 20 run failed · 30 run aborted";

#[derive(Debug, Parser)]
#[command(
    name = "fm",
    bin_name = "fm",
    version,
    about = "Run Agent Automations through any agent CLI. With no command, the terminal UI.",
    after_help = IN_RUN_HELP
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Draw a Mermaid flowchart with node statuses, as text or SVG (`fm render --help`)
    #[command(disable_help_flag = true)]
    Render {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        args: Vec<String>,
    },
    /// Open the terminal UI (the default)
    Tui {
        /// Demo data, nothing on disk
        #[arg(long)]
        fake: bool,
    },
    /// List every automation, grouped by scope
    List {
        /// Every automation as one JSON array
        #[arg(long)]
        json: bool,
    },
    /// Re-check one automation and show it with its consent preview; every one without a name
    Check { name: Option<String> },
    /// Arm it, after the consent preview
    Arm {
        name: String,
        /// Accept the preview without asking
        #[arg(long, short)]
        yes: bool,
    },
    /// Withdraw consent; it stays listed
    Disarm { name: String },
    /// Set enabled: true in a user automation
    Enable { name: String },
    /// Set enabled: false in a user automation
    Disable { name: String },
    /// Run one now, by hand, showing it as it goes
    Run {
        name: String,
        /// Start it in the background and return
        #[arg(long)]
        detach: bool,
    },
    /// Stop the run that is going now
    Stop { name: String },
    /// The last runs, with their notes and links
    #[command(visible_alias = "runs")]
    Log {
        name: String,
        /// How many
        #[arg(short = 'n', default_value_t = 10)]
        n: usize,
        /// One record per line, as JSON
        #[arg(long)]
        json: bool,
    },
    /// Pause or resume a watch's checks
    Watch {
        #[command(subcommand)]
        verb: WatchVerb,
    },
    /// The agent CLIs that run a body
    Agent {
        #[command(subcommand)]
        verb: AgentVerb,
    },
    /// Run what is due, once
    Tick {
        /// What the tick did, as one JSON object
        #[arg(long)]
        json: bool,
        /// Who is ticking: the watcher job passes `job`.
        #[arg(long, value_enum, default_value = "cli", hide = true)]
        by: TickBy,
    },
    /// Tick every minute, in the foreground
    Serve,
    /// The watcher, which fires schedules and runs watch checks
    Watcher {
        #[command(subcommand)]
        verb: WatcherVerb,
        /// The answer as one JSON object
        #[arg(long, global = true)]
        json: bool,
    },
    /// Run one automation by its key (what the watcher and `fm run` start)
    #[command(hide = true)]
    Fire {
        #[arg(long)]
        key: String,
        #[arg(long, value_enum)]
        trigger: TriggerArg,
        #[arg(long)]
        scheduled_for: Option<String>,
        #[arg(long)]
        late: bool,
        /// The caller follows the live file: leave it when the run ends (what `fm run` passes)
        #[arg(long, hide = true)]
        follow: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum WatchVerb {
    /// Stop its checks until resumed; its schedule and manual runs still work
    Pause { name: String },
    /// Resume its checks
    Resume { name: String },
}

#[derive(Debug, Subcommand)]
pub enum AgentVerb {
    /// The CLIs found on this machine, with their versions
    List,
    /// Run one automation through <cli>, or `default` to use the default again
    Set { name: String, cli: String },
    /// The CLI every automation runs through unless it has its own (`none` clears it)
    Default { cli: String },
}

#[derive(Debug, Subcommand)]
pub enum WatcherVerb {
    /// Register the job with the OS: Task Scheduler, launchd, or a systemd user timer or cron
    Start,
    /// Remove the job
    Stop,
    /// Whether the watcher is on, and when it last ticked
    Status,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TickBy {
    Job,
    Serve,
    Cli,
}

impl From<TickBy> for fm_model::TickSource {
    fn from(b: TickBy) -> fm_model::TickSource {
        match b {
            TickBy::Job => fm_model::TickSource::Job,
            TickBy::Serve => fm_model::TickSource::Serve,
            TickBy::Cli => fm_model::TickSource::Cli,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TriggerArg {
    Manual,
    Schedule,
    Watch,
}

impl From<TriggerArg> for Trigger {
    fn from(t: TriggerArg) -> Trigger {
        match t {
            TriggerArg::Manual => Trigger::Manual,
            TriggerArg::Schedule => Trigger::Schedule,
            TriggerArg::Watch => Trigger::Watch,
        }
    }
}

/// `println!` that ignores a closed stdout (`fm list | head`).
macro_rules! out {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

fn refuse(line: impl AsRef<str>) -> i32 {
    eprintln!("{}", line.as_ref());
    EXIT_REFUSED
}

fn print(lines: &[String]) {
    let mut out = std::io::stdout().lock();
    for l in lines {
        let _ = writeln!(out, "{l}");
    }
}

/// Entry point: returns the process exit code.
pub fn main() -> i32 {
    let argv: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if let Some(verb) = argv.get(1) {
        if IN_RUN_VERBS.contains(&verb.as_str()) {
            return seams::progress(&argv[1..]);
        }
        // `fm node` runs a step of the run it is called from, through the step's own agent.
        if verb == "node" {
            return match crate::env::HostEnv::from_process() {
                Ok(env) => seams::node(&env, &argv[2..]),
                Err(e) => {
                    eprintln!("{e:#}");
                    EXIT_ERROR
                }
            };
        }
        // `fm render` keeps its own parser, messages and exit codes (0, 1 I/O, 2 bad input).
        if verb == "render" {
            return i32::from(fm_render::cli::render_status(&argv[1..]));
        }
    }
    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { EXIT_ERROR } else { EXIT_OK };
        }
    };
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e:#}");
            EXIT_ERROR
        }
    }
}

fn pick(host: &mut RealHost, name: &str) -> Result<Status, i32> {
    let all = host.statuses();
    match resolve(&all, name) {
        Match::One(s) => Ok(s.clone()),
        Match::Ambiguous(v) => Err(refuse(ambiguous_line(name, &v))),
        Match::None => Err(refuse(not_found_line(name))),
    }
}

fn act(r: Result<String, String>) -> i32 {
    match r {
        Ok(line) => {
            out!("{line}");
            EXIT_OK
        }
        Err(line) => refuse(line),
    }
}

pub fn run(cli: Cli) -> anyhow::Result<i32> {
    let cmd = cli.cmd.unwrap_or(Cmd::Tui { fake: false });
    if let Cmd::Tui { fake: true } = cmd {
        fm_tui::run_fake(fm_tui::TuiOptions::from_env()).context("terminal UI")?;
        return Ok(EXIT_OK);
    }
    let mut host = RealHost::from_process()?;
    if let Cmd::Tui { .. } = cmd {
        fm_tui::run(&mut host, fm_tui::TuiOptions::from_env()).context("terminal UI")?;
        return Ok(EXIT_OK);
    }
    if let Err(e) = host.config() {
        eprintln!("{e}");
        return Ok(EXIT_ERROR);
    }

    macro_rules! pick {
        ($name:expr) => {
            match pick(&mut host, $name) {
                Ok(s) => s,
                Err(code) => return Ok(code),
            }
        };
    }

    Ok(match cmd {
        Cmd::Tui { .. } => unreachable!("handled above"),
        Cmd::Render { .. } => unreachable!("`fm render` is handled before parsing"),
        Cmd::List { json } => {
            let all = host.statuses();
            if json {
                out!("{}", serde_json::to_string_pretty(&all)?);
            } else {
                print(&listing(&all, &host.env().home, host.now()));
            }
            EXIT_OK
        }
        Cmd::Check { name: None } => {
            let mut all = host.statuses();
            all.sort_by(|a, b| a.name.cmp(&b.name));
            if all.is_empty() {
                out!("no automations found");
            }
            for s in all {
                match &s.reason {
                    Some(r) => out!("{}  {}  — {r}", s.name, state_word(s.state)),
                    None => out!("{}  {}", s.name, state_word(s.state)),
                }
            }
            EXIT_OK
        }
        Cmd::Check { name: Some(name) } => {
            let s = pick!(&name);
            print(&check_lines(&mut host, &s));
            EXIT_OK
        }
        Cmd::Arm { name, yes } => {
            let s = pick!(&name);
            arm(&mut host, &s, yes)
        }
        Cmd::Disarm { name } => {
            let s = pick!(&name);
            act(host.disarm(&s.key))
        }
        Cmd::Enable { name } => {
            let s = pick!(&name);
            act(host.set_enabled(&s.key, true))
        }
        Cmd::Disable { name } => {
            let s = pick!(&name);
            act(host.set_enabled(&s.key, false))
        }
        Cmd::Run { name, detach } => {
            let s = pick!(&name);
            if detach {
                act(host.run_now(&s.key))
            } else {
                run_attached(&mut host, &s)?
            }
        }
        Cmd::Stop { name } => {
            let s = pick!(&name);
            act(host.stop(&s.key))
        }
        Cmd::Log { name, n, json } => {
            let s = pick!(&name);
            let records = read_records(host.paths(), &s.key, n);
            if json {
                for r in &records {
                    out!("{}", serde_json::to_string(r)?);
                }
            } else if records.is_empty() {
                out!("{} has not run yet", s.name);
            } else {
                for r in &records {
                    print(&log_lines(r));
                }
            }
            EXIT_OK
        }
        Cmd::Watch { verb } => {
            let (name, paused) = match &verb {
                WatchVerb::Pause { name } => (name, true),
                WatchVerb::Resume { name } => (name, false),
            };
            let s = pick!(name);
            act(host.pause_watch(&s.key, paused))
        }
        Cmd::Agent { verb } => agent(&mut host, verb)?,
        Cmd::Tick { json, by } => seams::tick(host.env(), json, by.into())?,
        Cmd::Serve => seams::serve(host.env())?,
        Cmd::Watcher { verb, json } => {
            let v = match verb {
                WatcherVerb::Start => "start",
                WatcherVerb::Stop => "stop",
                WatcherVerb::Status => "status",
            };
            seams::watcher(host.env(), v, json)?
        }
        Cmd::Fire {
            key,
            trigger,
            scheduled_for,
            late,
            follow,
        } => seams::fire(
            host.env(),
            &FireArgs {
                key,
                trigger: trigger.into(),
                scheduled_for,
                late,
                follow,
            },
        )?,
    })
}

fn arm(host: &mut RealHost, s: &Status, yes: bool) -> i32 {
    let name = &s.name;
    match s.state {
        AutomationState::Invalid => {
            return refuse(format!(
                "{name}: {}",
                s.reason.as_deref().unwrap_or("invalid")
            ));
        }
        AutomationState::Disabled => {
            return refuse(format!(
                "{name}: enabled: false in the file — it is inert on every host"
            ));
        }
        AutomationState::Armed => {
            out!("{name} is already armed");
            return EXIT_OK;
        }
        AutomationState::Off => {}
    }
    let preview = match host.preview(&s.key) {
        Ok(p) => p,
        Err(e) => return refuse(e),
    };
    let root = s.dir.parent().unwrap_or(&s.dir);
    let description = s
        .automation
        .as_ref()
        .map(|a| a.description.clone())
        .unwrap_or_default();
    print(&preview_lines(
        &preview,
        &description,
        "Arm",
        &tildify(root, &host.env().home),
    ));
    if !yes {
        if !std::io::stdin().is_terminal() {
            return refuse("not armed — re-run with --yes to accept the preview");
        }
        print!("Arm {name}? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return refuse("not armed");
        }
    }
    act(host.arm(&s.key))
}

/// `fm run` in the foreground: start `fm fire --follow`, print the live view as it grows, then
/// the record. With `--follow` the run leaves its live file when it ends, so the last events (the
/// final post hook's end) are read here once it has exited, before the file is removed.
fn run_attached(host: &mut RealHost, s: &Status) -> anyhow::Result<i32> {
    if let Some(why) = RealHost::run_refusal(s) {
        return Ok(refuse(why));
    }
    let started = host.now();
    // Events from before this run (a file a killed `fm run` never removed) are not this run's.
    let since = fm_run::live::now_ms();
    let mut cmd = host.fire_command(&s.key);
    cmd.arg("--follow").stdin(Stdio::null());
    let mut child = cmd.spawn().context("could not start fm fire")?;
    let paths = host.paths().clone();
    let live = paths.live_file(&s.key);
    let mut run_id: Option<String> = None;
    let mut printed = 0usize;
    // False when the file could not be read this time; nothing is printed or forgotten then.
    let show = |printed: &mut usize| -> bool {
        let Ok(bytes) = std::fs::read(&live) else {
            return false;
        };
        let events: Vec<fm_model::LiveEvent> = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|l| serde_json::from_str::<fm_model::LiveEvent>(l).ok())
            .filter(|e| e.ts() >= since)
            .collect();
        if events.len() < *printed {
            *printed = 0;
        }
        for line in live_lines(&events, *printed) {
            out!("{line}");
        }
        *printed = events.len();
        true
    };
    let code = loop {
        if run_id.is_none() {
            run_id = read_run_lock(&paths, &s.key).map(|l| l.info.run_id);
        }
        if run_id.is_some() {
            show(&mut printed);
        }
        if let Some(status) = child.try_wait()? {
            let code = status.code().unwrap_or(EXIT_ERROR);
            // Refused: another run holds the lock, and the live file is its own.
            if code != EXIT_REFUSED {
                // A scanner holding the file a moment is no reason to lose the last events.
                for _ in 0..10 {
                    if show(&mut printed) || !live.exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = std::fs::remove_file(&live);
            }
            break code;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let rec = last_record(&paths, &s.key).filter(|r| match &run_id {
        Some(id) => &r.id == id,
        None => fm_state::time::parse_iso(&r.recorded_at).is_some_and(|t| t >= started),
    });
    if let Some(rec) = rec {
        print(&log_lines(&rec));
    }
    Ok(code)
}

fn agent(host: &mut RealHost, verb: AgentVerb) -> anyhow::Result<i32> {
    Ok(match verb {
        AgentVerb::List => {
            let config = host.config().unwrap_or_default();
            let found = host.detect_all();
            out!("  {:<10}  {:<12}  PATH", "CLI", "VERSION");
            for a in found {
                let mark = if config.default_agent == Some(a.cli) {
                    "*"
                } else {
                    " "
                };
                match a.found {
                    Ok(i) => out!(
                        "{mark} {:<10}  {:<12}  {}",
                        a.cli.as_str(),
                        i.version.as_deref().unwrap_or("?"),
                        i.path.display()
                    ),
                    Err(e) => out!("{mark} {:<10}  {:<12}  {e}", a.cli.as_str(), "—"),
                }
            }
            match config.default_agent {
                Some(d) => out!("\n* the default: {d} ({})", d.title()),
                None => out!("\nno default agent — fm agent default <cli>"),
            }
            EXIT_OK
        }
        AgentVerb::Set { name, cli } => {
            let agent = if cli == "default" {
                None
            } else {
                match cli.parse::<CliId>() {
                    Ok(c) => Some(c),
                    Err(e) => {
                        eprintln!("{e}, or default");
                        return Ok(EXIT_ERROR);
                    }
                }
            };
            let s = match pick(host, &name) {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            act(host.set_agent(&s.key, agent))
        }
        AgentVerb::Default { cli } => {
            let agent = if cli == "none" {
                None
            } else {
                match cli.parse::<CliId>() {
                    Ok(c) => Some(c),
                    Err(e) => {
                        eprintln!("{e}, or none");
                        return Ok(EXIT_ERROR);
                    }
                }
            };
            act(host.set_default_agent(agent))
        }
    })
}

/// `fm check <name>`: the state and why, what it is, how it runs, and the consent preview.
pub fn check_lines(host: &mut RealHost, s: &Status) -> Vec<String> {
    let now = host.now();
    let home = host.env().home.clone();
    let config = host.config().unwrap_or_default();
    let mut out = vec![format!("{}  {}", s.name, state_word(s.state))];
    let row = |label: &str, value: &str| format!("  {label:<12}  {value}");
    if !s.problems.is_empty() {
        let field = s
            .reason
            .as_deref()
            .and_then(|r| r.split(':').next())
            .unwrap_or("");
        let n = s.problems.len();
        out.push(row(
            "reason",
            &format!("{field}: {n} problem{}", if n == 1 { "" } else { "s" }),
        ));
        for p in &s.problems {
            out.push(format!("    {p}"));
        }
    } else if let Some(r) = &s.reason {
        out.push(row("reason", r));
    }
    out.push(row(
        "scope",
        match s.scope {
            fm_model::Scope::User => "user",
            fm_model::Scope::Repository => "repository",
        },
    ));
    out.push(row("directory", &tildify(&s.dir, &home)));
    out.push(row("key", &s.key));
    out.push(row(
        "agent",
        &match &s.agent {
            Some(a)
                if config
                    .automations
                    .get(&s.key)
                    .and_then(|x| x.agent)
                    .is_some() =>
            {
                format!("{a} (set for this automation)")
            }
            Some(a) => format!("{a} (the default)"),
            None => "none — fm agent default <cli>".into(),
        },
    ));
    out.push(row(
        "timeout",
        &span(i64::try_from(s.effective_timeout_ms).unwrap_or(i64::MAX)),
    ));

    let found = host.rediscover().to_vec();
    let located = match by_key(&found, &s.key) {
        Some(Discovered::Valid(l)) => Some(l.as_ref().clone()),
        _ => None,
    };
    if let Some(l) = &located {
        match digest_of(l) {
            Ok(d) => out.push(row("digest", &d.digest)),
            Err(r) => out.push(row("digest", &r.to_string())),
        }
        let which = |bin: &str| {
            fm_spec::which_in(
                bin,
                &host.env().path_var,
                host.env().pathext.as_deref(),
                Platform::current(),
            )
        };
        for (script, cmd) in interpreter_map(l, Platform::current(), &which) {
            out.push(row("interpreter", &format!("{script} → {cmd}")));
        }
        if let Some(sc) = &l.automation.on.schedule {
            let tz = sc
                .timezone
                .as_deref()
                .map(|t| format!(" · {t}"))
                .unwrap_or_default();
            out.push(row("schedule", &format!("{}{tz}", sc.cron)));
            if let Ok(cron) = CronSchedule::from_schedule(sc) {
                let stagger = stagger_of(&s.key, &cron, &config, now);
                out.push(row("stagger", &format!("{stagger}s")));
                let mut cursor = now;
                for _ in 0..3 {
                    let Some(next) = cron.next_after(cursor) else {
                        break;
                    };
                    let at = next + chrono::Duration::seconds(i64::try_from(stagger).unwrap_or(0));
                    out.push(row(
                        "next",
                        &format!(
                            "{}  ({})",
                            fm_spec::iso_with_offset(at, cron.zone()),
                            at.with_timezone(&chrono::Local).format("%a %b %-d %H:%M")
                        ),
                    ));
                    cursor = next;
                }
            }
        }
        if let Some(w) = &l.automation.on.watch {
            out.push(row("watch", &format!("{} every {}", w.script, w.every)));
            if let Some(st) = &s.watch {
                let result = st
                    .problem
                    .clone()
                    .unwrap_or_else(|| format!("exit {}", st.exit.unwrap_or(0)));
                out.push(row("last check", &format!("{} · {result}", st.checked_at)));
            }
            if let Some(p) = &s.watch_paused_at {
                out.push(row(
                    "paused",
                    &format!("since {p} — fm watch resume {}", s.name),
                ));
            }
        }
    }
    if let Some(r) = &s.running {
        out.push(row(
            "running",
            &format!("{} since {}", r.run_id, r.started_at),
        ));
    }
    if let Some(last) = &s.last {
        let took = last
            .duration_ms
            .map(|d| format!(" in {}", span(i64::try_from(d).unwrap_or(0))))
            .unwrap_or_default();
        let why = last
            .reason
            .as_deref()
            .map(|r| format!(" — {r}"))
            .unwrap_or_default();
        out.push(row(
            "last run",
            &format!("{} at {}{took}{why}", outcome_word(last.outcome), last.at),
        ));
    }
    if is_armed_in_pa(&s.key, &host.pa_keys()) {
        out.push(row(
            "pa",
            &format!(
                "armed in pi-automations too{}",
                if s.state == AutomationState::Armed {
                    format!(" — it runs twice; pa disarm {}", s.name)
                } else {
                    String::new()
                }
            ),
        ));
    }

    if let Some(l) = &located {
        let preview = host.preview_of(l, &config);
        out.push(String::new());
        let root = tildify(&l.root.path, &home);
        out.extend(preview_lines(
            &preview,
            &l.automation.description,
            "Consent preview for",
            &root,
        ));
    }
    out
}

//! Where `fm` hands off to the run engine (`fm-run`) and the clock (`fm-clock`).

use chrono::{DateTime, Utc};
use fm_clock::watcher::{self, WatcherContext};
use fm_clock::{DetachedSpawn, FixedClock, TickContext};
use fm_model::{Discovered, Located, Outcome, TickSource, Trigger};
use fm_run::{FireDeps, FireError, FireOptions};
use fm_spec::{DiscoverContext, Platform, by_key, discover, repository_roots, which_in};
use fm_state::keys::name_of;
use fm_state::{ProcessEnv, load_config};

use crate::cli::EXIT_REFUSED;
use crate::env::HostEnv;
use crate::host::RealHost;

/// `fm fire`: the contract the clock spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireArgs {
    pub key: String,
    pub trigger: Trigger,
    pub scheduled_for: Option<String>,
    pub late: bool,
    /// The caller follows the live file and removes it once read to the end.
    pub follow: bool,
}

/// The exit code of a run that left this record: 0 ok, 10 skipped, 20 failed, 30 aborted.
pub fn exit_code_of(outcome: Outcome) -> i32 {
    match outcome {
        Outcome::Ok => 0,
        Outcome::Skipped | Outcome::Missed => 10,
        Outcome::Failed => 20,
        Outcome::Aborted => 30,
    }
}

/// Run one automation now, in this process, through [`fm_run::fire`]. Returns the exit code
/// (0 ok, 10 skipped, 20 failed, 30 aborted; 2 when it was refused with no record). Prints
/// nothing on stdout: `fm run` shows the live view and the record.
pub fn fire(env: &HostEnv, args: &FireArgs) -> anyhow::Result<i32> {
    let found = discover(
        &RealHost::new(env.clone()).roots(),
        &DiscoverContext::default(),
    );
    let l = match by_key(&found, &args.key) {
        Some(Discovered::Valid(l)) => l.as_ref().clone(),
        Some(Discovered::Invalid { error, .. }) => {
            eprintln!("{}: {error}", name_of(&args.key));
            return Ok(EXIT_REFUSED);
        }
        None => {
            eprintln!(
                "{}: not found — no AUTOMATION.md for {} here",
                name_of(&args.key),
                args.key
            );
            return Ok(EXIT_REFUSED);
        }
    };
    if !l.automation.enabled {
        eprintln!(
            "{}: enabled: false in the file — it is inert on every host",
            l.automation.name
        );
        return Ok(EXIT_REFUSED);
    }
    let config = load_config(&env.state)?;
    let mut deps = FireDeps::new(env.state.clone(), config);
    deps.fm_program = Some(env.exe.clone());
    deps.notify = Some(crate::notify::desktop_notifier());
    let mut opts = FireOptions::new(args.trigger);
    opts.scheduled_for.clone_from(&args.scheduled_for);
    opts.late = args.late;
    opts.keep_live = args.follow;
    match fm_run::fire(&l, opts, &deps) {
        Ok(rec) => Ok(exit_code_of(rec.outcome)),
        Err(e @ FireError::Running { .. }) => {
            eprintln!("{e}");
            Ok(EXIT_REFUSED)
        }
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

/// How the clock starts a run: this executable as `fm fire --key … --trigger …`, detached, against
/// the same state folder, its output appended to `logs/<slug>.log`.
pub fn fire_starter(env: &HostEnv) -> DetachedSpawn {
    let mut s = DetachedSpawn::new(&env.exe).with_log_dir(env.state.logs_dir());
    s.env = vec![(
        "FM_HOME".into(),
        env.state.root.to_string_lossy().into_owned(),
    )];
    s
}

/// Run one tick with `by` as its source, starting runs with [`fire_starter`].
pub fn run_tick(env: &HostEnv, by: TickSource) -> anyhow::Result<fm_clock::TickReport> {
    let config = fm_state::load_config(&env.state)?;
    let starter = fire_starter(env);
    let path_var = env.path_var.clone();
    let pathext = env.pathext.clone();
    let which = move |b: &str| which_in(b, &path_var, pathext.as_deref(), Platform::current());
    let check = move |l: &Located| crate::host::agent_check(&config, l);
    let mut ctx = TickContext::new(env.state.clone(), env.home.clone(), by, &starter);
    ctx.which = &which;
    ctx.agent_check = Some(&check);
    if by == TickSource::Cli {
        // By hand from a repository: its automations too, registered or not.
        ctx.extra_roots = repository_roots(&env.cwd);
    }
    fm_clock::tick(&ctx, Utc::now()).map_err(|e| anyhow::anyhow!("fm tick: {e}"))
}

/// One tick: what is due runs. `json` prints the summary as one object. `by` is who ticks: the
/// watcher job passes `job`; by hand it is `cli`, and a quiet tick still says so.
pub fn tick(env: &HostEnv, json: bool, by: TickSource) -> anyhow::Result<i32> {
    let r = run_tick(env, by)?;
    if json {
        println!("{}", serde_json::to_string(&r)?);
        return Ok(0);
    }
    if by == TickSource::Job && !r.eventful() && !r.locked {
        // The job's output is appended to tick.log every minute: quiet ticks write nothing.
        return Ok(0);
    }
    let stamp = if by == TickSource::Job {
        format!("{} ", fm_state::time::iso(Utc::now()))
    } else {
        String::new()
    };
    println!("{stamp}{}", r.summary());
    for (key, reason) in &r.failed {
        println!("  {key}: could not start — {reason}");
    }
    for p in &r.problems {
        println!("  {p}");
    }
    Ok(0)
}

/// Tick every minute in the foreground, until Ctrl-C. Refused while the watcher job is registered.
pub fn serve(env: &HostEnv) -> anyhow::Result<i32> {
    let config = fm_state::load_config(&env.state)?;
    let starter = fire_starter(env);
    let path_var = env.path_var.clone();
    let pathext = env.pathext.clone();
    let which = move |b: &str| which_in(b, &path_var, pathext.as_deref(), Platform::current());
    let check = move |l: &Located| crate::host::agent_check(&config, l);
    let mut ctx = TickContext::new(
        env.state.clone(),
        env.home.clone(),
        TickSource::Serve,
        &starter,
    );
    ctx.which = &which;
    ctx.agent_check = Some(&check);
    match fm_clock::serve_until_ctrl_c(&ctx) {
        Ok(()) => Ok(0),
        Err(line) => {
            eprintln!("{line}");
            Ok(crate::cli::EXIT_REFUSED)
        }
    }
}

fn watcher_ctx(env: &HostEnv) -> anyhow::Result<WatcherContext<'static>> {
    let mut ctx = WatcherContext::system(env.state.clone()).map_err(|e| anyhow::anyhow!(e))?;
    ctx.home = env.home.clone();
    ctx.current_exe = Some(env.exe.clone());
    Ok(ctx)
}

/// `fm watcher start|stop|status`.
pub fn watcher(env: &HostEnv, verb: &str, json: bool) -> anyhow::Result<i32> {
    let ctx = watcher_ctx(env)?;
    let (ok, lines, value) = match verb {
        "start" => {
            let r = watcher::start(&ctx, &env.exe);
            (r.ok, r.lines.clone(), serde_json::to_value(&r)?)
        }
        "stop" => {
            let r = watcher::stop(&ctx);
            (r.ok, r.lines.clone(), serde_json::to_value(&r)?)
        }
        _ => {
            let s = watcher::status(&ctx, true);
            (true, s.lines.clone(), serde_json::to_value(&s)?)
        }
    };
    if json {
        println!("{value}");
    } else {
        for l in &lines {
            println!("{l}");
        }
    }
    Ok(if ok { 0 } else { crate::cli::EXIT_ERROR })
}

/// The watcher in one line for the TUI (`on — last tick 12s ago`), without asking the scheduler.
pub fn watcher_summary(env: &HostEnv, now: DateTime<Utc>) -> String {
    let clock = FixedClock(now);
    match watcher_ctx(env) {
        Ok(ctx) => {
            let mut ctx: WatcherContext<'_> = ctx;
            ctx.clock = &clock;
            watcher::status(&ctx, false).summary
        }
        Err(e) => format!("off — {e}"),
    }
}

/// `fm step|next|say|note|message|status …`, `argv` starting with the verb. Reads `AUTOMATION_RUN`,
/// `AUTOMATION_NODE` and `AUTOMATION_STATE_DIR` from the environment.
pub fn progress(argv: &[String]) -> i32 {
    let out = fm_run::progress_cli(argv, &ProcessEnv, Utc::now());
    print!("{}", out.stdout);
    eprint!("{}", out.stderr);
    out.code
}

/// `fm node <id> [-m <text> | --file <path>] [--cwd <dir>] [--run <id>] [--json]`: run one step of
/// the run through its node's agent and print the answer. 0 answered, 2 refused, 20 the agent did
/// not answer, 30 stopped or out of time.
pub fn node(env: &HostEnv, argv: &[String]) -> i32 {
    const USAGE: &str =
        "usage: fm node <id> [-m <text> | --file <path>] [--cwd <dir>] [--run <id>] [--json]";
    let mut id = None;
    let (mut message, mut file, mut cwd, mut run, mut json) = (None, None, None, None, false);
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let mut value = || {
            inline.clone().or_else(|| {
                i += 1;
                argv.get(i).cloned()
            })
        };
        let slot = match name {
            "-m" | "--message" => &mut message,
            "--file" => &mut file,
            "--cwd" => &mut cwd,
            "--run" => &mut run,
            "--json" => {
                json = true;
                i += 1;
                continue;
            }
            n if n.starts_with('-') => {
                eprintln!("fm node: unknown option {n}\n{USAGE}");
                return 1;
            }
            _ if id.is_none() => {
                id = Some(a.clone());
                i += 1;
                continue;
            }
            _ => {
                eprintln!("fm node: one step at a time\n{USAGE}");
                return 1;
            }
        };
        match value() {
            Some(v) => *slot = Some(v),
            None => {
                eprintln!("fm node: {name} needs a value\n{USAGE}");
                return 1;
            }
        }
        i += 1;
    }
    let Some(id) = id else {
        eprintln!("fm node: which step?\n{USAGE}");
        return 1;
    };
    if message.is_some() && file.is_some() {
        eprintln!("fm node: -m and --file are two handoffs; pick one\n{USAGE}");
        return 1;
    }
    let handoff = match file {
        Some(f) => match std::fs::read_to_string(&f) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("fm node: cannot read {f}: {e}");
                return 1;
            }
        },
        None => message,
    };
    let refuse = |m: String| {
        eprintln!("fm node: {m}");
        EXIT_REFUSED
    };
    let Some(run_id) = run.or_else(|| {
        std::env::var("AUTOMATION_RUN")
            .ok()
            .filter(|s| !s.is_empty())
    }) else {
        return refuse("not inside a run — AUTOMATION_RUN is not set (or pass --run <id>)".into());
    };
    let paths = std::env::var("AUTOMATION_STATE_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(
            || env.state.clone(),
            |d| fm_model::StatePaths::new(d.trim()),
        );
    let snap = match fm_state::progress_status(&paths, Some(&run_id)) {
        Ok(s) => s,
        Err(e) => return refuse(e.message),
    };
    let found = discover(
        &RealHost::new(env.clone()).roots(),
        &DiscoverContext::default(),
    );
    let l: Located = match by_key(&found, &snap.key) {
        Some(Discovered::Valid(l)) => l.as_ref().clone(),
        Some(Discovered::Invalid { error, .. }) => {
            return refuse(format!("{}: {error}", snap.name));
        }
        None => {
            return refuse(format!(
                "{}: its AUTOMATION.md is not found here",
                snap.name
            ));
        }
    };
    let config = match load_config(&env.state) {
        Ok(c) => c,
        Err(e) => return refuse(e.to_string()),
    };
    let deps = fm_run::node::NodeDeps {
        paths: &paths,
        config: &config,
        located: &l,
        path_env: fm_run::fire::path_overlay(Some(&env.exe)),
        poll: std::time::Duration::from_millis(100),
    };
    let req = fm_run::node::NodeRequest {
        id,
        handoff,
        cwd: cwd.map(std::path::PathBuf::from),
    };
    match fm_run::node::run_node(&deps, &run_id, &req, Utc::now) {
        Ok(a) => {
            if json {
                let v = serde_json::json!({"ok": true, "data": {
                    "answer": a.answer, "agent": a.agent, "model": a.model,
                    "transcript": a.transcript.to_string_lossy(),
                }});
                println!("{v}");
            } else {
                println!("{}", a.answer.trim_end());
            }
            0
        }
        Err(e) => {
            let (code, kind) = match &e {
                fm_run::node::NodeError::Refused(_) => (EXIT_REFUSED, "REFUSED"),
                fm_run::node::NodeError::Failed(_) => (20, "FAILED"),
                fm_run::node::NodeError::Stopped(_) => (30, "STOPPED"),
            };
            if json {
                println!(
                    "{}",
                    serde_json::json!({"ok": false, "error": {"code": kind, "message": e.to_string()}})
                );
            } else {
                eprintln!("fm node: {e}");
            }
            code
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_refuses_an_unknown_key() {
        let dir = std::path::Path::new(".");
        let env = HostEnv::isolated(dir, dir, dir);
        let args = FireArgs {
            key: "user:/x:a".into(),
            trigger: Trigger::Manual,
            scheduled_for: None,
            late: false,
            follow: false,
        };
        assert_eq!(fire(&env, &args).unwrap(), EXIT_REFUSED);
    }

    #[test]
    fn the_clock_starts_this_fm_as_a_detached_fire_on_the_same_state_folder() {
        let dir = std::path::Path::new("state");
        let env = HostEnv::isolated(dir, dir, dir);
        let s = fire_starter(&env);
        assert_eq!(s.exe, env.exe);
        assert_eq!(s.log_dir, Some(env.state.logs_dir()));
        assert_eq!(s.env, [("FM_HOME".to_string(), "state".to_string())]);
    }
}

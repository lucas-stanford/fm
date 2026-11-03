//! `on.watch`: a script decides when a run starts. The decisions here are pure; [`run_watch_script`]
//! runs the script through an injectable [`RunScript`].
//!
//! The contract (`watch-scripts.md`): the host runs the script every `every` in the automation
//! folder, with the hook environment and no standard input, for at most the hook timeout capped at
//! two minutes. Exit 0 with output whose fingerprint differs from the one that started the last run
//! starts a run; exit 0 with the same output does nothing; any other exit does nothing and forgets
//! the fingerprint; a timeout, a start failure or a signal does nothing and keeps it. The output is
//! never passed to the run.

use std::time::Duration;

use chrono::{DateTime, Utc};
use fm_model::{Located, Watch, WatchState};
use fm_run::process::{
    AutomationVars, EnvMode, ProcessOutcome, ProcessSpec, ScriptContext, Stdin, automation_env,
    format_duration,
};
use fm_spec::{Platform, Which, resolve_interpreter, script_path};
use sha2::{Digest, Sha256};

use fm_state::time::{iso, parse_iso};

/// A check counts as due slightly early, so tick jitter does not add a minute to every interval.
pub const WATCH_SLACK: Duration = Duration::from_secs(30);

/// The longest `said` line kept in state and shown in a listing.
pub const WATCH_SAID_MAX: usize = 200;

/// The longest a watch script may run, whatever the hook timeout: under the tick lock's
/// five-minute staleness.
pub const WATCH_TIMEOUT_MAX: Duration = Duration::from_secs(2 * 60);

/// `every`, parsed. Validation already refused anything this cannot read; 5 minutes otherwise.
pub fn every_of(watch: &Watch) -> Duration {
    fm_spec::parse_duration(&watch.every).unwrap_or(Duration::from_secs(5 * 60))
}

fn chrono_of(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).unwrap_or(chrono::Duration::MAX)
}

/// Is a check due at `now`? Never checked, or a clock that moved backwards, means due.
pub fn watch_due(prev: Option<&WatchState>, watch: &Watch, now: DateTime<Utc>) -> bool {
    let Some(last) = prev.and_then(|p| parse_iso(&p.checked_at)) else {
        return true;
    };
    let elapsed = now - last;
    if elapsed < chrono::Duration::zero() {
        return true;
    }
    elapsed >= chrono_of(every_of(watch)) - chrono_of(WATCH_SLACK)
}

/// When the next check is due; in the past when overdue. Never checked means `now`.
pub fn next_check_at(
    prev: Option<&WatchState>,
    watch: &Watch,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    match prev.and_then(|p| parse_iso(&p.checked_at)) {
        Some(last) if last <= now => last + chrono_of(every_of(watch)),
        _ => now,
    }
}

/// What one run of the script came to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchCheck {
    /// The exit code. `None` when the script never exited; then `problem` says why.
    pub exit: Option<i32>,
    pub problem: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// What a finished check means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchDecision {
    /// Start a run now.
    pub fire: bool,
    /// Why, in a few words.
    pub because: String,
    /// What to remember about this check.
    pub next: WatchState,
}

/// `sha256:<hex>` of the output, surrounding whitespace trimmed.
pub fn fingerprint(output: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(output.trim().as_bytes()))
    )
}

fn lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

fn clip(text: &str) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= WATCH_SAID_MAX {
        return one;
    }
    let mut out: String = one.chars().take(WATCH_SAID_MAX - 1).collect();
    out.push('…');
    out
}

/// One line of what a check said: the first line of standard output after exit 0, otherwise the
/// last line of standard error (or the first of standard output), clipped to 200 characters.
pub fn said_line(check: &WatchCheck) -> Option<String> {
    if let Some(p) = &check.problem {
        return Some(clip(p));
    }
    let out = lines(&check.stdout);
    let first_out = out.first().copied();
    if check.exit == Some(0) {
        return first_out.map(clip);
    }
    lines(&check.stderr).last().copied().or(first_out).map(clip)
}

/// What a finished check means:
/// - exit 0 with output different from the last run's finding: start a run;
/// - exit 0 with the same output: no run;
/// - another exit code: no run, and the last finding is forgotten;
/// - no exit (timeout, signal, start failure): no run, nothing forgotten.
pub fn decide_watch(
    prev: Option<&WatchState>,
    check: &WatchCheck,
    now: DateTime<Utc>,
) -> WatchDecision {
    let now_iso = iso(now);
    let answered = check.problem.is_none() && check.exit.is_some();
    let (exit, problem) = match (&check.problem, check.exit) {
        (Some(p), _) => (None, Some(p.clone())),
        (None, Some(e)) => (Some(e), None),
        (None, None) => (
            None,
            Some("the script ended without an exit code".to_string()),
        ),
    };
    let base = WatchState {
        checked_at: now_iso.clone(),
        answered_at: if answered {
            Some(now_iso.clone())
        } else {
            prev.and_then(|p| p.answered_at.clone())
        },
        exit,
        problem: problem.clone(),
        duration_ms: check.duration_ms,
        said: said_line(check),
        fired_at: prev.and_then(|p| p.fired_at.clone()),
        fired_for: None,
    };

    if !answered {
        return WatchDecision {
            fire: false,
            because: format!("could not check: {}", problem.unwrap_or_default()),
            next: WatchState {
                fired_for: prev.and_then(|p| p.fired_for.clone()),
                ..base
            },
        };
    }
    if check.exit != Some(0) {
        return WatchDecision {
            fire: false,
            because: format!("nothing to do (exit {})", check.exit.unwrap_or_default()),
            next: base,
        };
    }
    let found = fingerprint(&check.stdout);
    if prev.and_then(|p| p.fired_for.as_deref()) == Some(found.as_str()) {
        return WatchDecision {
            fire: false,
            because: "the same as what started the last run".into(),
            next: WatchState {
                fired_for: Some(found),
                ..base
            },
        };
    }
    WatchDecision {
        fire: true,
        because: "found something new".into(),
        next: WatchState {
            fired_at: Some(now_iso),
            fired_for: Some(found),
            ..base
        },
    }
}

/// Runs one process to its end. The real one is [`SystemScripts`]; tests answer without starting
/// anything.
pub trait RunScript: Sync {
    fn run(&self, spec: &ProcessSpec) -> ProcessOutcome;
}

/// Runs scripts with `fm_run::process::run`: bounded time and output, the whole tree killed on a
/// timeout.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemScripts;

impl RunScript for SystemScripts {
    fn run(&self, spec: &ProcessSpec) -> ProcessOutcome {
        fm_run::process::run(spec, |_| {})
    }
}

/// How long a watch script may run: the hook timeout, capped at two minutes.
pub fn watch_timeout(hook_timeout: Duration) -> Duration {
    hook_timeout.min(WATCH_TIMEOUT_MAX)
}

/// The process a watch check runs: the script's interpreter in the automation folder, no standard
/// input, credentials scrubbed, `AUTOMATION_*` with `AUTOMATION_PHASE=watch`.
pub fn watch_spec(
    l: &Located,
    last_checked: Option<&str>,
    timeout: Duration,
    max_output_bytes: u64,
    which: Which<'_>,
) -> Result<ProcessSpec, String> {
    let Some(watch) = &l.automation.on.watch else {
        return Err("this automation has no on.watch".into());
    };
    let abs = script_path(&l.dir, &watch.script);
    let interp = resolve_interpreter(&abs, &watch.script, Platform::current(), which)
        .map_err(|reason| format!("{} {reason}", watch.script))?;
    let vars = AutomationVars::new(
        l,
        ScriptContext::Watch {
            last_checked: last_checked.map(str::to_string),
        },
    );
    let mut spec =
        ProcessSpec::new(interp.program.clone(), l.dir.clone()).args(interp.command_args());
    spec.env_mode = EnvMode::Scrubbed {
        overlay: automation_env(&vars),
    };
    spec.stdin = Stdin::Null;
    spec.timeout = timeout;
    spec.max_output_bytes = max_output_bytes;
    Ok(spec)
}

/// A check that never got an answer from the script.
pub fn problem_check(text: String, duration_ms: u64) -> WatchCheck {
    WatchCheck {
        exit: None,
        problem: Some(text),
        duration_ms,
        ..WatchCheck::default()
    }
}

/// What a finished process means for the watch check of `script` (the path as written).
pub fn check_of(script: &str, outcome: ProcessOutcome, duration_ms: u64) -> WatchCheck {
    let problem = |text: String, out: fm_run::process::Output| WatchCheck {
        exit: None,
        problem: Some(text),
        stdout: out.stdout,
        stderr: out.stderr,
        duration_ms,
    };
    match outcome {
        ProcessOutcome::Exited { exit, output } => WatchCheck {
            exit: Some(exit),
            problem: None,
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms,
        },
        ProcessOutcome::StartFailed { message } => {
            problem_check(format!("{script} could not start — {message}"), duration_ms)
        }
        ProcessOutcome::TimedOut { after, output } => problem(
            format!("{script} timed out after {}", format_duration(after)),
            output,
        ),
        ProcessOutcome::Stopped { output } => problem(format!("{script} was stopped"), output),
        // A signal is not "found work", even though a hook would read it as an exit.
        ProcessOutcome::Signaled { signal, output } => {
            problem(format!("{script} was killed by {signal}"), output)
        }
    }
}

/// Run a prepared watch process and say what it came to.
pub fn run_spec(script: &str, spec: &ProcessSpec, runner: &dyn RunScript) -> WatchCheck {
    let started = std::time::Instant::now();
    let outcome = runner.run(spec);
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    check_of(script, outcome, ms)
}

/// Run `l`'s watch script once and say what it came to. Never fails: a script that could not run
/// is a [`WatchCheck`] with a `problem`.
pub fn run_watch_script(
    l: &Located,
    last_checked: Option<&str>,
    timeout: Duration,
    max_output_bytes: u64,
    which: Which<'_>,
    runner: &dyn RunScript,
) -> WatchCheck {
    let script = l
        .automation
        .on
        .watch
        .as_ref()
        .map(|w| w.script.clone())
        .unwrap_or_default();
    match watch_spec(l, last_checked, timeout, max_output_bytes, which) {
        Ok(spec) => run_spec(&script, &spec, runner),
        Err(e) => problem_check(e, 0),
    }
}

/// The next check for a listing (`Status.next_check`): ISO, `None` while the watch is paused.
pub fn next_check(
    watch: &Watch,
    prev: Option<&WatchState>,
    paused: bool,
    now: DateTime<Utc>,
) -> Option<String> {
    (!paused).then(|| iso(next_check_at(prev, watch, now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-07T12:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ago(ms: i64) -> String {
        iso(now() - chrono::Duration::milliseconds(ms))
    }

    fn five() -> Watch {
        Watch {
            script: "scripts/watch.js".into(),
            every: "5m".into(),
        }
    }

    fn checked(over: impl FnOnce(&mut WatchState)) -> WatchState {
        let mut w = WatchState {
            checked_at: ago(5 * 60_000),
            exit: Some(3),
            duration_ms: 10,
            ..WatchState::default()
        };
        over(&mut w);
        w
    }

    fn ran(over: impl FnOnce(&mut WatchCheck)) -> WatchCheck {
        let mut c = WatchCheck {
            exit: Some(0),
            duration_ms: 12,
            ..WatchCheck::default()
        };
        over(&mut c);
        c
    }

    #[test]
    fn due_when_never_checked() {
        assert!(watch_due(None, &five(), now()));
    }

    #[test]
    fn waits_out_its_interval_less_the_slack() {
        let w = five();
        assert!(!watch_due(
            Some(&checked(|c| c.checked_at = ago(4 * 60_000))),
            &w,
            now()
        ));
        assert!(watch_due(
            Some(&checked(|c| c.checked_at = ago(5 * 60_000 - 30_000))),
            &w,
            now()
        ));
        assert!(watch_due(
            Some(&checked(|c| c.checked_at = ago(5 * 60_000 - 1000))),
            &w,
            now()
        ));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_park_the_watch() {
        assert!(watch_due(
            Some(&checked(|c| c.checked_at = ago(-3_600_000))),
            &five(),
            now()
        ));
    }

    #[test]
    fn says_when_the_next_check_is_due_even_when_past() {
        let w = five();
        assert_eq!(iso(next_check_at(None, &w, now())), iso(now()));
        assert_eq!(
            iso(next_check_at(
                Some(&checked(|c| c.checked_at = ago(60_000))),
                &w,
                now()
            )),
            ago(-4 * 60_000)
        );
        assert_eq!(
            iso(next_check_at(
                Some(&checked(|c| c.checked_at = ago(20 * 60_000))),
                &w,
                now()
            )),
            ago(15 * 60_000)
        );
        // Paused: nothing will run it, so there is no next check.
        assert_eq!(next_check(&w, None, true, now()), None);
        assert_eq!(next_check(&w, None, false, now()), Some(iso(now())));
    }

    #[test]
    fn starts_a_run_the_first_time_it_finds_something() {
        let d = decide_watch(
            None,
            &ran(|c| c.stdout = "PR 12: 2 new comments\n".into()),
            now(),
        );
        assert!(d.fire);
        assert_eq!(d.next.exit, Some(0));
        assert_eq!(d.next.fired_at, Some(iso(now())));
        assert_eq!(d.next.said.as_deref(), Some("PR 12: 2 new comments"));
        assert_eq!(
            d.next.fired_for.as_deref(),
            Some(fingerprint("PR 12: 2 new comments").as_str())
        );
    }

    #[test]
    fn fingerprint_is_sha256_of_trimmed_output() {
        assert_eq!(
            fingerprint("  abc\n"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(fingerprint("x"), fingerprint("\r\n x \t\n"));
    }

    #[test]
    fn does_not_start_the_same_work_twice() {
        let prev = checked(|c| {
            c.exit = Some(0);
            c.fired_at = Some(ago(5 * 60_000));
            c.fired_for = Some(fingerprint("PR 12"));
        });
        let d = decide_watch(Some(&prev), &ran(|c| c.stdout = "  PR 12\n".into()), now());
        assert!(!d.fire);
        assert_eq!(d.because, "the same as what started the last run");
        assert_eq!(d.next.fired_for, Some(fingerprint("PR 12")));
        assert_eq!(d.next.fired_at, Some(ago(5 * 60_000)));
    }

    #[test]
    fn starts_a_run_when_what_it_finds_changes() {
        let prev = checked(|c| {
            c.exit = Some(0);
            c.fired_for = Some(fingerprint("PR 12"));
        });
        assert!(
            decide_watch(
                Some(&prev),
                &ran(|c| c.stdout = "PR 12\nPR 14".into()),
                now()
            )
            .fire
        );
    }

    #[test]
    fn forgets_the_finding_when_a_check_finds_nothing() {
        let fired = checked(|c| {
            c.exit = Some(0);
            c.fired_at = Some(ago(10 * 60_000));
            c.fired_for = Some(fingerprint("x"));
        });
        let nothing = decide_watch(
            Some(&fired),
            &ran(|c| {
                c.exit = Some(3);
                c.stderr = "nothing is waiting on you\n".into();
            }),
            now(),
        );
        assert!(!nothing.fire);
        assert_eq!(nothing.because, "nothing to do (exit 3)");
        assert_eq!(nothing.next.fired_for, None);
        // When the watch last started something survives.
        assert_eq!(nothing.next.fired_at, Some(ago(10 * 60_000)));
        assert_eq!(
            nothing.next.said.as_deref(),
            Some("nothing is waiting on you")
        );
        assert!(decide_watch(Some(&nothing.next), &ran(|c| c.stdout = "x".into()), now()).fire);
    }

    #[test]
    fn never_starts_a_run_when_the_script_could_not_answer() {
        let d = decide_watch(
            None,
            &ran(|c| {
                c.exit = None;
                c.problem = Some("scripts/watch.js timed out after 1m".into());
            }),
            now(),
        );
        assert!(!d.fire);
        assert_eq!(
            d.next.problem.as_deref(),
            Some("scripts/watch.js timed out after 1m")
        );
        assert_eq!(d.next.exit, None);
        assert!(d.because.contains("could not check"));
    }

    #[test]
    fn keeps_the_finding_and_the_last_answer_through_a_check_that_could_not_answer() {
        let fired = checked(|c| {
            c.exit = Some(0);
            c.answered_at = Some(ago(5 * 60_000));
            c.fired_at = Some(ago(5 * 60_000));
            c.fired_for = Some(fingerprint("PR 12"));
        });
        let hiccup = decide_watch(
            Some(&fired),
            &ran(|c| {
                c.exit = None;
                c.problem = Some("scripts/watch.js timed out after 1m".into());
            }),
            now(),
        );
        assert!(!hiccup.fire);
        assert_eq!(hiccup.next.checked_at, iso(now()));
        assert_eq!(hiccup.next.fired_for, Some(fingerprint("PR 12")));
        assert_eq!(hiccup.next.answered_at, Some(ago(5 * 60_000)));
        let later = now() + chrono::Duration::minutes(5);
        assert!(
            !decide_watch(
                Some(&hiccup.next),
                &ran(|c| c.stdout = "PR 12".into()),
                later
            )
            .fire
        );
    }

    #[test]
    fn moves_the_last_answer_on_every_check_that_exited() {
        let old = checked(|c| c.answered_at = Some(ago(10 * 60_000)));
        let a = decide_watch(Some(&old), &ran(|c| c.exit = Some(3)), now());
        assert_eq!(a.next.answered_at, Some(iso(now())));
        let b = decide_watch(Some(&old), &ran(|c| c.stdout = "x".into()), now());
        assert_eq!(b.next.answered_at, Some(iso(now())));
    }

    #[test]
    fn keeps_one_bounded_line_of_what_was_said() {
        assert_eq!(
            said_line(&ran(|c| c.stdout = "\n\nfirst\nsecond".into())).as_deref(),
            Some("first")
        );
        assert_eq!(
            said_line(&ran(|c| {
                c.exit = Some(1);
                c.stdout = "out".into();
                c.stderr = "warn\nthe reason\n".into();
            }))
            .as_deref(),
            Some("the reason")
        );
        assert_eq!(
            said_line(&ran(|c| {
                c.exit = Some(1);
                c.stdout = "only out".into();
            }))
            .as_deref(),
            Some("only out")
        );
        let long = said_line(&ran(|c| c.stdout = "x".repeat(500))).unwrap();
        assert_eq!(long.chars().count(), 200);
        assert!(long.ends_with('…'));
        assert_eq!(said_line(&ran(|_| {})), None);
    }

    #[test]
    fn a_check_with_no_exit_and_no_problem_is_a_problem() {
        let d = decide_watch(None, &ran(|c| c.exit = None), now());
        assert!(!d.fire);
        assert_eq!(
            d.next.problem.as_deref(),
            Some("the script ended without an exit code")
        );
    }

    #[test]
    fn the_timeout_is_the_hook_timeout_capped_at_two_minutes() {
        assert_eq!(watch_timeout(Duration::from_secs(600)), WATCH_TIMEOUT_MAX);
        assert_eq!(
            watch_timeout(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }
}

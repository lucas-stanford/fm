//! `fm serve`: the clock in the foreground, for containers and machines with no scheduler. It
//! ticks at once, then on every minute, until it is stopped. One clock at a time: it is refused
//! while the watcher job is registered.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, DurationRound, Utc};
use fm_model::TickSource;

use crate::tick::{TickContext, tick};
use crate::watcher::watcher_registered;

/// The time, and a way to wait. Injected so tests do not wait a minute.
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
    fn sleep(&self, d: Duration);
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// A clock stopped at one instant, for a status read at a given `now`. Sleeping does nothing.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
    fn sleep(&self, _d: Duration) {}
}

pub struct ServeOptions<'a> {
    /// Set to stop: the loop ends at the next check (at most half a second later).
    pub stop: &'a AtomicBool,
    pub clock: &'a dyn Clock,
    /// One line per eventful tick, and when it starts and stops.
    pub log: &'a mut dyn FnMut(&str),
}

/// The refusal when the watcher job is registered.
pub fn refuse_while_watcher_on() -> String {
    "fm serve: the watcher is already ticking — one clock at a time: fm watcher stop".to_string()
}

/// The next whole minute after `now`.
pub fn next_minute(now: DateTime<Utc>) -> DateTime<Utc> {
    now.duration_trunc(chrono::Duration::minutes(1))
        .unwrap_or(now)
        + chrono::Duration::minutes(1)
}

/// Tick in the foreground until `opts.stop` is set. `ctx.by` is ignored: these ticks are
/// `serve`'s. A failing tick is logged and the loop goes on.
pub fn serve(ctx: &TickContext<'_>, opts: ServeOptions<'_>) -> Result<(), String> {
    if watcher_registered(&ctx.paths) {
        return Err(refuse_while_watcher_on());
    }
    let ctx = TickContext {
        by: TickSource::Serve,
        paths: ctx.paths.clone(),
        home: ctx.home.clone(),
        starter: ctx.starter,
        probe: ctx.probe,
        scripts: ctx.scripts,
        which: ctx.which,
        agent_check: ctx.agent_check,
        roots: ctx.roots.clone(),
        extra_roots: ctx.extra_roots.clone(),
    };
    (opts.log)("fm serve — ticking every minute. Ctrl-C to stop.");
    while !opts.stop.load(Ordering::SeqCst) {
        let now = opts.clock.now();
        if watcher_registered(&ctx.paths) {
            (opts.log)(&refuse_while_watcher_on());
            return Err(refuse_while_watcher_on());
        }
        match tick(&ctx, now) {
            Ok(r) if r.eventful() => {
                (opts.log)(&format!("{} {}", fm_state::time::iso(now), r.summary()));
                for (key, reason) in &r.failed {
                    (opts.log)(&format!("  {key}: could not start — {reason}"));
                }
                for p in &r.problems {
                    (opts.log)(&format!("  {p}"));
                }
            }
            Ok(_) => {}
            Err(e) => (opts.log)(&format!("{} tick failed: {e}", fm_state::time::iso(now))),
        }
        let until = next_minute(now);
        loop {
            if opts.stop.load(Ordering::SeqCst) {
                break;
            }
            let left = (until - opts.clock.now()).to_std().unwrap_or_default();
            if left.is_zero() {
                break;
            }
            opts.clock.sleep(left.min(Duration::from_millis(500)));
        }
    }
    (opts.log)("fm serve — stopped.");
    Ok(())
}

/// [`serve`] with the system clock, printing to standard output, until Ctrl-C.
pub fn serve_until_ctrl_c(ctx: &TickContext<'_>) -> Result<(), String> {
    if watcher_registered(&ctx.paths) {
        return Err(refuse_while_watcher_on());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
        .map_err(|e| format!("fm serve: cannot catch Ctrl-C — {e}"))?;
    let mut log = |line: &str| println!("{line}");
    serve(
        ctx,
        ServeOptions {
            stop: &stop,
            clock: &SystemClock,
            log: &mut log,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_minute_is_on_the_minute() {
        let t = DateTime::parse_from_rfc3339("2026-09-17T12:00:42.250Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            fm_state::time::iso(next_minute(t)),
            "2026-09-17T12:01:00.000Z"
        );
        let on = DateTime::parse_from_rfc3339("2026-09-17T12:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            fm_state::time::iso(next_minute(on)),
            "2026-09-17T12:02:00.000Z"
        );
    }
}

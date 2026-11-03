//! `fm serve` with a fake clock: it ticks at once and then on the minute, says what happened, and
//! refuses to be a second clock.

mod common;

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use common::*;
use fm_clock::{Clock, ServeOptions, serve};
use fm_model::TickSource;

struct Fake<'s> {
    now: Cell<DateTime<Utc>>,
    stop: &'s AtomicBool,
    stop_at: DateTime<Utc>,
}

impl Clock for Fake<'_> {
    fn now(&self) -> DateTime<Utc> {
        self.now.get()
    }
    fn sleep(&self, d: Duration) {
        let next = self.now.get() + chrono::Duration::from_std(d).unwrap();
        self.now.set(next);
        if next >= self.stop_at {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
}

#[test]
fn ticks_at_once_then_on_every_minute_until_stopped() {
    let w = World::new();
    w.write(
        "m",
        "  schedule:\n    cron: \"*/2 * * * *\"\n    timezone: UTC\n",
        &[],
    );
    let key = w.arm("m", at("2026-09-17T11:00:00Z"));
    w.seed_last_tick("2026-09-17T11:59:00.000Z");
    let starter = Starter::ok();
    let scripts = no_scripts();
    let ctx = w.ctx(&starter, &scripts);
    let stop = AtomicBool::new(false);
    let clock = Fake {
        now: Cell::new(at("2026-09-17T12:00:40Z")),
        stop: &stop,
        stop_at: at("2026-09-17T12:05:30Z"),
    };
    let mut lines = Vec::new();
    let mut log = |l: &str| lines.push(l.to_string());
    serve(
        &ctx,
        ServeOptions {
            stop: &stop,
            clock: &clock,
            log: &mut log,
        },
    )
    .unwrap();

    // Ticks at 12:00:40, then 12:01 … 12:05: 12:00, 12:02 and 12:04 each fired once (an even\n    // minute held by its stagger fires on the next tick).
    let fired: Vec<String> = starter
        .calls()
        .into_iter()
        .map(|s| s.scheduled_for.unwrap())
        .collect();
    assert_eq!(fired.len(), 3, "{fired:?}");
    assert!(fired.windows(2).all(|p| p[0] < p[1]));
    assert_eq!(w.state().last_tick_by, Some(TickSource::Serve));
    assert_eq!(lines[0], "fm serve — ticking every minute. Ctrl-C to stop.");
    assert!(lines.iter().any(|l| l.contains("fired 1")), "{lines:?}");
    assert_eq!(lines.last().unwrap(), "fm serve — stopped.");
    assert!(w.records(&key).is_empty());
}

#[test]
fn refuses_while_the_watcher_is_registered() {
    let w = World::new();
    std::fs::write(w.paths.root.join("watcher.json"), "{}").unwrap();
    let starter = Starter::ok();
    let scripts = no_scripts();
    let stop = AtomicBool::new(false);
    let clock = Fake {
        now: Cell::new(at("2026-09-17T12:00:00Z")),
        stop: &stop,
        stop_at: at("2026-09-17T12:10:00Z"),
    };
    let mut log = |_: &str| {};
    let err = serve(
        &w.ctx(&starter, &scripts),
        ServeOptions {
            stop: &stop,
            clock: &clock,
            log: &mut log,
        },
    )
    .unwrap_err();
    assert_eq!(
        err,
        "fm serve: the watcher is already ticking — one clock at a time: fm watcher stop"
    );
    assert!(w.state().last_tick_at.is_none());
}

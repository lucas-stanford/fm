//! The schedule decision: what one tick does about each armed automation with a schedule. Pure:
//! everything it needs is passed in, `now` included.
//!
//! The window of instants a tick looks at starts after the newest of the last tick, the
//! automation's claim, when it was armed, and the catch-up horizon (`now - catch-up-window`), and
//! ends at `now`:
//!
//! - none due: nothing;
//! - due, but none older than the automation's stagger offset: `wait`, and the caller keeps the
//!   instant in the next tick's window;
//! - the newest ready instant within the grace period (measured from the instant plus the
//!   stagger): fire on time; older ready instants are missed;
//! - past the grace period, `catch-up: skip`: every ready instant is missed;
//! - past the grace period, `catch-up: once`: one late run for the newest, the older ones missed,
//!   unless the next regular instant is due within the grace period (then all missed).
//!
//! The same rules as pi-automations' `decideTick`.

use chrono::{DateTime, Duration, Utc};
use fm_model::CatchUp;
use fm_spec::CronSchedule;

/// The most instants one decision walks. Every minute for a year and more: unreachable with any
/// sensible `catch-up-window`, and a bound on the work if one is not sensible.
pub const MAX_DUE_INSTANTS: usize = 1_000_000;

/// What a tick should do about one automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing is due.
    None,
    /// Due, but still inside its stagger offset. The tick must not move `lastTickAt` past it.
    Wait,
    /// Start a run for `scheduled_for`.
    Fire,
    /// Record `scheduled_for` (and older instants) as missed; start nothing.
    Missed,
}

/// The decision for one automation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub key: String,
    pub action: Action,
    /// The cron instant (not the staggered one) the run or the missed record is for.
    pub scheduled_for: Option<DateTime<Utc>>,
    /// The run starts past its grace period (`catch-up: once`).
    pub late: bool,
    /// How many due instants will never run. For `Missed` this includes `scheduled_for`.
    pub missed_count: u32,
    /// The newest of the missed instants.
    pub missed_until: Option<DateTime<Utc>>,
    /// The earliest instant still held back by its stagger, on any decision.
    pub waiting_from: Option<DateTime<Utc>>,
}

impl Decision {
    fn none(key: &str) -> Decision {
        Decision {
            key: key.to_string(),
            action: Action::None,
            scheduled_for: None,
            late: false,
            missed_count: 0,
            missed_until: None,
            waiting_from: None,
        }
    }
}

/// One armed automation with a schedule.
#[derive(Debug, Clone)]
pub struct ScheduleInput {
    pub key: String,
    pub schedule: CronSchedule,
    pub catch_up: CatchUp,
    /// The last instant claimed (by a tick, or by a run that covered it).
    pub claimed: Option<DateTime<Utc>>,
    /// When consent was given. Instants before it were never this host's to run, so they are not
    /// missed. (pi-automations looks back the whole window on a first tick; fm does not.)
    pub armed_at: Option<DateTime<Utc>>,
    /// This automation's fixed offset, in seconds.
    pub stagger_sec: u64,
}

/// Everything one tick decides from.
#[derive(Debug, Clone)]
pub struct TickInputs {
    pub last_tick_at: Option<DateTime<Utc>>,
    /// How late a run may start and still be on time.
    pub grace: Duration,
    /// How far back a tick looks.
    pub catch_up_window: Duration,
    pub automations: Vec<ScheduleInput>,
}

/// The decision for every automation in `inputs`, in the same order.
pub fn decide_tick(inputs: &TickInputs, now: DateTime<Utc>) -> Vec<Decision> {
    inputs
        .automations
        .iter()
        .map(|a| {
            decide_one(
                a,
                inputs.last_tick_at,
                inputs.grace,
                inputs.catch_up_window,
                now,
            )
        })
        .collect()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The decision for one automation. See the module documentation for the rules.
pub fn decide_one(
    a: &ScheduleInput,
    last_tick_at: Option<DateTime<Utc>>,
    grace: Duration,
    catch_up_window: Duration,
    now: DateTime<Utc>,
) -> Decision {
    let horizon = now - catch_up_window;
    let from = [last_tick_at, a.claimed, a.armed_at, Some(horizon)]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(horizon);

    let dues: Vec<DateTime<Utc>> = a
        .schedule
        .instants_between(from, now, MAX_DUE_INSTANTS)
        .into_iter()
        .filter(|d| a.claimed.is_none_or(|c| *d > c))
        .collect();
    if dues.is_empty() {
        return Decision::none(&a.key);
    }

    // Fire the newest instant whose stagger has elapsed, not the newest due one.
    let stagger = Duration::seconds(i64::try_from(a.stagger_sec).unwrap_or(i64::MAX / 1000));
    let ready_cutoff = now - stagger;
    let ready: Vec<DateTime<Utc>> = dues
        .iter()
        .copied()
        .filter(|d| *d <= ready_cutoff)
        .collect();
    let waiting_from = dues.iter().copied().find(|d| *d > ready_cutoff);

    let Some(&last) = ready.last() else {
        return Decision {
            action: Action::Wait,
            scheduled_for: waiting_from,
            waiting_from,
            ..Decision::none(&a.key)
        };
    };
    let older = ready.len() - 1;
    let previous = (older > 0).then(|| ready[ready.len() - 2]);

    // Lateness is measured from the cron instant plus the stagger offset.
    let lateness = now - last - stagger;

    let fire = |late: bool| Decision {
        key: a.key.clone(),
        action: Action::Fire,
        scheduled_for: Some(last),
        late,
        missed_count: count(older),
        missed_until: previous,
        waiting_from,
    };
    let missed = || Decision {
        key: a.key.clone(),
        action: Action::Missed,
        scheduled_for: Some(last),
        late: false,
        missed_count: count(older + 1),
        missed_until: Some(last),
        waiting_from,
    };

    if lateness <= grace {
        return fire(false);
    }
    if a.catch_up == CatchUp::Skip {
        return missed();
    }
    // catch-up: once, unless the next regular instant is about to fire anyway.
    let crowded = a
        .schedule
        .next_after(now)
        .is_some_and(|upcoming| upcoming - now < grace);
    if crowded {
        return missed();
    }
    fire(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE_MIN: i64 = 10;
    const WINDOW_DAYS: i64 = 7;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn iso(d: Option<DateTime<Utc>>) -> String {
        d.map(fm_state::time::iso).unwrap_or_default()
    }

    struct Case {
        cron: &'static str,
        catch_up: CatchUp,
        now: &'static str,
        last_tick_at: Option<&'static str>,
        claimed: Option<&'static str>,
        armed_at: Option<&'static str>,
        stagger: u64,
        window_days: i64,
    }

    impl Default for Case {
        fn default() -> Self {
            Case {
                cron: "0 * * * *",
                catch_up: CatchUp::Skip,
                now: "2026-09-17T12:00:30.000Z",
                last_tick_at: None,
                claimed: None,
                armed_at: None,
                stagger: 0,
                window_days: WINDOW_DAYS,
            }
        }
    }

    fn decide(c: Case) -> Decision {
        let input = ScheduleInput {
            key: "user:/r:a".into(),
            schedule: CronSchedule::new(c.cron, Some("UTC")).unwrap(),
            catch_up: c.catch_up,
            claimed: c.claimed.map(at),
            armed_at: c.armed_at.map(at),
            stagger_sec: c.stagger,
        };
        decide_one(
            &input,
            c.last_tick_at.map(at),
            Duration::minutes(GRACE_MIN),
            Duration::days(c.window_days),
            at(c.now),
        )
    }

    #[test]
    fn fires_on_time_not_late() {
        let d = decide(Case {
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert!(!d.late);
        assert_eq!(iso(d.scheduled_for), "2026-09-17T12:00:00.000Z");
        assert_eq!(d.missed_count, 0);
    }

    #[test]
    fn does_nothing_when_no_instant_is_due() {
        let d = decide(Case {
            now: "2026-09-17T12:30:00.000Z",
            last_tick_at: Some("2026-09-17T12:29:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn holds_a_fire_back_until_the_stagger_has_elapsed_without_losing_it() {
        let early = decide(Case {
            now: "2026-09-17T12:00:30.000Z",
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            stagger: 47,
            ..Case::default()
        });
        assert_eq!(early.action, Action::Wait);
        assert_eq!(iso(early.scheduled_for), "2026-09-17T12:00:00.000Z");
        assert_eq!(iso(early.waiting_from), "2026-09-17T12:00:00.000Z");

        let ready = decide(Case {
            now: "2026-09-17T12:00:48.000Z",
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            stagger: 47,
            ..Case::default()
        });
        assert_eq!(ready.action, Action::Fire);
        assert!(!ready.late);
        // The record says the cron instant, not the staggered one.
        assert_eq!(iso(ready.scheduled_for), "2026-09-17T12:00:00.000Z");
    }

    #[test]
    fn still_on_time_at_the_very_end_of_the_grace_period() {
        let d = decide(Case {
            now: "2026-09-17T12:10:00.000Z",
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert!(!d.late);
    }

    #[test]
    fn grace_is_measured_from_the_instant_plus_the_stagger() {
        // 12:10:20 is 20s past grace from 12:00, but within it from 12:00 + 30s.
        let d = decide(Case {
            now: "2026-09-17T12:10:20.000Z",
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            stagger: 30,
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert!(!d.late);
    }

    #[test]
    fn records_a_missed_instant_past_grace_when_catch_up_is_skip() {
        let d = decide(Case {
            now: "2026-09-17T12:11:00.000Z",
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Missed);
        assert_eq!(iso(d.missed_until), "2026-09-17T12:00:00.000Z");
        assert_eq!(iso(d.scheduled_for), "2026-09-17T12:00:00.000Z");
        // The instant being reported is itself missed, so the count includes it.
        assert_eq!(d.missed_count, 1);
    }

    #[test]
    fn runs_one_late_instance_past_grace_when_catch_up_is_once() {
        let d = decide(Case {
            cron: "0 7 * * *",
            catch_up: CatchUp::Once,
            now: "2026-09-17T09:14:00.000Z",
            last_tick_at: Some("2026-09-17T06:00:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert!(d.late);
        assert_eq!(iso(d.scheduled_for), "2026-09-17T07:00:00.000Z");
        assert_eq!(d.missed_count, 0);
    }

    #[test]
    fn collapses_several_missed_instants_into_one_fire_and_a_count() {
        let d = decide(Case {
            catch_up: CatchUp::Once,
            now: "2026-09-17T12:05:00.000Z",
            last_tick_at: Some("2026-09-17T08:30:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert!(!d.late);
        assert_eq!(iso(d.scheduled_for), "2026-09-17T12:00:00.000Z");
        assert_eq!(d.missed_count, 3);
        assert_eq!(iso(d.missed_until), "2026-09-17T11:00:00.000Z");
    }

    #[test]
    fn skip_reports_every_missed_instant_in_one_count() {
        let d = decide(Case {
            now: "2026-09-17T12:30:00.000Z",
            last_tick_at: Some("2026-09-17T08:30:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Missed);
        assert_eq!(d.missed_count, 4);
        assert_eq!(iso(d.missed_until), "2026-09-17T12:00:00.000Z");
    }

    #[test]
    fn clamps_a_long_outage_to_the_catch_up_window() {
        let d = decide(Case {
            cron: "0 7 * * *",
            catch_up: CatchUp::Once,
            now: "2026-09-30T09:00:00.000Z",
            last_tick_at: Some("2026-09-17T06:00:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert!(d.late);
        assert_eq!(iso(d.scheduled_for), "2026-09-30T07:00:00.000Z");
        // Six older daily instants inside the window are missed, not run.
        assert_eq!(d.missed_count, 6);
    }

    #[test]
    fn refuses_a_late_run_when_the_next_regular_instant_is_about_to_fire() {
        // Missed 12:00, it is 12:55, and 13:00 is five minutes away.
        let d = decide(Case {
            catch_up: CatchUp::Once,
            now: "2026-09-17T12:55:00.000Z",
            last_tick_at: Some("2026-09-17T11:30:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Missed);
        assert_eq!(d.missed_count, 1);
    }

    #[test]
    fn does_nothing_when_the_instant_is_already_claimed() {
        let d = decide(Case {
            last_tick_at: Some("2026-09-17T11:59:00.000Z"),
            claimed: Some("2026-09-17T12:00:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn fires_the_next_instant_after_a_claim() {
        let d = decide(Case {
            now: "2026-09-17T13:00:30.000Z",
            claimed: Some("2026-09-17T12:00:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert_eq!(iso(d.scheduled_for), "2026-09-17T13:00:00.000Z");
        assert_eq!(d.missed_count, 0);
    }

    #[test]
    fn a_manual_run_claim_stops_catch_up_from_repeating_a_missed_instant() {
        // The host was down at 07:00; a person ran it by hand at 09:00, which claimed 07:00.
        let d = decide(Case {
            cron: "0 7 * * *",
            catch_up: CatchUp::Once,
            now: "2026-09-17T09:14:00.000Z",
            last_tick_at: Some("2026-09-17T06:00:00.000Z"),
            claimed: Some("2026-09-17T07:00:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::None);
    }

    #[test]
    fn does_not_walk_months_of_instants_after_a_long_outage() {
        let d = decide(Case {
            catch_up: CatchUp::Once,
            now: "2026-09-17T12:05:00.000Z",
            last_tick_at: Some("2026-01-01T00:00:00.000Z"),
            ..Case::default()
        });
        assert!(d.missed_count <= 7 * 24);
        assert_eq!(d.missed_count, 7 * 24 - 1);
    }

    #[test]
    fn instants_before_consent_are_not_missed() {
        // A first tick, armed at 11:30: 12:00 fires on time and nothing older counts as missed.
        let d = decide(Case {
            armed_at: Some("2026-09-17T11:30:00.000Z"),
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert_eq!(d.missed_count, 0);
        // Without it, a first tick looks back the whole window, as pi-automations does.
        assert_eq!(decide(Case::default()).missed_count, 7 * 24 - 1);
    }

    #[test]
    fn handles_a_first_ever_tick_with_no_last_tick_and_no_claim() {
        let d = decide(Case::default());
        assert_eq!(d.action, Action::Fire);
    }

    #[test]
    fn the_window_is_bounded_by_catch_up_window() {
        let d = decide(Case {
            catch_up: CatchUp::Skip,
            now: "2026-09-17T12:30:00.000Z",
            last_tick_at: Some("2026-09-10T00:00:00.000Z"),
            window_days: 1,
            ..Case::default()
        });
        assert_eq!(d.action, Action::Missed);
        assert_eq!(d.missed_count, 24);
    }

    #[test]
    fn fires_the_newest_ready_instant_not_the_newest_due_one() {
        // Per-minute, 20s offset, tick 2s past the minute: 12:00 is ready, 12:01 still holds.
        let d = decide(Case {
            cron: "* * * * *",
            now: "2026-09-17T12:01:02.000Z",
            last_tick_at: Some("2026-09-17T11:59:59.999Z"),
            stagger: 20,
            ..Case::default()
        });
        assert_eq!(d.action, Action::Fire);
        assert_eq!(iso(d.scheduled_for), "2026-09-17T12:00:00.000Z");
        assert_eq!(iso(d.waiting_from), "2026-09-17T12:01:00.000Z");
        assert_eq!(d.missed_count, 0);
    }

    #[test]
    fn evaluates_the_cron_in_its_own_zone() {
        // 07:00 in Los Angeles is 14:00 UTC in September (daylight time).
        let input = ScheduleInput {
            key: "k".into(),
            schedule: CronSchedule::new("0 7 * * *", Some("America/Los_Angeles")).unwrap(),
            catch_up: CatchUp::Skip,
            claimed: None,
            armed_at: None,
            stagger_sec: 0,
        };
        let d = decide_one(
            &input,
            Some(at("2026-09-17T13:59:00Z")),
            Duration::minutes(5),
            Duration::days(7),
            at("2026-09-17T14:00:20Z"),
        );
        assert_eq!(d.action, Action::Fire);
        assert_eq!(iso(d.scheduled_for), "2026-09-17T14:00:00.000Z");
    }

    #[test]
    fn decide_tick_keeps_the_order_and_shares_last_tick() {
        let mk = |key: &str, cron: &str| ScheduleInput {
            key: key.into(),
            schedule: CronSchedule::new(cron, Some("UTC")).unwrap(),
            catch_up: CatchUp::Skip,
            claimed: None,
            armed_at: None,
            stagger_sec: 0,
        };
        let inputs = TickInputs {
            last_tick_at: Some(at("2026-09-17T11:59:00Z")),
            grace: Duration::minutes(5),
            catch_up_window: Duration::days(7),
            automations: vec![mk("a", "0 * * * *"), mk("b", "30 * * * *")],
        };
        let ds = decide_tick(&inputs, at("2026-09-17T12:00:30Z"));
        assert_eq!(ds.len(), 2);
        assert_eq!((ds[0].key.as_str(), ds[0].action), ("a", Action::Fire));
        assert_eq!((ds[1].key.as_str(), ds[1].action), ("b", Action::None));
    }
}

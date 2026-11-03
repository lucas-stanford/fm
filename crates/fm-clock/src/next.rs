//! What a listing shows about the clock: the next fire of a schedule and the next watch check.

use chrono::{DateTime, Utc};
use fm_model::{Schedule, Watch, WatchState};
use fm_spec::{CronSchedule, iso_with_offset, stagger_cap_for, stagger_seconds};

/// This automation's stagger offset in seconds: a hash of its key, at most `stagger-max-seconds`
/// and at most half the schedule's shortest gap.
pub fn stagger_for(
    key: &str,
    schedule: &CronSchedule,
    stagger_max_seconds: u32,
    now: DateTime<Utc>,
) -> u64 {
    stagger_seconds(
        key,
        stagger_cap_for(Some(schedule), u64::from(stagger_max_seconds), now),
    )
}

/// When the schedule fires next (`Status.next_fire`): the next cron instant after `now` plus the
/// stagger, as ISO with the schedule zone's offset. `None` when it never fires again.
pub fn next_fire(
    schedule: &Schedule,
    key: &str,
    stagger_max_seconds: u32,
    now: DateTime<Utc>,
) -> Option<String> {
    let cron = CronSchedule::from_schedule(schedule).ok()?;
    let stagger = stagger_for(key, &cron, stagger_max_seconds, now);
    let next = cron.next_after(now)?;
    let at = next + chrono::Duration::seconds(i64::try_from(stagger).unwrap_or(0));
    Some(iso_with_offset(at, cron.zone()))
}

/// The next scheduled instants (the "Scheduled for" list), without stagger, ISO in the zone.
pub fn scheduled_for(schedule: &Schedule, now: DateTime<Utc>, n: usize) -> Vec<String> {
    let Ok(cron) = CronSchedule::from_schedule(schedule) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = now;
    while out.len() < n {
        let Some(next) = cron.next_after(cursor) else {
            break;
        };
        out.push(iso_with_offset(next, cron.zone()));
        cursor = next;
    }
    out
}

/// When the watch checks next (`Status.next_check`); `None` while it is paused.
pub fn next_check(
    watch: &Watch,
    prev: Option<&WatchState>,
    paused: bool,
    now: DateTime<Utc>,
) -> Option<String> {
    crate::watch::next_check(watch, prev, paused, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_model::CatchUp;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn sched(cron: &str, tz: &str) -> Schedule {
        Schedule {
            cron: cron.into(),
            timezone: Some(tz.into()),
            catch_up: CatchUp::Skip,
        }
    }

    #[test]
    fn next_fire_adds_the_stagger_and_keeps_the_zone() {
        let s = sched("0 7 * * *", "America/Los_Angeles");
        let now = at("2026-09-17T12:00:00Z");
        let cron = CronSchedule::from_schedule(&s).unwrap();
        let st = stagger_for("user:/r:a", &cron, 30, now);
        assert!(st < 30);
        let got = next_fire(&s, "user:/r:a", 30, now).unwrap();
        assert_eq!(got, format!("2026-09-17T07:00:{st:02}-07:00"));
        assert_eq!(
            next_fire(&s, "user:/r:a", 0, now).as_deref(),
            Some("2026-09-17T07:00:00-07:00")
        );
    }

    #[test]
    fn a_per_minute_schedule_staggers_at_most_half_a_minute() {
        let cron = CronSchedule::new("* * * * *", Some("UTC")).unwrap();
        for k in ["a", "b", "c", "d", "user:/x:y"] {
            assert!(stagger_for(k, &cron, 600, at("2026-09-17T12:00:00Z")) < 30);
        }
    }

    #[test]
    fn scheduled_for_lists_the_next_instants() {
        let s = sched("0 9 * * 1-5", "UTC");
        assert_eq!(
            scheduled_for(&s, at("2026-09-18T10:00:00Z"), 2),
            ["2026-09-21T09:00:00Z", "2026-09-22T09:00:00Z"]
        );
    }
}

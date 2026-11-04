//! The words the screens use for times, spans and schedules. Pure: every function takes `now`.
//! Ported from pi-automations' `words.ts`. Times are shown in the offset of `now`, the reader's.

use chrono::{DateTime, Datelike, Duration, FixedOffset};
use fm_model::{RunRecord, Trigger};

pub type Now = DateTime<FixedOffset>;

pub fn parse(iso: &str) -> Option<Now> {
    DateTime::parse_from_rfc3339(iso).ok()
}

fn ms_between(from: &Now, to: &Now) -> i64 {
    (*to - *from).num_milliseconds()
}

/// `45s`, `4m`, `3h`, `1h 20m`, `2d`.
pub fn span_words(ms: i64) -> String {
    let s = (ms.max(0) as f64 / 1000.0).round() as i64;
    if s < 60 {
        return format!("{s}s");
    }
    let m = (s as f64 / 60.0).round() as i64;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    let rest = m % 60;
    if h < 24 {
        return if rest > 0 {
            format!("{h}h {rest}m")
        } else {
            format!("{h}h")
        };
    }
    format!("{}d", h / 24)
}

fn unit(n: i64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// `45 seconds`, `1 hour 20 minutes`: a span in a sentence.
pub fn span_long(ms: i64) -> String {
    let s = (ms.max(0) as f64 / 1000.0).round() as i64;
    if s < 60 {
        return unit(s, "second");
    }
    let m = (s as f64 / 60.0).round() as i64;
    if m < 60 {
        return unit(m, "minute");
    }
    let h = m / 60;
    let rest = m % 60;
    if h < 24 {
        return if rest > 0 {
            format!("{} {}", unit(h, "hour"), unit(rest, "minute"))
        } else {
            unit(h, "hour")
        };
    }
    unit(h / 24, "day")
}

/// `31s ago`, `2m ago`, `3h ago`, `2d ago`.
pub fn ago_words(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let ms = ms_between(&at, now).max(0);
    if ms < 3_600_000 {
        return format!("{} ago", span_words(ms));
    }
    if ms < 86_400_000 {
        return format!("{}h ago", (ms as f64 / 3_600_000.0).round() as i64);
    }
    format!("{}d ago", (ms as f64 / 86_400_000.0).round() as i64)
}

/// `just now`, `3 hours ago`, `2 days ago`: `ago_words` for a sentence.
pub fn ago_long(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let ms = ms_between(&at, now).max(0);
    if ms < 60_000 {
        return "just now".into();
    }
    if ms < 3_600_000 {
        return format!("{} ago", span_long(ms));
    }
    if ms < 86_400_000 {
        return format!(
            "{} ago",
            span_long((ms as f64 / 3_600_000.0).round() as i64 * 3_600_000)
        );
    }
    format!(
        "{} ago",
        span_long((ms as f64 / 86_400_000.0).round() as i64 * 86_400_000)
    )
}

fn local(at: &Now, now: &Now) -> Now {
    at.with_timezone(now.offset())
}

/// `23:23`, on the 24-hour clock every screen uses, in the reader's offset.
pub fn time_words(iso: &str, now: &Now) -> String {
    match parse(iso) {
        Some(at) => local(&at, now).format("%H:%M").to_string(),
        None => iso.to_string(),
    }
}

/// Future times: `in 12m`, `today 14:00`, `tomorrow 09:00`, `Mon 09:00`, `Oct 21 09:00`.
pub fn when_words(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let at = local(&at, now);
    let ms = ms_between(now, &at);
    if ms <= 0 {
        return "now".into();
    }
    if ms < 3_600_000 {
        return format!("in {}", span_words(ms));
    }
    let time = at.format("%H:%M");
    let (day, today) = (at.date_naive(), now.date_naive());
    if day == today {
        return format!("today {time}");
    }
    if Some(day) == today.succ_opt() {
        return format!("tomorrow {time}");
    }
    if ms < (6.5 * 86_400_000.0) as i64 {
        return format!("{} {time}", at.format("%a"));
    }
    format!("{} {time}", at.format("%b %-d"))
}

/// `in 3 minutes`, `today at 22:00`, `Mon at 07:00`: `when_words` for a sentence.
pub fn when_long(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let ms = ms_between(now, &at);
    if ms <= 0 {
        return "now".into();
    }
    if ms < 3_600_000 {
        return format!("in {}", span_long(ms));
    }
    let w = when_words(iso, now);
    match w.rsplit_once(' ') {
        Some((day, time)) if time.len() == 5 && time.as_bytes()[2] == b':' => {
            format!("{day} at {time}")
        }
        _ => w,
    }
}

/// `today 09:00`, `tomorrow 09:00`, `Mon 09:00`: a scheduled instant, never a countdown.
pub fn day_time_words(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let local_at = local(&at, now);
    if local_at.date_naive() == now.date_naive() {
        return format!("today {}", local_at.format("%H:%M"));
    }
    when_words(iso, now)
}

/// `at 08:28` today, `yesterday at 08:00`, `Mon at 07:00`, `Oct 10 at 07:00`: when a run started.
pub fn at_words(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let at = local(&at, now);
    let time = at.format("%H:%M");
    let (day, today) = (at.date_naive(), now.date_naive());
    if day == today {
        return format!("at {time}");
    }
    if day.succ_opt() == Some(today) {
        return format!("yesterday at {time}");
    }
    let ms = ms_between(&at, now);
    if ms > 0 && ms < (6.5 * 86_400_000.0) as i64 {
        return format!("{} at {time}", at.format("%a"));
    }
    format!("{} {} at {time}", at.format("%b"), at.day())
}

/// Past times: `26m ago`, then `today 11:38`, `yesterday 18:02`, `Mon 07:00`, `Oct 10`.
pub fn moment_words(iso: &str, now: &Now) -> String {
    let Some(at) = parse(iso) else {
        return iso.to_string();
    };
    let at = local(&at, now);
    let ms = ms_between(&at, now);
    if (0..3_600_000).contains(&ms) {
        return if ms < 60_000 {
            "just now".into()
        } else {
            format!("{} ago", span_words(ms))
        };
    }
    let time = at.format("%H:%M");
    let (day, today) = (at.date_naive(), now.date_naive());
    if day == today {
        return format!("today {time}");
    }
    if day.succ_opt() == Some(today) {
        return format!("yesterday {time}");
    }
    if ms > 0 && ms < (6.5 * 86_400_000.0) as i64 {
        return format!("{} {time}", at.format("%a"));
    }
    format!("{} {}", at.format("%b"), at.day())
}

/// `by hand`, `by its schedule`, `by its watch`.
pub fn trigger_words(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Manual => "by hand",
        Trigger::Schedule => "by its schedule",
        Trigger::Watch => "by its watch",
    }
}

/// How long a run took, when the record knows both ends.
pub fn took_ms(rec: &RunRecord) -> Option<i64> {
    let start = parse(rec.started_at.as_deref()?)?;
    let end = parse(rec.ended_at.as_deref()?)?;
    let ms = ms_between(&start, &end);
    (ms >= 0).then_some(ms)
}

/// The median duration of recent ok runs, when there are at least two.
pub fn usual_ms(records: &[RunRecord]) -> Option<i64> {
    let mut took: Vec<i64> = records
        .iter()
        .filter(|r| r.outcome == fm_model::Outcome::Ok)
        .filter_map(took_ms)
        .collect();
    if took.len() < 2 {
        return None;
    }
    took.sort_unstable();
    let mid = took.len() / 2;
    Some(if took.len() % 2 == 1 {
        took[mid]
    } else {
        (took[mid - 1] + took[mid]) / 2
    })
}

/// `3 minutes so far, usually 7. Gives up at 1 hour.`
pub fn running_words(elapsed_ms: i64, usual: Option<i64>, timeout_ms: u64) -> String {
    let so = span_long(elapsed_ms);
    let mut text = format!("{so} so far");
    if let Some(u) = usual {
        let u = span_long(u);
        let unit_of = |w: &str| {
            w.split_once(' ')
                .map(|(_, u)| u.trim_end_matches('s').to_string())
        };
        let simple = |w: &str| w.split(' ').count() == 2;
        if simple(&so) && simple(&u) && unit_of(&so) == unit_of(&u) {
            text.push_str(&format!(", usually {}", u.split(' ').next().unwrap_or("")));
        } else {
            text.push_str(&format!(", usually {u}"));
        }
    }
    text.push('.');
    if timeout_ms > 0 {
        text.push_str(&format!(" Gives up at {}.", span_long(timeout_ms as i64)));
    }
    text
}

/// `weekdays 09:00`, `daily 07:30`, `every 15 min`, `hourly at :05`, or the cron itself.
pub fn schedule_words(cron: &str) -> String {
    let f: Vec<&str> = cron.split_whitespace().collect();
    if f.len() != 5 {
        return format!("cron {cron}");
    }
    let (min, hour, dom, mon, dow) = (f[0], f[1], f[2], f[3], f[4]);
    let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if dom == "*" && mon == "*" && hour == "*" && dow == "*" {
        if let Some(n) = min.strip_prefix("*/") {
            return format!("every {n} min");
        }
        if min == "*" {
            return "every minute".into();
        }
        if num(min) {
            return format!("hourly at :{:02}", min.parse::<u32>().unwrap_or(0));
        }
    }
    if !num(min) || !num(hour) || dom != "*" || mon != "*" {
        return format!("cron {cron}");
    }
    let at = format!(
        "{:02}:{:02}",
        hour.parse::<u32>().unwrap_or(0),
        min.parse::<u32>().unwrap_or(0)
    );
    match dow {
        "*" => format!("daily {at}"),
        "1-5" | "mon-fri" | "MON-FRI" => format!("weekdays {at}"),
        "0,6" | "6,0" | "6-7" | "sat,sun" => format!("weekends {at}"),
        _ => format!("cron {cron}"),
    }
}

/// `today 09:00` is the instant, `weekdays` the cadence: `weekdays 09:00` without its time.
pub fn cadence_words(cron: &str) -> String {
    let w = schedule_words(cron);
    match w.rsplit_once(' ') {
        Some((head, time)) if time.len() == 5 && time.as_bytes()[2] == b':' => head.to_string(),
        _ => w,
    }
}

/// `30m`, `1h 30m` from milliseconds; `none` for zero.
pub fn timeout_words(ms: u64) -> String {
    if ms == 0 {
        "none".into()
    } else {
        span_words(ms as i64)
    }
}

pub fn add_ms(now: &Now, ms: i64) -> Now {
    *now + Duration::milliseconds(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Now {
        parse("2026-10-29T08:30:00-07:00").unwrap()
    }

    #[test]
    fn spans() {
        assert_eq!(span_words(45_000), "45s");
        assert_eq!(span_words(4 * 60_000), "4m");
        assert_eq!(span_words(80 * 60_000), "1h 20m");
        assert_eq!(span_words(3 * 86_400_000), "3d");
        assert_eq!(span_long(60_000), "1 minute");
        assert_eq!(span_long(80 * 60_000), "1 hour 20 minutes");
    }

    #[test]
    fn future_times() {
        let n = now();
        assert_eq!(when_words("2026-10-29T08:42:00-07:00", &n), "in 12m");
        assert_eq!(when_words("2026-10-29T14:00:00-07:00", &n), "today 14:00");
        assert_eq!(
            when_words("2026-10-30T09:00:00-07:00", &n),
            "tomorrow 09:00"
        );
        assert_eq!(when_words("2026-11-02T09:00:00-08:00", &n), "Mon 10:00");
        assert_eq!(when_words("2026-11-20T09:00:00-07:00", &n), "Nov 20 09:00");
        assert_eq!(
            when_long("2026-10-30T09:00:00-07:00", &n),
            "tomorrow at 09:00"
        );
        // Another offset is shown in the reader's.
        assert_eq!(when_words("2026-10-29T21:00:00Z", &n), "today 14:00");
    }

    #[test]
    fn past_times() {
        let n = now();
        assert_eq!(moment_words("2026-10-29T08:29:30-07:00", &n), "just now");
        assert_eq!(moment_words("2026-10-29T08:04:00-07:00", &n), "26m ago");
        assert_eq!(moment_words("2026-10-29T06:00:00-07:00", &n), "today 06:00");
        assert_eq!(
            moment_words("2026-10-28T18:02:00-07:00", &n),
            "yesterday 18:02"
        );
        assert_eq!(moment_words("2026-10-26T07:00:00-07:00", &n), "Mon 07:00");
        assert_eq!(moment_words("2026-10-10T07:00:00-07:00", &n), "Oct 10");
        assert_eq!(ago_words("2026-10-29T02:30:00-07:00", &n), "6h ago");
        assert_eq!(ago_long("2026-10-29T02:30:00-07:00", &n), "6 hours ago");
    }

    #[test]
    fn instants_and_starts() {
        let n = now();
        assert_eq!(
            day_time_words("2026-10-29T09:00:00-07:00", &n),
            "today 09:00"
        );
        assert_eq!(
            day_time_words("2026-10-30T09:00:00-07:00", &n),
            "tomorrow 09:00"
        );
        assert_eq!(at_words("2026-10-29T08:28:00-07:00", &n), "at 08:28");
        assert_eq!(
            at_words("2026-10-28T08:00:00-07:00", &n),
            "yesterday at 08:00"
        );
        assert_eq!(at_words("2026-10-26T08:00:00-07:00", &n), "Mon at 08:00");
        assert_eq!(at_words("2026-10-10T08:00:00-07:00", &n), "Oct 10 at 08:00");
    }

    #[test]
    fn schedules() {
        assert_eq!(schedule_words("0 9 * * 1-5"), "weekdays 09:00");
        assert_eq!(schedule_words("30 7 * * *"), "daily 07:30");
        assert_eq!(schedule_words("*/15 * * * *"), "every 15 min");
        assert_eq!(schedule_words("5 * * * *"), "hourly at :05");
        assert_eq!(schedule_words("0 9 1 * *"), "cron 0 9 1 * *");
        assert_eq!(cadence_words("0 9 * * 1-5"), "weekdays");
    }

    #[test]
    fn running_sentence() {
        assert_eq!(
            running_words(3 * 60_000, Some(7 * 60_000), 3_600_000),
            "3 minutes so far, usually 7. Gives up at 1 hour."
        );
        assert_eq!(running_words(30_000, None, 0), "30 seconds so far.");
    }
}

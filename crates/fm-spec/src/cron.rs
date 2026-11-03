//! Cron: the Draft 0.1.0 grammar is checked here, because cron libraries accept more (`@daily`, `L`,
//! `#`, six fields). `croner` then does only the date arithmetic, daylight saving included: a fixed
//! time in a spring-forward gap fires once just after it, and once on a fall-back day.

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Local, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};

use fm_model::{Schedule, fnv1a};

use crate::refusal::quote;

const FIELD_NAMES: [&str; 5] = ["minute", "hour", "day-of-month", "month", "day-of-week"];
const FIELD_RANGES: [(u32, u32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// A plain number, or a three-letter name in the month and day-of-week fields.
fn parse_value(raw: &str, field: usize) -> Option<u32> {
    let lower = raw.to_ascii_lowercase();
    let names: &[&str] = match field {
        3 => &MONTHS,
        4 => &DAYS,
        _ => &[],
    };
    if let Some(i) = names.iter().position(|n| *n == lower) {
        return Some(if field == 3 { i as u32 + 1 } else { i as u32 });
    }
    if (1..=2).contains(&raw.len()) && raw.chars().all(|c| c.is_ascii_digit()) {
        return raw.parse().ok();
    }
    None
}

fn in_range(v: u32, field: usize) -> bool {
    let (lo, hi) = FIELD_RANGES[field];
    v >= lo && v <= hi
}

fn range_text(field: usize) -> String {
    let (lo, hi) = FIELD_RANGES[field];
    format!("{lo}-{hi}")
}

// Checked per token, after the name lookup, since names like `WED` contain a W.
fn describe_bad_value(token: &str, field: usize) -> String {
    if parse_value(token, field).is_none()
        && let Some(c) = token
            .chars()
            .find(|c| matches!(c.to_ascii_uppercase(), 'L' | 'W' | 'H' | '#' | '?'))
    {
        return format!(
            "{} uses \"{c}\", which Draft 0.1.0 does not define — use *, a value, a range, a list or a step",
            quote(token)
        );
    }
    format!("{} is not a value", quote(token))
}

fn validate_field(text: &str, field: usize) -> Option<String> {
    if text.is_empty() {
        return Some("is empty".to_string());
    }
    for item in text.split(',') {
        if item.is_empty() {
            return Some(format!("{} has an empty list entry", quote(text)));
        }
        let mut parts = item.split('/');
        let spec = parts.next().unwrap_or("");
        let step = parts.next();
        if parts.next().is_some() {
            return Some(format!("{} has more than one step", quote(item)));
        }
        if let Some(step) = step {
            if step.is_empty() || !step.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("{} has a step that is not a number", quote(item)));
            }
            let n: u64 = step.parse().unwrap_or(u64::MAX);
            if n < 1 {
                return Some(format!(
                    "{} has a step of {n} — a step must be 1 or more",
                    quote(item)
                ));
            }
            // A step is only meaningful over a span: `*` or an explicit range.
            if spec != "*" && !spec.contains('-') {
                return Some(format!(
                    "{} applies a step to a single value — use */{n} or a range like 0-30/{n}",
                    quote(item)
                ));
            }
        }
        if spec == "*" {
            continue;
        }
        if spec.contains('-') {
            let bounds: Vec<&str> = spec.split('-').collect();
            if bounds.len() != 2 {
                return Some(format!("{} is not a range", quote(item)));
            }
            let Some(lo) = parse_value(bounds[0], field) else {
                return Some(describe_bad_value(bounds[0], field));
            };
            let Some(hi) = parse_value(bounds[1], field) else {
                return Some(describe_bad_value(bounds[1], field));
            };
            if !in_range(lo, field) {
                return Some(format!(
                    "{} is out of range {}",
                    quote(bounds[0]),
                    range_text(field)
                ));
            }
            if !in_range(hi, field) {
                return Some(format!(
                    "{} is out of range {}",
                    quote(bounds[1]),
                    range_text(field)
                ));
            }
            if lo > hi {
                return Some(format!(
                    "{} runs backwards — write it low to high",
                    quote(spec)
                ));
            }
            continue;
        }
        let Some(v) = parse_value(spec, field) else {
            return Some(describe_bad_value(spec, field));
        };
        if !in_range(v, field) {
            return Some(format!(
                "{} is out of range {}",
                quote(spec),
                range_text(field)
            ));
        }
    }
    None
}

/// Check a cron against the Draft 0.1.0 grammar. `Err` is the message after `on.schedule.cron: `.
pub fn validate_cron(expr: &str) -> Result<(), String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(format!("{} is not a 5-field cron expression", quote(expr)));
    }
    // Nicknames are another notation altogether, so say so rather than counting fields.
    if trimmed.starts_with('@') {
        return Err(format!(
            "{} is a nickname — Draft 0.1.0 takes 5 cron fields, so write \"0 0 * * *\"",
            quote(expr)
        ));
    }
    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("{} is not a 5-field cron expression", quote(expr)));
    }
    for (i, field) in fields.iter().enumerate() {
        if let Some(problem) = validate_field(field, i) {
            return Err(format!(
                "{} is not a valid cron expression — {} {problem}",
                quote(expr),
                FIELD_NAMES[i]
            ));
        }
    }
    Ok(())
}

/// An IANA zone name (`UTC` included), as `on.schedule.timezone` takes.
pub fn parse_zone(name: &str) -> Option<Tz> {
    if name.is_empty() {
        return None;
    }
    Tz::from_str(name).ok()
}

pub fn is_iana_zone(name: &str) -> bool {
    parse_zone(name).is_some()
}

/// Where a cron is evaluated: a named zone, or the host's local zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Local,
    Named(Tz),
}

impl Zone {
    /// `None` is the local zone; an unknown name is an error.
    pub fn from_name(name: Option<&str>) -> Result<Zone, String> {
        match name {
            None => Ok(Zone::Local),
            Some(n) => parse_zone(n).map(Zone::Named).ok_or_else(|| {
                format!("{} is not an IANA zone — try America/Los_Angeles", quote(n))
            }),
        }
    }
}

/// A validated cron, ready for date arithmetic in its zone.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    cron: Cron,
    zone: Zone,
    expr: String,
}

fn search<Z: TimeZone>(
    cron: &Cron,
    zone: &Z,
    from: DateTime<Utc>,
    forward: bool,
    inclusive: bool,
) -> Option<DateTime<Utc>> {
    let start = from.with_timezone(zone);
    let found = if forward {
        cron.find_next_occurrence(&start, inclusive)
    } else {
        cron.find_previous_occurrence(&start, inclusive)
    };
    found.ok().map(|d| d.with_timezone(&Utc))
}

impl CronSchedule {
    /// Validate `expr` (Draft 0.1.0 grammar) and `timezone` (`None` = local), then compile.
    pub fn new(expr: &str, timezone: Option<&str>) -> Result<CronSchedule, String> {
        validate_cron(expr)?;
        let zone = Zone::from_name(timezone)?;
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .dom_and_dow(false)
            .build()
            .parse(expr.trim())
            .map_err(|e| format!("{} is not a valid cron expression — {e}", quote(expr)))?;
        Ok(CronSchedule {
            cron,
            zone,
            expr: expr.trim().to_string(),
        })
    }

    pub fn from_schedule(schedule: &Schedule) -> Result<CronSchedule, String> {
        CronSchedule::new(&schedule.cron, schedule.timezone.as_deref())
    }

    pub fn expr(&self) -> &str {
        &self.expr
    }

    pub fn zone(&self) -> Zone {
        self.zone
    }

    fn find(&self, from: DateTime<Utc>, forward: bool, inclusive: bool) -> Option<DateTime<Utc>> {
        match self.zone {
            Zone::Local => search(&self.cron, &Local, from, forward, inclusive),
            Zone::Named(tz) => search(&self.cron, &tz, from, forward, inclusive),
        }
    }

    /// The first instant strictly after `after`; `None` when the pattern never fires again.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut from = after;
        // croner works in whole seconds; never hand back an instant that is not after `after`.
        for _ in 0..4 {
            let next = self.find(from, true, false)?;
            if next > after {
                return Some(next);
            }
            from = next + chrono::Duration::seconds(1);
        }
        None
    }

    /// The most recent instant at or before `at`.
    pub fn previous_at_or_before(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut from = at;
        for _ in 0..4 {
            let prev = self.find(from, false, true)?;
            if prev <= at {
                return Some(prev);
            }
            from = prev - chrono::Duration::seconds(1);
        }
        None
    }

    /// Every instant in `(after_exclusive, until_inclusive]`, oldest first, at most `max`.
    pub fn instants_between(
        &self,
        after_exclusive: DateTime<Utc>,
        until_inclusive: DateTime<Utc>,
        max: usize,
    ) -> Vec<DateTime<Utc>> {
        let mut out = Vec::new();
        let mut cursor = after_exclusive;
        while out.len() < max {
            match self.next_after(cursor) {
                Some(next) if next <= until_inclusive => {
                    out.push(next);
                    cursor = next;
                }
                _ => break,
            }
        }
        out
    }

    /// The shortest gap between consecutive instants, looking `samples` instants ahead of `from`.
    pub fn period(&self, from: DateTime<Utc>, samples: usize) -> Option<Duration> {
        let mut cursor = self.next_after(from)?;
        let mut smallest: Option<Duration> = None;
        for _ in 0..samples {
            let Some(next) = self.next_after(cursor) else {
                break;
            };
            let gap = (next - cursor).to_std().unwrap_or_default();
            smallest = Some(smallest.map_or(gap, |s| s.min(gap)));
            cursor = next;
        }
        smallest
    }
}

/// The first instant of `expr` in `timezone` (`None` = local) strictly after `after`.
pub fn next_after(
    expr: &str,
    timezone: Option<&str>,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    Ok(CronSchedule::new(expr, timezone)?.next_after(after))
}

/// Every instant of `expr` in `(after_exclusive, until_inclusive]`, oldest first, at most `max`.
pub fn instants_between(
    expr: &str,
    timezone: Option<&str>,
    after_exclusive: DateTime<Utc>,
    until_inclusive: DateTime<Utc>,
    max: usize,
) -> Result<Vec<DateTime<Utc>>, String> {
    Ok(CronSchedule::new(expr, timezone)?.instants_between(after_exclusive, until_inclusive, max))
}

/// Seconds to delay a scheduled fire for this automation key; 0 when staggering is off.
pub fn stagger_seconds(key: &str, max_sec: u64) -> u64 {
    if max_sec == 0 {
        return 0;
    }
    u64::from(fnv1a(key)) % max_sec
}

/// The largest stagger: half the shortest gap, so a per-minute schedule still fires every minute.
pub fn stagger_cap_for(
    schedule: Option<&CronSchedule>,
    configured_max_sec: u64,
    from: DateTime<Utc>,
) -> u64 {
    let Some(period) = schedule.and_then(|s| s.period(from, 5)) else {
        return configured_max_sec;
    };
    configured_max_sec.min(period.as_secs() / 2)
}

/// `+0:47`, how listings show a stagger.
pub fn format_stagger(sec: u64) -> String {
    format!("+{}:{:02}", sec / 60, sec % 60)
}

fn offset_text(seconds: i32) -> String {
    if seconds == 0 {
        return "Z".to_string();
    }
    let sign = if seconds > 0 { '+' } else { '-' };
    let minutes = (seconds.unsigned_abs() + 30) / 60;
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

fn iso_in<Z: TimeZone>(at: DateTime<Utc>, zone: &Z) -> String {
    let local = at.with_timezone(zone);
    let offset = local.offset().fix().local_minus_utc();
    format!(
        "{}{}",
        local.naive_local().format("%Y-%m-%dT%H:%M:%S"),
        offset_text(offset)
    )
}

/// ISO 8601 with the zone's own offset (`Z` for zero): `2026-09-17T07:00:00-07:00`.
pub fn iso_with_offset(at: DateTime<Utc>, zone: Zone) -> String {
    match zone {
        Zone::Local => iso_in(at, &Local),
        Zone::Named(tz) => iso_in(at, &tz),
    }
}

fn naive_in(at: DateTime<Utc>, zone: Zone) -> chrono::NaiveDateTime {
    match zone {
        Zone::Local => at.with_timezone(&Local).naive_local(),
        Zone::Named(tz) => at.with_timezone(&tz).naive_local(),
    }
}

/// Wall-clock `HH:MM` of an instant in a zone.
pub fn clock_in(at: DateTime<Utc>, zone: Zone) -> String {
    naive_in(at, zone).format("%H:%M").to_string()
}

/// The listing's next-fire text: `07:00 +0:47 tomorrow`, `13:00 in 42m`, `09:00 Monday`, or `—`.
pub fn describe_next(schedule: &CronSchedule, from: DateTime<Utc>, stagger_sec: u64) -> String {
    let Some(next) = schedule.next_after(from) else {
        return "—".to_string();
    };
    let zone = schedule.zone();
    let clock = clock_in(next, zone);
    let offset = if stagger_sec > 0 {
        format!(" {}", format_stagger(stagger_sec))
    } else {
        String::new()
    };
    let fire_at = next + chrono::Duration::seconds(stagger_sec as i64);
    let delta = (fire_at - from).num_milliseconds();
    let when = if delta < 60_000 {
        format!("in {}s", ((delta.max(0) as f64) / 1000.0).round() as i64)
    } else if delta < 3_600_000 {
        format!("in {}m", ((delta as f64) / 60_000.0).round() as i64)
    } else {
        let today = naive_in(from, zone).date();
        let target = naive_in(next, zone).date();
        if target == today {
            "today".to_string()
        } else if target == naive_in(from + chrono::Duration::days(1), zone).date() {
            "tomorrow".to_string()
        } else if delta < 7 * 86_400_000 {
            naive_in(next, zone).format("%A").to_string()
        } else {
            naive_in(next, zone).format("%b %-d").to_string()
        }
    };
    format!("{clock}{offset} {when}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn iso(d: DateTime<Utc>) -> String {
        d.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    fn utc(expr: &str) -> CronSchedule {
        CronSchedule::new(expr, Some("UTC")).unwrap()
    }

    fn la(expr: &str) -> CronSchedule {
        CronSchedule::new(expr, Some("America/Los_Angeles")).unwrap()
    }

    #[test]
    fn accepts_the_specification_examples() {
        for e in [
            "0 7 * * MON-FRI",
            "0 */4 * * MON-FRI",
            "0 9 * * MON",
            "0 8 * * MON-FRI",
            "*/15 * * * *",
            "0 0 * * *",
            "* * * * *",
            "0 0 1 JAN *",
            "0 0 1 jan *",
            "0 7 * * SUN",
            "0 7 * * sat",
            "0 7 * * WED",
            "0 7 * * THU",
            "0 0 1 JUL *",
            "0 7 * * MON,WED,FRI",
            "0 0 1 JUL-AUG *",
            "0,30 * * * *",
            "0-30 * * * *",
            "0-30/5 * * * *",
            "0 9,17 * * MON-FRI",
            "0 0 1,15 * *",
            "0 7 * * 7",
            "  0 7 * * *  ",
        ] {
            assert_eq!(validate_cron(e), Ok(()), "{e}");
        }
    }

    #[test]
    fn refuses_with_the_specification_wording() {
        assert_eq!(
            validate_cron("0 7 * * MON-FRI *").unwrap_err(),
            "\"0 7 * * MON-FRI *\" is not a 5-field cron expression"
        );
        assert_eq!(
            validate_cron("0 7 * *").unwrap_err(),
            "\"0 7 * *\" is not a 5-field cron expression"
        );
        assert_eq!(
            validate_cron("").unwrap_err(),
            "\"\" is not a 5-field cron expression"
        );
        let nick = validate_cron("@daily").unwrap_err();
        assert!(nick.contains("is a nickname") && nick.contains("\"0 0 * * *\""));
        assert_eq!(
            validate_cron("60 * * * *").unwrap_err(),
            "\"60 * * * *\" is not a valid cron expression — minute \"60\" is out of range 0-59"
        );
        assert!(validate_cron("* 24 * * *").unwrap_err().contains("hour"));
        assert!(
            validate_cron("* * 0 * *")
                .unwrap_err()
                .contains("day-of-month")
        );
        assert!(validate_cron("* * * 13 *").unwrap_err().contains("month"));
        assert!(
            validate_cron("* * * * 8")
                .unwrap_err()
                .contains("day-of-week")
        );
    }

    #[test]
    fn refuses_the_extended_vocabulary() {
        for e in [
            "0 0 L * *",
            "0 7 * * 1#2",
            "0 7 ? * *",
            "0 0 1W * *",
            "0 H * * *",
        ] {
            assert!(
                validate_cron(e)
                    .unwrap_err()
                    .contains("Draft 0.1.0 does not define"),
                "{e}"
            );
        }
        assert!(validate_cron("MON 7 * * *").unwrap_err().contains("minute"));
        assert!(validate_cron("0 JAN * * *").unwrap_err().contains("hour"));
        assert!(validate_cron("0 7 * MON *").unwrap_err().contains("month"));
        assert!(
            validate_cron("0 7 * * JAN")
                .unwrap_err()
                .contains("day-of-week")
        );
        assert!(
            validate_cron("*/0 * * * *")
                .unwrap_err()
                .contains("step must be 1 or more")
        );
        assert!(
            validate_cron("*/ * * * *")
                .unwrap_err()
                .contains("step that is not a number")
        );
        assert!(
            validate_cron("5/15 * * * *")
                .unwrap_err()
                .contains("applies a step to a single value")
        );
        assert!(
            validate_cron("30-10 * * * *")
                .unwrap_err()
                .contains("runs backwards")
        );
        assert!(
            validate_cron("0,,30 * * * *")
                .unwrap_err()
                .contains("empty list entry")
        );
        for e in [
            "@daily",
            "0 0 L * *",
            "60 * * * *",
            "0 7 * *",
            "30-10 * * * *",
        ] {
            assert!(!validate_cron(e).unwrap_err().contains('\n'));
        }
    }

    #[test]
    fn zones_are_iana_names() {
        assert!(is_iana_zone("UTC"));
        assert!(is_iana_zone("America/Los_Angeles"));
        assert!(is_iana_zone("Europe/Berlin"));
        assert!(!is_iana_zone("PST"));
        assert!(!is_iana_zone("Mars/Olympus"));
        assert!(!is_iana_zone(""));
        assert!(CronSchedule::new("0 7 * * *", Some("PST")).is_err());
    }

    #[test]
    fn previous_is_at_or_before() {
        let s = utc("0 7 * * *");
        let p = |at: &str| s.previous_at_or_before(t(at)).map(iso);
        assert_eq!(
            p("2026-09-17T09:00:00Z").as_deref(),
            Some("2026-09-17T07:00:00Z")
        );
        assert_eq!(
            p("2026-09-17T07:00:00Z").as_deref(),
            Some("2026-09-17T07:00:00Z")
        );
        assert_eq!(
            p("2026-09-17T06:59:00Z").as_deref(),
            Some("2026-09-16T07:00:00Z")
        );
        assert_eq!(
            utc("* * * * *")
                .previous_at_or_before(t("2026-09-17T12:00:30Z"))
                .map(iso)
                .as_deref(),
            Some("2026-09-17T12:00:00Z")
        );
        assert_eq!(
            utc("0 9 * * MON")
                .previous_at_or_before(t("2026-09-20T12:00:00Z"))
                .map(iso)
                .as_deref(),
            Some("2026-09-14T09:00:00Z")
        );
    }

    #[test]
    fn a_pattern_that_never_fires_has_no_instants() {
        let s = utc("0 0 30 FEB *");
        assert_eq!(s.next_after(t("2026-09-17T12:00:00Z")), None);
        assert_eq!(s.previous_at_or_before(t("2026-09-17T12:00:00Z")), None);
        assert_eq!(describe_next(&s, t("2026-09-17T12:00:00Z"), 0), "—");
    }

    #[test]
    fn next_is_strictly_after() {
        let s = utc("0 7 * * *");
        assert_eq!(
            iso(s.next_after(t("2026-09-17T07:00:00Z")).unwrap()),
            "2026-09-18T07:00:00Z"
        );
        assert_eq!(
            iso(s.next_after(t("2026-09-17T06:59:59.500Z")).unwrap()),
            "2026-09-17T07:00:00Z"
        );
        assert_eq!(
            iso(s.next_after(t("2026-09-17T07:00:00.500Z")).unwrap()),
            "2026-09-18T07:00:00Z"
        );
    }

    #[test]
    fn evaluates_in_the_schedule_zone() {
        let from = t("2026-09-17T00:00:00Z");
        assert_eq!(
            iso(la("0 7 * * *").next_after(from).unwrap()),
            "2026-09-17T14:00:00Z"
        );
        assert_eq!(
            iso(utc("0 7 * * *").next_after(from).unwrap()),
            "2026-09-17T07:00:00Z"
        );
        assert_eq!(
            next_after("0 7 * * *", Some("UTC"), from).unwrap(),
            Some(t("2026-09-17T07:00:00Z"))
        );
        assert!(next_after("@daily", None, from).is_err());
    }

    #[test]
    fn day_fields_are_ored_as_in_posix() {
        let days: Vec<String> = utc("0 0 13 * FRI")
            .instants_between(t("2026-02-01T00:00:00Z"), t("2026-02-21T00:00:00Z"), 3)
            .into_iter()
            .map(|d| d.format("%Y-%m-%d").to_string())
            .collect();
        assert_eq!(days, ["2026-02-06", "2026-02-13", "2026-02-20"]);
    }

    #[test]
    fn instants_between_is_exclusive_then_inclusive() {
        let s = utc("0 * * * *");
        let got: Vec<String> = s
            .instants_between(t("2026-09-17T01:00:00Z"), t("2026-09-17T04:00:00Z"), 100)
            .into_iter()
            .map(iso)
            .collect();
        assert_eq!(
            got,
            [
                "2026-09-17T02:00:00Z",
                "2026-09-17T03:00:00Z",
                "2026-09-17T04:00:00Z"
            ]
        );
        let at = t("2026-09-17T01:00:00Z");
        assert!(s.instants_between(at, at, 100).is_empty());
        assert!(
            s.instants_between(t("2026-09-17T05:00:00Z"), at, 100)
                .is_empty()
        );
        assert_eq!(
            utc("* * * * *")
                .instants_between(t("2026-01-01T00:00:00Z"), t("2027-01-01T00:00:00Z"), 5)
                .len(),
            5
        );
        let daily = utc("0 7 * * *");
        assert_eq!(
            daily
                .instants_between(t("2026-09-17T07:00:00Z"), t("2026-09-18T07:00:00Z"), 100)
                .into_iter()
                .map(iso)
                .collect::<Vec<_>>(),
            ["2026-09-18T07:00:00Z"]
        );
    }

    #[test]
    fn spring_forward_fires_once_a_day_at_three_on_the_missing_hour() {
        let s = la("0 2 * * *");
        let ticks = s.instants_between(
            t("2026-03-06T00:00:00-08:00"),
            t("2026-03-11T00:00:00-07:00"),
            100,
        );
        assert_eq!(
            ticks.iter().map(|d| iso(*d)).collect::<Vec<_>>(),
            [
                "2026-03-06T10:00:00Z",
                "2026-03-07T10:00:00Z",
                "2026-03-08T10:00:00Z",
                "2026-03-09T09:00:00Z",
                "2026-03-10T09:00:00Z",
            ]
        );
        let zone = s.zone();
        assert_eq!(
            ticks.iter().map(|d| clock_in(*d, zone)).collect::<Vec<_>>(),
            ["02:00", "02:00", "03:00", "02:00", "02:00"]
        );
    }

    #[test]
    fn fall_back_fires_once_on_the_repeated_hour() {
        let s = la("0 2 * * *");
        let ticks = s.instants_between(
            t("2026-10-30T00:00:00-07:00"),
            t("2026-11-04T00:00:00-08:00"),
            100,
        );
        assert_eq!(ticks.len(), 5);
        assert!(ticks.iter().all(|d| clock_in(*d, s.zone()) == "02:00"));

        let half_past_one = la("30 1 * * *");
        let ticks = half_past_one.instants_between(
            t("2026-10-31T12:00:00-07:00"),
            t("2026-11-01T12:00:00-08:00"),
            100,
        );
        assert_eq!(
            ticks.iter().map(|d| iso(*d)).collect::<Vec<_>>(),
            ["2026-11-01T08:30:00Z"]
        );
    }

    #[test]
    fn interval_schedules_follow_real_time_across_both_transitions() {
        let s = la("*/30 * * * *");
        let fall = s.instants_between(
            t("2026-11-01T00:45:00-07:00"),
            t("2026-11-01T02:00:00-08:00"),
            100,
        );
        assert_eq!(
            fall.iter().map(|d| iso(*d)).collect::<Vec<_>>(),
            [
                "2026-11-01T08:00:00Z",
                "2026-11-01T08:30:00Z",
                "2026-11-01T09:00:00Z",
                "2026-11-01T09:30:00Z",
                "2026-11-01T10:00:00Z",
            ]
        );
        let spring = s.instants_between(
            t("2026-03-08T01:15:00-08:00"),
            t("2026-03-08T03:30:00-07:00"),
            100,
        );
        assert_eq!(
            spring.iter().map(|d| iso(*d)).collect::<Vec<_>>(),
            [
                "2026-03-08T09:30:00Z",
                "2026-03-08T10:00:00Z",
                "2026-03-08T10:30:00Z",
            ]
        );
    }

    #[test]
    fn period_is_the_shortest_gap() {
        let from = t("2026-09-17T12:00:00Z");
        let p = |e: &str| utc(e).period(from, 5).map(|d| d.as_secs());
        assert_eq!(p("* * * * *"), Some(60));
        assert_eq!(p("0 * * * *"), Some(3_600));
        assert_eq!(p("*/15 * * * *"), Some(900));
        assert_eq!(p("0 7 * * MON-FRI"), Some(86_400));
    }

    #[test]
    fn stagger_stays_inside_the_schedule_slot() {
        let from = t("2026-09-17T12:00:00Z");
        assert_eq!(stagger_cap_for(Some(&utc("* * * * *")), 120, from), 30);
        assert_eq!(stagger_cap_for(Some(&utc("*/2 * * * *")), 120, from), 60);
        assert_eq!(stagger_cap_for(Some(&utc("0 * * * *")), 120, from), 120);
        assert_eq!(stagger_cap_for(None, 120, from), 120);
        for e in ["* * * * *", "*/2 * * * *", "*/5 * * * *", "0 * * * *"] {
            let s = utc(e);
            let period = s.period(from, 5).unwrap().as_secs();
            let cap = stagger_cap_for(Some(&s), 120, from);
            for key in ["a", "b", "user:/r:x", "repository:/r:y"] {
                assert!(stagger_seconds(key, cap) < period, "{e} {key}");
            }
        }
        assert_eq!(stagger_seconds("a", 0), 0);
        assert_eq!(format_stagger(47), "+0:47");
        assert_eq!(format_stagger(72), "+1:12");
    }

    #[test]
    fn describes_the_next_fire() {
        let from = t("2026-09-17T12:00:00Z");
        assert_eq!(
            describe_next(&utc("0 7 * * *"), from, 47),
            "07:00 +0:47 tomorrow"
        );
        assert_eq!(describe_next(&utc("0 7 * * *"), from, 0), "07:00 tomorrow");
        assert_eq!(
            describe_next(&utc("0 * * * *"), t("2026-09-17T12:19:00Z"), 72),
            "13:00 +1:12 in 42m"
        );
        assert_eq!(describe_next(&utc("0 23 * * *"), from, 0), "23:00 today");
        assert_eq!(describe_next(&utc("0 9 * * MON"), from, 0), "09:00 Monday");
        assert!(describe_next(&la("0 7 * * *"), from, 0).starts_with("07:00"));
    }

    #[test]
    fn iso_carries_the_zone_offset() {
        let la_zone = Zone::from_name(Some("America/Los_Angeles")).unwrap();
        let utc_zone = Zone::from_name(Some("UTC")).unwrap();
        assert_eq!(
            iso_with_offset(t("2026-09-17T14:00:00Z"), la_zone),
            "2026-09-17T07:00:00-07:00"
        );
        assert_eq!(
            iso_with_offset(t("2026-09-17T07:00:00Z"), utc_zone),
            "2026-09-17T07:00:00Z"
        );
        assert_eq!(
            iso_with_offset(t("2026-12-17T15:00:00Z"), la_zone),
            "2026-12-17T07:00:00-08:00"
        );
        let at = t("2026-09-17T14:00:00Z");
        for z in [
            "UTC",
            "America/Los_Angeles",
            "Europe/Berlin",
            "Asia/Kolkata",
        ] {
            let zone = Zone::from_name(Some(z)).unwrap();
            assert_eq!(t(&iso_with_offset(at, zone).replace('Z', "+00:00")), at);
        }
        assert_eq!(
            t(&iso_with_offset(at, Zone::Local).replace('Z', "+00:00")),
            at
        );
    }
}

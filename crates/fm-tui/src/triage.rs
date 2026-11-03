//! Sorting automations into the list page's sections, the one phrase each row says, and what a
//! person may do to each. Mirrors pi-automations' `list.ts` (`triage`, `classify`, `actionsFor`).

use std::cmp::Ordering;

use fm_model::{AutomationState, LiveRun, Outcome, Scope, Status, StepStatus};

use crate::text::one_line;
use crate::theme::{SectionTone, UiColor};
use crate::words::{
    Now, ago_words, day_time_words, parse, span_words, time_words, trigger_words, when_words,
};

pub type Section = SectionTone;

pub const SECTION_ORDER: [Section; 5] = [
    Section::Needs,
    Section::Running,
    Section::Coming,
    Section::Watching,
    Section::Off,
];

pub fn section_title(s: Section) -> &'static str {
    match s {
        Section::Needs => "Needs you",
        Section::Running => "Running now",
        Section::Coming => "Coming up",
        Section::Watching => "Watching",
        Section::Off => "Off and disabled",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Invalid,
    Failed,
    WatchProblem,
    /// Off for a reason other than never being armed: changed since armed, consent expired.
    Consent,
    Running,
    /// On its schedule.
    Coming,
    /// Armed, by hand only.
    Manual,
    Watched,
    Paused,
    Disabled,
    NotArmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub section: Section,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triage {
    pub sections: Vec<(Section, Vec<Entry>)>,
}

impl Triage {
    pub fn order(&self) -> impl Iterator<Item = &Entry> {
        self.sections.iter().flat_map(|(_, e)| e.iter())
    }

    pub fn count(&self, s: Section) -> usize {
        self.sections
            .iter()
            .find(|(id, _)| *id == s)
            .map_or(0, |(_, e)| e.len())
    }
}

/// `not armed` (or no reason): nobody has armed it yet. Anything else is a reason to look again.
pub fn never_armed(reason: Option<&str>) -> bool {
    match reason.map(str::trim) {
        None | Some("") => true,
        Some(r) => r == "not armed" || r == "not armed yet" || r.starts_with("not armed —"),
    }
}

fn watch_only(s: &Status) -> bool {
    s.automation
        .as_ref()
        .is_some_and(|a| a.on.watch.is_some() && a.on.schedule.is_none())
}

pub fn classify(s: &Status) -> (Section, Kind) {
    let a = s.automation.as_ref();
    if s.state == AutomationState::Invalid && s.running.is_none() {
        return (Section::Needs, Kind::Invalid);
    }
    if s.running.is_some() {
        return (Section::Running, Kind::Running);
    }
    if s.state == AutomationState::Armed
        && s.last
            .as_ref()
            .is_some_and(|l| l.outcome == Outcome::Failed)
    {
        return (Section::Needs, Kind::Failed);
    }
    if s.state == AutomationState::Armed
        && a.is_some_and(|a| a.on.watch.is_some())
        && s.watch_paused_at.is_none()
        && s.watch.as_ref().is_some_and(|w| w.problem.is_some())
    {
        return (Section::Needs, Kind::WatchProblem);
    }
    match s.state {
        AutomationState::Off if never_armed(s.reason.as_deref()) => (Section::Off, Kind::NotArmed),
        AutomationState::Off => (Section::Needs, Kind::Consent),
        AutomationState::Disabled => (Section::Off, Kind::Disabled),
        _ if watch_only(s) => {
            if s.watch_paused_at.is_some() {
                (Section::Watching, Kind::Paused)
            } else {
                (Section::Watching, Kind::Watched)
            }
        }
        _ if a.is_some_and(|a| a.on.schedule.is_some()) => (Section::Coming, Kind::Coming),
        _ => (Section::Coming, Kind::Manual),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Num(i64),
    Text(String),
}

fn rank(s: &Status, kind: Kind) -> Vec<Rank> {
    let name = Rank::Text(s.name.clone());
    let ts = |iso: Option<&str>| parse(iso.unwrap_or("")).map(|t| t.timestamp_millis());
    match kind {
        Kind::Invalid => vec![Rank::Num(1), name],
        Kind::Failed => vec![Rank::Num(2), name],
        Kind::WatchProblem => vec![Rank::Num(3), name],
        Kind::Consent => vec![Rank::Num(4), name],
        Kind::Running => vec![
            Rank::Num(ts(s.running.as_ref().map(|r| r.started_at.as_str())).unwrap_or(0)),
            name,
        ],
        Kind::Coming => vec![
            Rank::Num(0),
            Rank::Num(ts(s.next_fire.as_deref()).unwrap_or(i64::MAX)),
            name,
        ],
        Kind::Manual => vec![Rank::Num(2), Rank::Num(0), name],
        _ => vec![name],
    }
}

/// Every automation in its section, sections in pa's order, empty ones left out.
pub fn triage(statuses: &[Status]) -> Triage {
    let mut ranked: Vec<(Entry, Vec<Rank>)> = statuses
        .iter()
        .map(|s| {
            let (section, kind) = classify(s);
            (
                Entry {
                    key: s.key.clone(),
                    section,
                    kind,
                },
                rank(s, kind),
            )
        })
        .collect();
    ranked.sort_by(|a, b| a.1.cmp(&b.1).then(Ordering::Equal));
    let sections = SECTION_ORDER
        .iter()
        .map(|id| {
            (
                *id,
                ranked
                    .iter()
                    .filter(|(e, _)| e.section == *id)
                    .map(|(e, _)| e.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, items)| !items.is_empty())
        .collect();
    Triage { sections }
}

/// `step 3/7`: where a tracked run is on its chart.
pub fn step_of(live: Option<&LiveRun>) -> Option<(usize, usize)> {
    let steps = &live?.progress.as_ref()?.steps;
    if steps.is_empty() {
        return None;
    }
    let at = steps
        .iter()
        .rposition(|s| matches!(s.status, StepStatus::Active | StepStatus::Waiting))
        .or_else(|| steps.iter().rposition(|s| s.status == StepStatus::Done))?;
    Some((at + 1, steps.len()))
}

/// The one phrase a row says after its name, and its colour.
pub fn phrase(
    s: &Status,
    kind: Kind,
    live: Option<&LiveRun>,
    now: &Now,
    dash: &str,
) -> (String, UiColor) {
    let a = s.automation.as_ref();
    let reason = |r: Option<&str>, fallback: &str| {
        r.map(one_line)
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| fallback.into())
    };
    match kind {
        Kind::Invalid => (
            format!(
                "problem in its file {dash} {}",
                reason(s.reason.as_deref(), "it could not be read")
            ),
            UiColor::Error,
        ),
        Kind::Failed => {
            let last = s.last.as_ref();
            let ago = last.map(|l| ago_words(&l.at, now)).unwrap_or_default();
            let why = reason(last.and_then(|l| l.reason.as_deref()), "it did not say why");
            (format!("failed {ago} {dash} {why}"), UiColor::Error)
        }
        Kind::WatchProblem => (
            format!(
                "watch could not check {dash} {}",
                reason(
                    s.watch.as_ref().and_then(|w| w.problem.as_deref()),
                    "its watch script failed"
                )
            ),
            UiColor::Warning,
        ),
        Kind::Consent => (
            format!(
                "off {dash} {}",
                reason(s.reason.as_deref(), "look at it again")
            ),
            UiColor::Warning,
        ),
        Kind::Running => {
            let elapsed = s
                .running
                .as_ref()
                .and_then(|r| parse(&r.started_at))
                .map(|t| span_words((*now - t).num_milliseconds()))
                .unwrap_or_default();
            let tail = match step_of(live) {
                Some((i, n)) => format!("step {i}/{n}"),
                None => s
                    .running
                    .as_ref()
                    .map(|r| format!("started {}", trigger_words(r.trigger)))
                    .unwrap_or_default(),
            };
            (format!("running {elapsed} {dash} {tail}"), UiColor::Accent)
        }
        Kind::Coming => match &s.next_fire {
            Some(at) => (
                format!("Scheduled for {}", day_time_words(at, now)),
                UiColor::Text,
            ),
            None => ("on its schedule".into(), UiColor::Text),
        },
        Kind::Manual => ("when you run it".into(), UiColor::Muted),
        Kind::Watched => {
            let every = a
                .and_then(|a| a.on.watch.as_ref())
                .map(|w| w.every.clone())
                .unwrap_or_default();
            let said = s
                .watch
                .as_ref()
                .and_then(|w| w.said.as_deref())
                .map(one_line)
                .filter(|t| !t.is_empty());
            match said {
                Some(said) => (
                    format!("watching every {every} {dash} said \"{said}\""),
                    UiColor::Text,
                ),
                None => (format!("watching every {every}"), UiColor::Text),
            }
        }
        Kind::Paused => {
            let since = s
                .watch_paused_at
                .as_deref()
                .map(|t| format!(" since {}", time_words(t, now)))
                .unwrap_or_default();
            (format!("watch paused{since}"), UiColor::Warning)
        }
        Kind::Disabled => ("disabled".into(), UiColor::Muted),
        Kind::NotArmed => ("not armed yet".into(), UiColor::Muted),
    }
}

/// Something a person can do to an automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ShowRun,
    Stop,
    Run,
    PauseWatch,
    ResumeWatch,
    Arm,
    Disarm,
    Enable,
    Disable,
    Edit,
    Agent,
    Check,
    Runs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Affordance {
    pub action: Action,
    /// Absent: reachable from the automation page's menu only.
    pub key: Option<&'static str>,
    pub label: String,
}

fn aff(action: Action, key: Option<&'static str>, label: &str) -> Affordance {
    Affordance {
        action,
        key,
        label: label.into(),
    }
}

/// What a person may do to this automation right now, most useful first.
pub fn actions_for(s: &Status) -> Vec<Affordance> {
    let watch: Vec<Affordance> = match s.automation.as_ref().and_then(|a| a.on.watch.as_ref()) {
        Some(_) if s.watch_paused_at.is_some() => {
            vec![aff(Action::ResumeWatch, Some("p"), "resume its watch")]
        }
        Some(_) => vec![aff(Action::PauseWatch, Some("p"), "pause its watch")],
        None => vec![],
    };
    let edit = aff(Action::Edit, Some("e"), "edit it");
    let agent = aff(Action::Agent, Some("a"), "choose its agent");
    let check = aff(Action::Check, Some("c"), "check: every technical detail");
    let runs = aff(Action::Runs, Some("l"), "its runs");
    let mut out = Vec::new();
    if s.running.is_some() {
        out.push(aff(Action::ShowRun, Some("s"), "show its run"));
        out.push(aff(Action::Stop, Some("x"), "stop its run"));
        out.extend(watch);
        out.extend([edit, agent, check]);
        return out;
    }
    match s.state {
        AutomationState::Armed => {
            let failed = s
                .last
                .as_ref()
                .is_some_and(|l| l.outcome == Outcome::Failed);
            out.push(aff(
                Action::Run,
                Some("r"),
                if failed {
                    "run it again now"
                } else {
                    "run now"
                },
            ));
            out.extend(watch);
            out.extend([runs, edit, agent, check]);
            if s.scope == Scope::Repository {
                out.push(aff(Action::Disarm, Some("space"), "disarm it"));
                out.push(aff(Action::Disable, None, "disable it"));
            } else {
                out.push(aff(Action::Disable, Some("space"), "disable it"));
                out.push(aff(Action::Disarm, None, "disarm it"));
            }
        }
        AutomationState::Off => {
            let label = if never_armed(s.reason.as_deref()) {
                "arm it, after a preview"
            } else {
                "look at it, then arm it again"
            };
            out.push(aff(Action::Arm, Some("space"), label));
            out.extend(watch);
            if s.last.is_some() {
                out.push(runs);
            }
            out.extend([edit, agent, check]);
            out.push(aff(Action::Disable, None, "disable it"));
        }
        AutomationState::Disabled => {
            out.push(aff(Action::Enable, Some("space"), "enable it"));
            if s.last.is_some() {
                out.push(runs);
            }
            out.extend([edit, agent, check]);
        }
        AutomationState::Invalid => {
            out.push(aff(Action::Edit, Some("e"), "fix it"));
            out.push(check);
        }
    }
    out
}

/// The key a person presses for `action` on this automation, if it has one.
pub fn key_action(s: &Status, key: &str) -> Option<Action> {
    let actions = actions_for(s);
    if let Some(a) = actions.iter().find(|a| a.key == Some(key)) {
        return Some(a.action);
    }
    // `s` and `l` both open the run page, whichever the row prints.
    match key {
        "s" | "l" if s.state != AutomationState::Invalid || s.running.is_some() => {
            Some(if s.running.is_some() {
                Action::ShowRun
            } else {
                Action::Runs
            })
        }
        _ => None,
    }
}

/// The one answer at the top of the list page.
pub fn status_answer(
    statuses: &[Status],
    tri: &Triage,
    now: &Now,
    dash: &str,
) -> (String, UiColor) {
    if statuses.is_empty() {
        return ("No automations yet".into(), UiColor::Muted);
    }
    let needs = tri.count(Section::Needs);
    if needs > 0 {
        let verb = if needs == 1 { "needs" } else { "need" };
        return (format!("{needs} {verb} you"), UiColor::Error);
    }
    let running = tri.count(Section::Running);
    if running > 0 {
        return (
            format!("Nothing needs you {dash} {running} running now"),
            UiColor::Accent,
        );
    }
    let next = statuses
        .iter()
        .filter(|s| s.state == AutomationState::Armed)
        .filter_map(|s| {
            s.next_fire
                .as_deref()
                .and_then(|t| parse(t).map(|p| (p, t)))
        })
        .min_by_key(|(p, _)| *p);
    match next {
        Some((_, iso)) => (
            format!("All quiet {dash} next run {}", when_words(iso, now)),
            UiColor::Success,
        ),
        None => ("All quiet".into(), UiColor::Success),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeHost;
    use fm_model::Host;

    #[test]
    fn fake_covers_every_section_in_order() {
        let mut host = FakeHost::new();
        let tri = triage(&host.list());
        let ids: Vec<Section> = tri.sections.iter().map(|(s, _)| *s).collect();
        assert_eq!(ids, SECTION_ORDER.to_vec());
        let needs: Vec<Kind> = tri.sections[0].1.iter().map(|e| e.kind).collect();
        assert_eq!(
            needs,
            vec![
                Kind::Invalid,
                Kind::Failed,
                Kind::WatchProblem,
                Kind::Consent
            ]
        );
    }

    #[test]
    fn never_armed_reasons() {
        assert!(never_armed(None));
        assert!(never_armed(Some("not armed")));
        assert!(!never_armed(Some("changed since armed: AUTOMATION.md")));
    }

    #[test]
    fn invalid_offers_only_fix_and_check() {
        let mut host = FakeHost::new();
        let s = host
            .list()
            .into_iter()
            .find(|s| s.state == AutomationState::Invalid)
            .unwrap();
        let keys: Vec<_> = actions_for(&s).iter().map(|a| a.key).collect();
        assert_eq!(keys, vec![Some("e"), Some("c")]);
        assert_eq!(key_action(&s, "r"), None);
    }
}

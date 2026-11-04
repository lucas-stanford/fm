//! The progress store: where a tracked run has got to, reported by the run and by anything it
//! starts (`fm step|next|say|note|message`). One file per run, `progress/<run id>.json`, holding
//! the run's events; folding them gives a [`ProgressSnapshot`], and at the end a
//! [`RunProgress`] for the record.
//!
//! Writes take the run's short lock and check their preconditions inside it, so two writers cannot
//! both move a node; retries converge, and a request id makes `say`/`note`/`message` exactly-once.
//! Writes are atomic, so reads take no lock. A finished run leaves a `<run id>.closed` tombstone so
//! stragglers hear `RUN_CLOSED`; tombstones are swept after a week.
//!
//! The chart is parsed by `fm-spec`: [`open_progress`] is handed its nodes in flow order.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use fm_model::{RunLink, RunNote, RunProgress, RunRecord, StatePaths, StepStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::StateError;
use crate::fsio::{
    LOCK_TIMEOUT, ensure_dir, lock_file, lock_path_for, read_json, write_json_atomic,
};
use crate::time::{iso, parse_iso};

/// How much text one update, note or message keeps. Longer is cut, with a warning, never refused.
pub const TEXT_MAX: usize = 4000;

/// The statuses `fm step --status` may set. `pending` and `interrupted` belong to the run.
pub const SETTABLE: [StepStatus; 5] = [
    StepStatus::Active,
    StepStatus::Waiting,
    StepStatus::Done,
    StepStatus::Failed,
    StepStatus::Skipped,
];

const TOMBSTONE_DAYS: i64 = 7;

fn running(s: StepStatus) -> bool {
    matches!(s, StepStatus::Active | StepStatus::Waiting)
}

fn settled(s: StepStatus) -> bool {
    matches!(
        s,
        StepStatus::Done | StepStatus::Failed | StepStatus::Skipped | StepStatus::Interrupted
    )
}

fn status_name(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Pending => "pending",
        StepStatus::Active => "active",
        StepStatus::Waiting => "waiting",
        StepStatus::Done => "done",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "skipped",
        StepStatus::Interrupted => "interrupted",
    }
}

/// Every way a progress write can be refused, as a stable code a script can branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProgressCode {
    Usage,
    NoRun,
    UnknownRun,
    RunClosed,
    NoChart,
    UnknownNode,
    AmbiguousNode,
    AmbiguousActive,
    NoActive,
    NotAnEdge,
    BadLink,
    LockTimeout,
    Io,
}

impl ProgressCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressCode::Usage => "USAGE",
            ProgressCode::NoRun => "NO_RUN",
            ProgressCode::UnknownRun => "UNKNOWN_RUN",
            ProgressCode::RunClosed => "RUN_CLOSED",
            ProgressCode::NoChart => "NO_CHART",
            ProgressCode::UnknownNode => "UNKNOWN_NODE",
            ProgressCode::AmbiguousNode => "AMBIGUOUS_NODE",
            ProgressCode::AmbiguousActive => "AMBIGUOUS_ACTIVE",
            ProgressCode::NoActive => "NO_ACTIVE",
            ProgressCode::NotAnEdge => "NOT_AN_EDGE",
            ProgressCode::BadLink => "BAD_LINK",
            ProgressCode::LockTimeout => "LOCK_TIMEOUT",
            ProgressCode::Io => "IO",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ProgressError {
    pub code: ProgressCode,
    /// One line.
    pub message: String,
    /// What the writer probably meant: node ids, or where it can go.
    pub suggestions: Vec<String>,
}

fn perr(code: ProgressCode, message: impl Into<String>) -> ProgressError {
    ProgressError {
        code,
        message: message.into(),
        suggestions: Vec::new(),
    }
}

impl From<StateError> for ProgressError {
    fn from(e: StateError) -> Self {
        let code = match e {
            StateError::LockTimeout { .. } => ProgressCode::LockTimeout,
            _ => ProgressCode::Io,
        };
        perr(code, e.to_string())
    }
}

type PResult<T> = std::result::Result<T, ProgressError>;

/// A node of the flowchart, as `fm-spec` gives it, in flow order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartNode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub successors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressNode {
    pub id: String,
    pub label: String,
    pub status: StepStatus,
    /// The latest update, or null.
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub successors: Vec<String>,
}

/// Where a tracked run is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressSnapshot {
    pub run: String,
    pub key: String,
    pub name: String,
    pub chart: String,
    /// `flow: strict`: `next` must follow an arrow, except between hooks and the body.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
    pub opened_at: String,
    pub nodes: Vec<ProgressNode>,
    pub taken: Vec<(String, String)>,
    pub notes: Vec<RunNote>,
    /// The latest message; a later one replaces it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<RunNote>,
    pub closed: bool,
    /// How many events were applied. Moves on every change, so it keys a render cache.
    pub revision: usize,
}

impl ProgressSnapshot {
    pub fn node(&self, id: &str) -> Option<&ProgressNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressResult {
    /// False for a retry of something already recorded: nothing was written.
    pub changed: bool,
    pub warnings: Vec<String>,
    pub snapshot: ProgressSnapshot,
    /// The node the write landed on.
    pub node: Option<ProgressNode>,
    /// For `next`: the node it came from.
    pub from: Option<ProgressNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Event {
    Open {
        run: String,
        key: String,
        name: String,
        at: String,
        chart: String,
        #[serde(default)]
        nodes: Vec<ChartNode>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        strict: bool,
    },
    Step {
        node: String,
        status: StepStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        at: String,
    },
    Next {
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        at: String,
    },
    Say {
        node: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rid: Option<String>,
        at: String,
    },
    Note {
        text: String,
        #[serde(default)]
        links: Vec<RunLink>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rid: Option<String>,
        at: String,
    },
    Message {
        text: String,
        #[serde(default)]
        links: Vec<RunLink>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rid: Option<String>,
        at: String,
    },
    Close {
        outcome: String,
        at: String,
    },
}

#[derive(Debug, Serialize)]
struct Doc<'a> {
    version: u32,
    events: &'a [Event],
}

// --------------------------------------------------------------------------- paths

/// A run id is a file name, so it is held to the shape `new_run_id` produces and no more.
pub fn valid_run_id(run_id: &str) -> bool {
    let b = run_id.as_bytes();
    !b.is_empty()
        && b.len() <= 81
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        && !run_id.contains("..")
}

fn require_run_id(run_id: Option<&str>) -> PResult<&str> {
    let Some(id) = run_id.filter(|s| !s.is_empty()) else {
        return Err(perr(
            ProgressCode::NoRun,
            "not inside a run — AUTOMATION_RUN is not set (or pass --run <id>)",
        ));
    };
    if !valid_run_id(id) {
        return Err(perr(
            ProgressCode::UnknownRun,
            format!("not a run id: {id} — use the id fm gave the run, like 20260917-140047-3fa2"),
        ));
    }
    Ok(id)
}

fn tombstone_file(paths: &StatePaths, run_id: &str) -> PathBuf {
    paths.progress_dir().join(format!("{run_id}.closed"))
}

// --------------------------------------------------------------------------- folding

/// The events in a progress file; `None` when there is no readable file.
fn read_events(file: &Path) -> Option<Vec<Event>> {
    let doc: Value = read_json(file)?;
    let events = doc.get("events")?.as_array()?;
    Some(
        events
            .iter()
            .filter_map(|e| serde_json::from_value(e.clone()).ok())
            .collect(),
    )
}

fn set_status(node: &mut ProgressNode, status: StepStatus, at: &str) {
    node.status = status;
    if running(status) {
        if node.started_at.is_none() {
            node.started_at = Some(at.to_string());
        }
        node.ended_at = None;
    } else if settled(status) {
        node.ended_at = Some(at.to_string());
    }
}

/// Replay events into where the run is now. `None` without an `open` first.
fn fold(events: &[Event]) -> Option<ProgressSnapshot> {
    let Some(Event::Open {
        run,
        key,
        name,
        at,
        chart,
        nodes,
        strict,
    }) = events.first()
    else {
        return None;
    };
    let mut snap = ProgressSnapshot {
        run: run.clone(),
        key: key.clone(),
        name: name.clone(),
        chart: chart.clone(),
        strict: *strict,
        opened_at: at.clone(),
        nodes: nodes
            .iter()
            .map(|n| ProgressNode {
                id: n.id.clone(),
                label: n.label.clone(),
                status: StepStatus::Pending,
                text: None,
                started_at: None,
                ended_at: None,
                successors: n.successors.clone(),
            })
            .collect(),
        taken: Vec::new(),
        notes: Vec::new(),
        message: None,
        closed: false,
        revision: events.len(),
    };
    let index = |snap: &ProgressSnapshot, id: &str| snap.nodes.iter().position(|n| n.id == id);
    for e in &events[1..] {
        match e {
            Event::Open { .. } => {}
            Event::Step {
                node,
                status,
                text,
                at,
            } => {
                if let Some(i) = index(&snap, node) {
                    set_status(&mut snap.nodes[i], *status, at);
                    if let Some(t) = text {
                        snap.nodes[i].text = Some(t.clone());
                    }
                }
            }
            Event::Next { from, to, text, at } => {
                let (Some(f), Some(t)) = (index(&snap, from), index(&snap, to)) else {
                    continue;
                };
                if !settled(snap.nodes[f].status) {
                    set_status(&mut snap.nodes[f], StepStatus::Done, at);
                }
                set_status(&mut snap.nodes[t], StepStatus::Active, at);
                if let Some(text) = text {
                    snap.nodes[t].text = Some(text.clone());
                }
                if !snap.taken.iter().any(|(a, b)| a == from && b == to) {
                    snap.taken.push((from.clone(), to.clone()));
                }
            }
            Event::Say { node, text, .. } => {
                if let Some(i) = index(&snap, node) {
                    snap.nodes[i].text = Some(text.clone());
                }
            }
            Event::Note {
                text, links, at, ..
            } => snap.notes.push(RunNote {
                ts: at.clone(),
                text: text.clone(),
                links: links.clone(),
            }),
            Event::Message {
                text, links, at, ..
            } => {
                snap.message = Some(RunNote {
                    ts: at.clone(),
                    text: text.clone(),
                    links: links.clone(),
                });
            }
            Event::Close { at, .. } => {
                snap.closed = true;
                // A node still going when the run ended did not finish.
                for n in &mut snap.nodes {
                    if running(n.status) {
                        set_status(n, StepStatus::Interrupted, at);
                    }
                }
            }
        }
    }
    Some(snap)
}

/// What a tracked run reported, for its record.
pub fn progress_of(snap: &ProgressSnapshot) -> RunProgress {
    RunProgress {
        chart: snap.chart.clone(),
        steps: snap
            .nodes
            .iter()
            .map(|n| fm_model::RunStep {
                id: n.id.clone(),
                label: n.label.clone(),
                status: n.status,
                text: n.text.clone(),
                started_at: n.started_at.clone(),
                ended_at: n.ended_at.clone(),
            })
            .collect(),
        taken: snap.taken.clone(),
        notes: snap.notes.clone(),
        message: snap.message.clone(),
    }
}

/// Put what a run reported onto its record. Empty lists, and a chart it never had, stay off.
pub fn with_progress(mut rec: RunRecord, progress: &RunProgress) -> RunRecord {
    if !progress.chart.is_empty() {
        rec.chart = Some(progress.chart.clone());
        rec.steps = progress.steps.clone();
    }
    if !progress.taken.is_empty() {
        rec.taken = progress.taken.clone();
    }
    if !progress.notes.is_empty() {
        rec.notes = progress.notes.clone();
    }
    if progress.message.is_some() {
        rec.message = progress.message.clone();
    }
    rec
}

/// Where a run is, without a lock. `None` when it has no open progress here.
pub fn read_progress(paths: &StatePaths, run_id: &str) -> Option<ProgressSnapshot> {
    if !valid_run_id(run_id) {
        return None;
    }
    fold(&read_events(&paths.progress_file(run_id))?)
}

// --------------------------------------------------------------------------- writing

/// Control characters out (C0 and C1; tab and newline kept when `keep_lines`), so outside text
/// cannot start an escape sequence on a terminal.
pub fn strip_controls(s: &str, keep_lines: bool) -> String {
    s.chars()
        .filter(|&c| {
            let cp = u32::from(c);
            let control = cp <= 0x1f || (0x7f..=0x9f).contains(&cp);
            !control || (keep_lines && (c == '\t' || c == '\n'))
        })
        .collect()
}

fn clamp(raw: &str, warnings: &mut Vec<String>) -> String {
    let text = strip_controls(raw, true);
    let n = text.chars().count();
    if n <= TEXT_MAX {
        return text;
    }
    warnings.push(format!(
        "text was {n} characters; kept the first {TEXT_MAX}"
    ));
    text.chars().take(TEXT_MAX).collect()
}

fn under_lock<T>(paths: &StatePaths, run_id: &str, f: impl FnOnce() -> PResult<T>) -> PResult<T> {
    let _held = lock_file(&lock_path_for(&paths.progress_file(run_id)), LOCK_TIMEOUT)?;
    f()
}

struct Open {
    file: PathBuf,
    events: Vec<Event>,
    snap: ProgressSnapshot,
}

fn closed_or_unknown(paths: &StatePaths, run_id: &str) -> ProgressError {
    if tombstone_file(paths, run_id).exists() {
        perr(
            ProgressCode::RunClosed,
            format!("run {run_id} has ended — its progress is in its run record"),
        )
    } else {
        perr(
            ProgressCode::UnknownRun,
            format!("no run {run_id} here — check AUTOMATION_RUN and AUTOMATION_STATE_DIR"),
        )
    }
}

fn load_open(paths: &StatePaths, run_id: &str) -> PResult<Open> {
    let file = paths.progress_file(run_id);
    let events = read_events(&file).unwrap_or_default();
    let Some(snap) = fold(&events) else {
        return Err(closed_or_unknown(paths, run_id));
    };
    if snap.closed {
        return Err(perr(
            ProgressCode::RunClosed,
            format!("run {run_id} has ended — its progress is in its run record"),
        ));
    }
    Ok(Open { file, events, snap })
}

fn commit(open: &mut Open, event: Event) -> PResult<ProgressSnapshot> {
    open.events.push(event);
    write_json_atomic(
        &open.file,
        &Doc {
            version: 1,
            events: &open.events,
        },
    )?;
    Ok(fold(&open.events).expect("an open run folds"))
}

fn require_chart(snap: &ProgressSnapshot) -> PResult<()> {
    if snap.nodes.is_empty() {
        return Err(perr(
            ProgressCode::NoChart,
            "this run has no flowchart, so it has no nodes to move — use fm note, fm message or fm status",
        ));
    }
    Ok(())
}

fn describe(n: &ProgressNode) -> String {
    if n.label != n.id {
        format!("{} (\"{}\")", n.id, n.label)
    } else {
        n.id.clone()
    }
}

fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 3 {
        return 99;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1)
                .min(cur[j - 1] + 1)
                .min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// A node by id, else by its exact label, else refused with the nodes it probably meant.
pub fn resolve_node(snap: &ProgressSnapshot, reference: &str) -> PResult<ProgressNode> {
    if let Some(n) = snap.node(reference) {
        return Ok(n.clone());
    }
    let by_label: Vec<&ProgressNode> = snap.nodes.iter().filter(|n| n.label == reference).collect();
    match by_label.len() {
        1 => return Ok(by_label[0].clone()),
        0 => {}
        k => {
            return Err(ProgressError {
                code: ProgressCode::AmbiguousNode,
                message: format!("\"{reference}\" is the label of {k} nodes — use an id"),
                suggestions: by_label.iter().map(|n| n.id.clone()).collect(),
            });
        }
    }
    let want = reference.to_lowercase();
    let mut close: Vec<(usize, &ProgressNode)> = snap
        .nodes
        .iter()
        .map(|n| {
            let (id, label) = (n.id.to_lowercase(), n.label.to_lowercase());
            let score = if id.contains(&want) || want.contains(&id) || label.contains(&want) {
                0
            } else {
                distance(&id, &want).min(distance(&label, &want))
            };
            (score, n)
        })
        .filter(|(s, _)| *s <= 2)
        .collect();
    close.sort_by_key(|(s, _)| *s);
    let suggestions: Vec<String> = if close.is_empty() {
        snap.nodes.iter().map(describe).collect()
    } else {
        close.iter().take(5).map(|(_, n)| describe(n)).collect()
    };
    Err(ProgressError {
        code: ProgressCode::UnknownNode,
        message: format!(
            "no node \"{reference}\" on this run's chart — use one of {}",
            suggestions.join(", ")
        ),
        suggestions,
    })
}

/// The node a write is about when none was named: `AUTOMATION_NODE`, else the one running node.
fn default_node(
    snap: &ProgressSnapshot,
    env_node: Option<&str>,
    flag: &str,
) -> PResult<ProgressNode> {
    if let Some(n) = env_node.map(str::trim).filter(|n| !n.is_empty()) {
        return resolve_node(snap, n);
    }
    let going: Vec<&ProgressNode> = snap.nodes.iter().filter(|n| running(n.status)).collect();
    match going.len() {
        1 => Ok(going[0].clone()),
        0 => Err(perr(
            ProgressCode::NoActive,
            format!("no node is running — say which one with {flag}"),
        )),
        k => Err(ProgressError {
            code: ProgressCode::AmbiguousActive,
            message: format!("{k} nodes are running — say which one with {flag}"),
            suggestions: going.iter().map(|n| describe(n)).collect(),
        }),
    }
}

/// `pre0`, `body`, `post0`…: the nodes the run moves itself.
pub fn is_reserved_node(id: &str) -> bool {
    if id == "body" {
        return true;
    }
    ["pre", "post"].iter().any(|p| {
        id.strip_prefix(p)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn require_edge(from: &ProgressNode, to: &ProgressNode) -> PResult<()> {
    if (is_reserved_node(&from.id) && is_reserved_node(&to.id)) || from.successors.contains(&to.id)
    {
        return Ok(());
    }
    let message = if from.successors.is_empty() {
        format!(
            "the chart has no arrow {} → {}; nothing follows {} — follow the chart's arrows",
            from.id, to.id, from.id
        )
    } else {
        format!(
            "the chart has no arrow {} → {}; from {} it goes to {} — follow the chart's arrows",
            from.id,
            to.id,
            from.id,
            from.successors.join(" or ")
        )
    };
    Err(ProgressError {
        code: ProgressCode::NotAnEdge,
        message,
        suggestions: from.successors.clone(),
    })
}

/// What a run opens its progress with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenArgs {
    pub run_id: String,
    pub key: String,
    pub name: String,
    /// The Mermaid source, or empty for a run with no chart (notes and a message only).
    pub chart: String,
    /// The chart's nodes in flow order.
    pub nodes: Vec<ChartNode>,
    pub strict: bool,
}

/// Open a run's progress, once, before anything reports. Refuses a run that already has it.
pub fn open_progress(
    paths: &StatePaths,
    args: &OpenArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressSnapshot> {
    let run_id = require_run_id(Some(&args.run_id))?;
    ensure_dir(&paths.progress_dir())?;
    sweep_tombstones(paths, now);
    under_lock(paths, run_id, || {
        let file = paths.progress_file(run_id);
        if file.exists() || tombstone_file(paths, run_id).exists() {
            return Err(perr(
                ProgressCode::Usage,
                format!("run {run_id} already has progress — give each run a new id"),
            ));
        }
        let events = vec![Event::Open {
            run: run_id.to_string(),
            key: args.key.clone(),
            name: args.name.clone(),
            at: iso(now),
            chart: args.chart.clone(),
            nodes: args.nodes.clone(),
            strict: args.strict,
        }];
        write_json_atomic(
            &file,
            &Doc {
                version: 1,
                events: &events,
            },
        )?;
        Ok(fold(&events).expect("an open event folds"))
    })
}

/// `fm step`: set one node's status (default `active`) and maybe its update.
#[derive(Debug, Clone, Default)]
pub struct StepArgs {
    pub node: String,
    pub status: Option<StepStatus>,
    pub text: Option<String>,
}

pub fn progress_step(
    paths: &StatePaths,
    run_id: Option<&str>,
    args: &StepArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressResult> {
    let run_id = require_run_id(run_id)?;
    let status = args.status.unwrap_or(StepStatus::Active);
    if !SETTABLE.contains(&status) {
        let names: Vec<&str> = SETTABLE.iter().map(|s| status_name(*s)).collect();
        return Err(perr(
            ProgressCode::Usage,
            format!(
                "--status {} cannot be set — use one of {}",
                status_name(status),
                names.join("|")
            ),
        ));
    }
    under_lock(paths, run_id, || {
        let mut open = load_open(paths, run_id)?;
        require_chart(&open.snap)?;
        let node = resolve_node(&open.snap, &args.node)?;
        let mut warnings = Vec::new();
        let text = args.text.as_deref().map(|t| clamp(t, &mut warnings));
        let same = node.status == status && (text.is_none() || node.text == text);
        if same {
            return Ok(ProgressResult {
                changed: false,
                warnings,
                snapshot: open.snap,
                node: Some(node),
                from: None,
            });
        }
        let after = commit(
            &mut open,
            Event::Step {
                node: node.id.clone(),
                status,
                text,
                at: iso(now),
            },
        )?;
        Ok(ProgressResult {
            changed: true,
            warnings,
            node: after.node(&node.id).cloned(),
            snapshot: after,
            from: None,
        })
    })
}

/// `fm next`: settle one node, start the next, record the arrow.
#[derive(Debug, Clone, Default)]
pub struct NextArgs {
    pub to: String,
    pub from: Option<String>,
    /// `AUTOMATION_NODE` of the writer.
    pub env_node: Option<String>,
    pub text: Option<String>,
}

pub fn progress_next(
    paths: &StatePaths,
    run_id: Option<&str>,
    args: &NextArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressResult> {
    let run_id = require_run_id(run_id)?;
    under_lock(paths, run_id, || {
        let mut open = load_open(paths, run_id)?;
        require_chart(&open.snap)?;
        let to = resolve_node(&open.snap, &args.to)?;
        let from = match &args.from {
            Some(f) => resolve_node(&open.snap, f)?,
            None => default_node(&open.snap, args.env_node.as_deref(), "--from")?,
        };
        if from.id == to.id {
            return Err(perr(
                ProgressCode::Usage,
                format!(
                    "next {} cannot come from itself — name the node it follows with --from",
                    to.id
                ),
            ));
        }
        if open.snap.strict {
            require_edge(&from, &to)?;
        }
        let mut warnings = Vec::new();
        let text = args.text.as_deref().map(|t| clamp(t, &mut warnings));
        // A retry of a move already made: the arrow is taken, `from` settled, `to` started.
        let taken = open
            .snap
            .taken
            .iter()
            .any(|(a, b)| *a == from.id && *b == to.id);
        if taken
            && settled(from.status)
            && to.status != StepStatus::Pending
            && (text.is_none() || to.text == text)
        {
            return Ok(ProgressResult {
                changed: false,
                warnings,
                snapshot: open.snap,
                node: Some(to),
                from: Some(from),
            });
        }
        let after = commit(
            &mut open,
            Event::Next {
                from: from.id.clone(),
                to: to.id.clone(),
                text,
                at: iso(now),
            },
        )?;
        Ok(ProgressResult {
            changed: true,
            warnings,
            node: after.node(&to.id).cloned(),
            from: after.node(&from.id).cloned(),
            snapshot: after,
        })
    })
}

/// `fm say`: an update on a node.
#[derive(Debug, Clone, Default)]
pub struct SayArgs {
    pub text: String,
    pub node: Option<String>,
    pub env_node: Option<String>,
    pub request_id: Option<String>,
}

pub fn progress_say(
    paths: &StatePaths,
    run_id: Option<&str>,
    args: &SayArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressResult> {
    let run_id = require_run_id(run_id)?;
    if args.text.trim().is_empty() {
        return Err(perr(
            ProgressCode::Usage,
            "say needs some text — fm say \"what is happening\"",
        ));
    }
    under_lock(paths, run_id, || {
        let mut open = load_open(paths, run_id)?;
        require_chart(&open.snap)?;
        let node = match &args.node {
            Some(n) => resolve_node(&open.snap, n)?,
            None => default_node(&open.snap, args.env_node.as_deref(), "--node")?,
        };
        let mut warnings = Vec::new();
        let text = clamp(&args.text, &mut warnings);
        let retry = args.request_id.as_ref().is_some_and(|rid| {
            open.events
                .iter()
                .any(|e| matches!(e, Event::Say { rid: Some(r), .. } if r == rid))
        });
        if retry {
            return Ok(ProgressResult {
                changed: false,
                warnings,
                snapshot: open.snap,
                node: Some(node),
                from: None,
            });
        }
        let after = commit(
            &mut open,
            Event::Say {
                node: node.id.clone(),
                text,
                rid: args.request_id.clone(),
                at: iso(now),
            },
        )?;
        Ok(ProgressResult {
            changed: true,
            warnings,
            node: after.node(&node.id).cloned(),
            snapshot: after,
            from: None,
        })
    })
}

/// One link from the command line: `URL` or `label=URL`, split on the first `=` only when an
/// http(s) URL follows. The parsed URL is kept, so control characters are percent-encoded.
pub fn parse_link(raw: &str) -> PResult<RunLink> {
    let text = raw.trim();
    let mut label = None;
    let mut url = text;
    if let Some(eq) = text.find('=').filter(|&i| i > 0) {
        let after = text[eq + 1..].trim();
        let lower = after.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            let l = strip_controls(&text[..eq], false).trim().to_string();
            label = (!l.is_empty()).then_some(l);
            url = after;
        }
    }
    let parsed = url::Url::parse(url).map_err(|_| {
        perr(
            ProgressCode::BadLink,
            format!("not a URL: {raw} — use https://… or \"label=https://…\""),
        )
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(perr(
            ProgressCode::BadLink,
            format!("only http and https links are kept: {raw} — use an https:// link"),
        ));
    }
    Ok(RunLink {
        label,
        url: parsed.to_string(),
    })
}

/// `fm note` and `fm message`: text with links.
#[derive(Debug, Clone, Default)]
pub struct LinkedArgs {
    pub text: String,
    /// Raw `URL` or `label=URL`.
    pub links: Vec<String>,
    pub request_id: Option<String>,
}

/// A note on the run's record: what it published, with links.
pub fn progress_note(
    paths: &StatePaths,
    run_id: Option<&str>,
    args: &LinkedArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressResult> {
    write_linked(false, paths, run_id, args, now)
}

/// The run's one message for its reader. A later message replaces it.
pub fn progress_message(
    paths: &StatePaths,
    run_id: Option<&str>,
    args: &LinkedArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressResult> {
    write_linked(true, paths, run_id, args, now)
}

fn write_linked(
    message: bool,
    paths: &StatePaths,
    run_id: Option<&str>,
    args: &LinkedArgs,
    now: DateTime<Utc>,
) -> PResult<ProgressResult> {
    let run_id = require_run_id(run_id)?;
    let kind = if message { "message" } else { "note" };
    if args.text.trim().is_empty() {
        return Err(perr(
            ProgressCode::Usage,
            format!("{kind} needs some text — fm {kind} \"what happened\""),
        ));
    }
    // Every link is checked before anything is written: one bad link refuses the whole call.
    let links = args
        .links
        .iter()
        .map(|l| parse_link(l))
        .collect::<PResult<Vec<_>>>()?;
    under_lock(paths, run_id, || {
        let mut open = load_open(paths, run_id)?;
        let mut warnings = Vec::new();
        let text = clamp(&args.text, &mut warnings);
        let retry = args.request_id.as_ref().is_some_and(|rid| {
            open.events.iter().any(|e| match e {
                Event::Note { rid: Some(r), .. } if !message => r == rid,
                Event::Message { rid: Some(r), .. } if message => r == rid,
                _ => false,
            })
        });
        if retry {
            return Ok(ProgressResult {
                changed: false,
                warnings,
                snapshot: open.snap,
                node: None,
                from: None,
            });
        }
        let (rid, at) = (args.request_id.clone(), iso(now));
        let event = if message {
            Event::Message {
                text,
                links,
                rid,
                at,
            }
        } else {
            Event::Note {
                text,
                links,
                rid,
                at,
            }
        };
        let after = commit(&mut open, event)?;
        Ok(ProgressResult {
            changed: true,
            warnings,
            snapshot: after,
            node: None,
            from: None,
        })
    })
}

/// `fm status`: where the run is.
pub fn progress_status(paths: &StatePaths, run_id: Option<&str>) -> PResult<ProgressSnapshot> {
    let run_id = require_run_id(run_id)?;
    read_progress(paths, run_id).ok_or_else(|| closed_or_unknown(paths, run_id))
}

/// Close a run's progress and return what it reported. `None` when it had none. Removing the file
/// is best effort: the tombstone and the close event both say it ended.
pub fn close_progress(
    paths: &StatePaths,
    run_id: &str,
    outcome: &str,
    now: DateTime<Utc>,
) -> PResult<Option<RunProgress>> {
    if !valid_run_id(run_id) || !paths.progress_file(run_id).exists() {
        return Ok(None);
    }
    under_lock(paths, run_id, || {
        let file = paths.progress_file(run_id);
        let mut events = read_events(&file).unwrap_or_default();
        let Some(before) = fold(&events) else {
            return Ok(None);
        };
        let at = iso(now);
        let snap = if before.closed {
            before
        } else {
            events.push(Event::Close {
                outcome: outcome.to_string(),
                at: at.clone(),
            });
            let _ = write_json_atomic(
                &file,
                &Doc {
                    version: 1,
                    events: &events,
                },
            );
            fold(&events).expect("an open run folds")
        };
        let tomb = serde_json::json!({ "run": run_id, "outcome": outcome, "at": at });
        let _ = write_json_atomic(&tombstone_file(paths, run_id), &tomb);
        let _ = fs::remove_file(&file);
        Ok(Some(progress_of(&snap)))
    })
}

/// Fold an abandoned run's progress into the `aborted` record the lock sweep writes for it.
pub fn fold_abandoned(paths: &StatePaths, rec: RunRecord, now: DateTime<Utc>) -> RunRecord {
    let outcome = serde_json::to_value(rec.outcome)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "aborted".into());
    match close_progress(paths, &rec.id, &outcome, now) {
        Ok(Some(p)) => with_progress(rec, &p),
        _ => rec,
    }
}

/// Remove progress files whose run has closed, and tombstones (with their lock) older than a week.
pub fn sweep_tombstones(paths: &StatePaths, now: DateTime<Utc>) {
    let dir = paths.progress_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(run_id) = name.strip_suffix(".closed") else {
            continue;
        };
        let events = paths.progress_file(run_id);
        let _ = fs::remove_file(&events);
        let tomb = entry.path();
        let at = read_json::<Value>(&tomb)
            .and_then(|v| v.get("at").and_then(Value::as_str).and_then(parse_iso))
            .or_else(|| {
                fs::metadata(&tomb)
                    .and_then(|m| m.modified())
                    .ok()
                    .map(DateTime::<Utc>::from)
            });
        if at.is_some_and(|at| now - at > Duration::days(TOMBSTONE_DAYS)) {
            let _ = fs::remove_file(&tomb);
            let _ = fs::remove_file(lock_path_for(&events));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Arc;
    use std::thread;

    const RUN: &str = "20260917-120000-abcd";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 12, 0, 0).unwrap()
    }

    fn node(id: &str, label: &str, succ: &[&str]) -> ChartNode {
        ChartNode {
            id: id.into(),
            label: label.into(),
            successors: succ.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// pre0 → body → gather → {a, b} → merge → post0
    fn chart() -> Vec<ChartNode> {
        vec![
            node("pre0", "pre[0] counts.js", &["body"]),
            node("body", "body", &["gather"]),
            node("gather", "Gather PRs", &["a", "b"]),
            node("a", "Review A", &["merge"]),
            node("b", "Review B", &["merge"]),
            node("merge", "Merge", &["post0"]),
            node("post0", "post[0] notify.js", &[]),
        ]
    }

    fn open(strict: bool) -> (tempfile::TempDir, StatePaths) {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        open_progress(
            &p,
            &OpenArgs {
                run_id: RUN.into(),
                key: "user:/r:x".into(),
                name: "x".into(),
                chart: "flowchart TD\n...".into(),
                nodes: chart(),
                strict,
            },
            now(),
        )
        .unwrap();
        (d, p)
    }

    fn step(
        p: &StatePaths,
        node: &str,
        status: Option<StepStatus>,
        text: Option<&str>,
    ) -> PResult<ProgressResult> {
        progress_step(
            p,
            Some(RUN),
            &StepArgs {
                node: node.into(),
                status,
                text: text.map(str::to_string),
            },
            now(),
        )
    }

    fn next(
        p: &StatePaths,
        to: &str,
        from: Option<&str>,
        env: Option<&str>,
    ) -> PResult<ProgressResult> {
        progress_next(
            p,
            Some(RUN),
            &NextArgs {
                to: to.into(),
                from: from.map(str::to_string),
                env_node: env.map(str::to_string),
                text: None,
            },
            now(),
        )
    }

    fn revision(p: &StatePaths) -> usize {
        read_progress(p, RUN).unwrap().revision
    }

    #[test]
    fn step_activates_by_default_and_a_repeat_appends_nothing() {
        let (_d, p) = open(false);
        let r = step(&p, "gather", None, None).unwrap();
        assert!(r.changed);
        assert_eq!(r.node.as_ref().unwrap().status, StepStatus::Active);
        assert_eq!(
            r.node.unwrap().started_at.as_deref(),
            Some("2026-09-17T12:00:00.000Z")
        );
        let rev = revision(&p);
        assert!(!step(&p, "gather", None, None).unwrap().changed);
        assert_eq!(revision(&p), rev);
    }

    #[test]
    fn step_addresses_a_node_by_its_exact_label() {
        let (_d, p) = open(false);
        let r = step(&p, "Gather PRs", Some(StepStatus::Done), Some("12 found")).unwrap();
        let n = r.node.unwrap();
        assert_eq!((n.id.as_str(), n.status), ("gather", StepStatus::Done));
        assert_eq!(n.text.as_deref(), Some("12 found"));
        assert!(n.ended_at.is_some());
    }

    #[test]
    fn refuses_an_unknown_node_with_what_it_probably_meant() {
        let (_d, p) = open(false);
        let e = step(&p, "gathr", None, None).unwrap_err();
        assert_eq!(e.code, ProgressCode::UnknownNode);
        assert!(
            e.suggestions
                .contains(&"gather (\"Gather PRs\")".to_string()),
            "{:?}",
            e.suggestions
        );
        assert!(e.message.contains(" — ") && !e.message.contains('\n'));
    }

    #[test]
    fn refuses_a_status_it_may_not_set() {
        let (_d, p) = open(false);
        let e = step(&p, "gather", Some(StepStatus::Interrupted), None).unwrap_err();
        assert_eq!(e.code, ProgressCode::Usage);
    }

    #[test]
    fn keeps_the_first_text_max_characters_and_says_so() {
        let (_d, p) = open(false);
        let long = "x".repeat(TEXT_MAX + 10);
        let r = step(&p, "gather", None, Some(&long)).unwrap();
        assert_eq!(r.node.unwrap().text.unwrap().chars().count(), TEXT_MAX);
        assert_eq!(
            r.warnings,
            vec![format!(
                "text was {} characters; kept the first {TEXT_MAX}",
                TEXT_MAX + 10
            )]
        );
    }

    #[test]
    fn next_settles_from_starts_to_and_records_the_arrow() {
        let (_d, p) = open(false);
        step(&p, "gather", None, None).unwrap();
        let r = next(&p, "a", Some("gather"), None).unwrap();
        assert_eq!(r.from.unwrap().status, StepStatus::Done);
        assert_eq!(r.node.unwrap().status, StepStatus::Active);
        assert_eq!(r.snapshot.taken, vec![("gather".into(), "a".into())]);
        // A retry changes nothing.
        let rev = revision(&p);
        assert!(!next(&p, "a", Some("gather"), None).unwrap().changed);
        assert_eq!(revision(&p), rev);
    }

    #[test]
    fn next_comes_from_fm_node_else_the_one_running_node() {
        let (_d, p) = open(false);
        step(&p, "gather", None, None).unwrap();
        let r = next(&p, "a", None, None).unwrap();
        assert_eq!(r.from.unwrap().id, "gather");
        let r = next(&p, "merge", None, Some("a")).unwrap();
        assert_eq!(r.from.unwrap().id, "a");
    }

    #[test]
    fn next_refuses_to_guess_between_several_running_nodes_or_none() {
        let (_d, p) = open(false);
        assert_eq!(
            next(&p, "a", None, None).unwrap_err().code,
            ProgressCode::NoActive
        );
        step(&p, "a", None, None).unwrap();
        step(&p, "b", None, None).unwrap();
        let e = next(&p, "merge", None, None).unwrap_err();
        assert_eq!(e.code, ProgressCode::AmbiguousActive);
        assert_eq!(e.suggestions.len(), 2);
        assert_eq!(
            next(&p, "a", Some("a"), None).unwrap_err().code,
            ProgressCode::Usage
        );
    }

    #[test]
    fn converges_branches_on_one_node_and_never_marks_a_failed_branch_done() {
        let (_d, p) = open(false);
        step(&p, "a", None, None).unwrap();
        step(&p, "b", Some(StepStatus::Failed), None).unwrap();
        next(&p, "merge", Some("a"), None).unwrap();
        let r = next(&p, "merge", Some("b"), None).unwrap();
        let s = r.snapshot;
        assert_eq!(s.node("b").unwrap().status, StepStatus::Failed);
        assert_eq!(s.node("merge").unwrap().status, StepStatus::Active);
        assert_eq!(s.taken.len(), 2);
    }

    #[test]
    fn say_lands_on_a_node_and_a_request_id_makes_a_retry_a_no_op() {
        let (_d, p) = open(false);
        step(&p, "gather", None, None).unwrap();
        let args = SayArgs {
            text: "halfway".into(),
            request_id: Some("r1".into()),
            ..SayArgs::default()
        };
        let r = progress_say(&p, Some(RUN), &args, now()).unwrap();
        assert_eq!(r.node.unwrap().text.as_deref(), Some("halfway"));
        let rev = revision(&p);
        assert!(!progress_say(&p, Some(RUN), &args, now()).unwrap().changed);
        assert_eq!(revision(&p), rev);
        let empty = SayArgs {
            text: " ".into(),
            ..SayArgs::default()
        };
        assert_eq!(
            progress_say(&p, Some(RUN), &empty, now()).unwrap_err().code,
            ProgressCode::Usage
        );
    }

    #[test]
    fn a_note_keeps_its_text_and_links() {
        let (_d, p) = open(false);
        let r = progress_note(
            &p,
            Some(RUN),
            &LinkedArgs {
                text: "Opened PR".into(),
                links: vec![
                    "PR #12=https://example.com/pr/12".into(),
                    "https://example.com/x".into(),
                ],
                request_id: None,
            },
            now(),
        )
        .unwrap();
        let note = &r.snapshot.notes[0];
        assert_eq!(note.text, "Opened PR");
        assert_eq!(note.links[0].label.as_deref(), Some("PR #12"));
        assert_eq!(note.links[0].url, "https://example.com/pr/12");
        assert_eq!(note.links[1].label, None);
    }

    #[test]
    fn refuses_a_link_that_is_not_http_and_writes_nothing() {
        let (_d, p) = open(false);
        let rev = revision(&p);
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "not a url"] {
            let e = progress_note(
                &p,
                Some(RUN),
                &LinkedArgs {
                    text: "x".into(),
                    links: vec!["https://ok.example".into(), bad.into()],
                    request_id: None,
                },
                now(),
            )
            .unwrap_err();
            assert_eq!(e.code, ProgressCode::BadLink, "{bad}");
        }
        assert_eq!(revision(&p), rev);
    }

    #[test]
    fn splits_label_url_on_the_first_equals_only_before_a_url() {
        let l = parse_link("https://example.com/?a=b").unwrap();
        assert_eq!(
            (l.label, l.url.as_str()),
            (None, "https://example.com/?a=b")
        );
        // `y=https://…` is not a URL, so `x` is not a label, and the whole is not a URL either.
        assert_eq!(
            parse_link("x=y=https://example.com/").unwrap_err().code,
            ProgressCode::BadLink
        );
        let l = parse_link("Docs=HTTPS://Example.com/a b").unwrap();
        assert_eq!(l.label.as_deref(), Some("Docs"));
        assert_eq!(l.url, "https://example.com/a%20b");
    }

    #[test]
    fn keeps_a_link_with_controls_percent_encoded_and_strips_them_from_text() {
        let l = parse_link("https://example.com/a\u{7}b").unwrap();
        assert!(!l.url.chars().any(|c| c.is_control()), "{}", l.url);
        let (_d, p) = open(false);
        let r = step(
            &p,
            "gather",
            None,
            Some("line one\n\u{1b}[31mred\u{9b}\tok"),
        )
        .unwrap();
        assert_eq!(
            r.node.unwrap().text.as_deref(),
            Some("line one\n[31mred\tok")
        );
    }

    #[test]
    fn a_message_replaces_the_last_and_is_exactly_once_by_request_id() {
        let (_d, p) = open(false);
        let msg = |t: &str, rid: &str| LinkedArgs {
            text: t.into(),
            links: vec![],
            request_id: Some(rid.into()),
        };
        progress_message(&p, Some(RUN), &msg("first", "1"), now()).unwrap();
        progress_message(&p, Some(RUN), &msg("second", "2"), now()).unwrap();
        assert!(
            !progress_message(&p, Some(RUN), &msg("second", "2"), now())
                .unwrap()
                .changed
        );
        let s = read_progress(&p, RUN).unwrap();
        assert_eq!(s.message.unwrap().text, "second");
        // A note with the same request id is a different write.
        assert!(
            progress_note(&p, Some(RUN), &msg("n", "2"), now())
                .unwrap()
                .changed
        );
    }

    #[test]
    fn closing_folds_everything_removes_the_file_and_refuses_stragglers() {
        let (_d, p) = open(false);
        step(&p, "gather", None, Some("going")).unwrap();
        progress_note(
            &p,
            Some(RUN),
            &LinkedArgs {
                text: "n".into(),
                ..LinkedArgs::default()
            },
            now(),
        )
        .unwrap();
        let progress = close_progress(&p, RUN, "ok", now()).unwrap().unwrap();
        let gather = progress.steps.iter().find(|s| s.id == "gather").unwrap();
        assert_eq!(gather.status, StepStatus::Interrupted);
        assert_eq!(progress.notes.len(), 1);
        assert!(!p.progress_file(RUN).exists());
        let e = step(&p, "gather", None, None).unwrap_err();
        assert_eq!(e.code, ProgressCode::RunClosed);
        assert_eq!(
            progress_status(&p, Some(RUN)).unwrap_err().code,
            ProgressCode::RunClosed
        );
        assert_eq!(close_progress(&p, RUN, "ok", now()).unwrap(), None);
    }

    #[test]
    fn puts_an_abandoned_runs_progress_on_the_aborted_record() {
        let (_d, p) = open(false);
        step(&p, "gather", None, None).unwrap();
        let mut rec = crate::records::tests::rec("user:/r:x", RUN);
        rec.outcome = fm_model::Outcome::Aborted;
        let rec = fold_abandoned(&p, rec, now());
        assert_eq!(rec.chart.as_deref(), Some("flowchart TD\n..."));
        assert_eq!(
            rec.steps.iter().find(|s| s.id == "gather").unwrap().status,
            StepStatus::Interrupted
        );
    }

    #[test]
    fn a_record_with_nothing_reported_stays_as_it_was() {
        let rec = crate::records::tests::rec("user:/r:x", RUN);
        assert_eq!(with_progress(rec.clone(), &RunProgress::default()), rec);
    }

    #[test]
    fn refuses_run_ids_that_would_escape_the_folder_and_a_missing_run() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        for bad in ["../x", "a/b", "a\\b", "..", ".x", ""] {
            assert!(!valid_run_id(bad), "{bad}");
        }
        assert_eq!(
            progress_status(&p, Some("../etc")).unwrap_err().code,
            ProgressCode::UnknownRun
        );
        assert_eq!(
            progress_status(&p, None).unwrap_err().code,
            ProgressCode::NoRun
        );
        assert_eq!(
            progress_status(&p, Some("nope")).unwrap_err().code,
            ProgressCode::UnknownRun
        );
        assert!(read_progress(&p, "../x").is_none());
    }

    #[test]
    fn refuses_to_open_a_run_twice() {
        let (_d, p) = open(false);
        let e = open_progress(
            &p,
            &OpenArgs {
                run_id: RUN.into(),
                ..OpenArgs::default()
            },
            now(),
        )
        .unwrap_err();
        assert_eq!(e.code, ProgressCode::Usage);
    }

    #[test]
    fn a_run_with_no_chart_keeps_notes_and_refuses_moves() {
        let d = tempfile::tempdir().unwrap();
        let p = StatePaths::new(d.path());
        open_progress(
            &p,
            &OpenArgs {
                run_id: RUN.into(),
                key: "k".into(),
                name: "n".into(),
                ..OpenArgs::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(
            step(&p, "x", None, None).unwrap_err().code,
            ProgressCode::NoChart
        );
        progress_note(
            &p,
            Some(RUN),
            &LinkedArgs {
                text: "n".into(),
                ..LinkedArgs::default()
            },
            now(),
        )
        .unwrap();
        let progress = close_progress(&p, RUN, "ok", now()).unwrap().unwrap();
        let rec = with_progress(crate::records::tests::rec("k", RUN), &progress);
        assert!(rec.chart.is_none() && rec.steps.is_empty());
        assert_eq!(rec.notes.len(), 1);
    }

    #[test]
    fn strict_flow_follows_arrows_except_between_hooks_and_the_body() {
        let (_d, p) = open(true);
        assert!(read_progress(&p, RUN).unwrap().strict);
        // The run's own moves between reserved nodes are free.
        step(&p, "pre0", None, None).unwrap();
        next(&p, "post0", Some("pre0"), None).unwrap();
        // The body's way in is checked: body → gather is an arrow, body → merge is not.
        let e = next(&p, "merge", Some("body"), None).unwrap_err();
        assert_eq!(e.code, ProgressCode::NotAnEdge);
        assert_eq!(e.suggestions, vec!["gather".to_string()]);
        assert!(
            e.message.contains("from body it goes to gather"),
            "{}",
            e.message
        );
        next(&p, "gather", Some("body"), None).unwrap();
        let e = next(&p, "merge", Some("gather"), None).unwrap_err();
        assert!(e.message.contains("a or b"), "{}", e.message);
        // `step` stays free.
        step(&p, "merge", None, None).unwrap();
        // And a loose run takes any move.
        let (_d2, loose) = open(false);
        next(&loose, "merge", Some("body"), None).unwrap();
    }

    #[test]
    fn sweeps_closed_files_now_and_tombstones_after_a_week() {
        let (_d, p) = open(false);
        close_progress(&p, RUN, "ok", now()).unwrap();
        let tomb = tombstone_file(&p, RUN);
        sweep_tombstones(&p, now() + Duration::days(1));
        assert!(tomb.exists());
        sweep_tombstones(&p, now() + Duration::days(8));
        assert!(!tomb.exists());
    }

    #[test]
    fn fan_in_from_many_threads_records_each_arrow_once() {
        let (_d, p) = open(false);
        let p = Arc::new(p);
        step(&p, "a", None, None).unwrap();
        step(&p, "b", None, None).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let p = p.clone();
                thread::spawn(move || {
                    let from = if i % 2 == 0 { "a" } else { "b" };
                    next(&p, "merge", Some(from), None).unwrap();
                    progress_message(
                        &p,
                        Some(RUN),
                        &LinkedArgs {
                            text: "done".into(),
                            links: vec![],
                            request_id: Some("m".into()),
                        },
                        now(),
                    )
                    .unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let s = read_progress(&p, RUN).unwrap();
        let mut taken = s.taken.clone();
        taken.sort();
        assert_eq!(
            taken,
            vec![("a".into(), "merge".into()), ("b".into(), "merge".into())]
        );
        // open + 2 steps + 2 nexts + 1 message.
        assert_eq!(s.revision, 6);
    }
}

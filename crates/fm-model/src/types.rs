//! Automations, consent, tick state and run records.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Repository,
}

/// A folder automations are discovered in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub path: PathBuf,
    pub scope: Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatchUp {
    Skip,
    Once,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub cron: String,
    /// IANA name; `None` means the host's local zone.
    pub timezone: Option<String>,
    pub catch_up: CatchUp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hook {
    /// Path relative to the automation folder.
    Script(String),
    Prompt(String),
}

/// `on.watch`: a script run every `every`; a run starts when it exits 0 with new output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    pub script: String,
    /// Raw duration, `1m` to `24h`.
    pub every: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triggers {
    pub schedule: Option<Schedule>,
    pub watch: Option<Watch>,
    pub manual: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hooks {
    pub pre: Vec<Hook>,
    pub post: Vec<Hook>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flow {
    #[default]
    Loose,
    Strict,
}

/// `<provider>/<id>[:<thinking>]`, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelSpec {
    pub provider: String,
    pub id: String,
    pub thinking: Option<String>,
}

impl ModelSpec {
    /// The canonical `provider/id[:thinking]` text.
    pub fn display(&self) -> String {
        match &self.thinking {
            Some(t) => format!("{}/{}:{}", self.provider, self.id, t),
            None => format!("{}/{}", self.provider, self.id),
        }
    }
}

/// A valid `AUTOMATION.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub model: Option<ModelSpec>,
    /// Raw duration string, as written.
    pub timeout: Option<String>,
    pub on: Triggers,
    pub hooks: Hooks,
    pub flow: Flow,
    /// The Markdown body, frontmatter removed.
    pub body: String,
}

/// An automation and where it was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    pub automation: Automation,
    pub dir: PathBuf,
    pub file: PathBuf,
    pub root: Root,
    /// Repository top level; repository scope only.
    pub repo_root: Option<PathBuf>,
    /// `<scope>:<root path>:<name>`. Stable identity used in every state file.
    pub key: String,
}

/// One folder found during discovery: valid, or refused with one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Discovered {
    Valid(Box<Located>),
    Invalid {
        dir: PathBuf,
        file: PathBuf,
        root: Root,
        key: String,
        /// One line: `<field path>: <what is wrong> — <what fixes it>`.
        error: String,
        /// Several problems (strict flow), each one; `error` joins them.
        problems: Vec<String>,
    },
}

impl Discovered {
    pub fn key(&self) -> &str {
        match self {
            Discovered::Valid(l) => &l.key,
            Discovered::Invalid { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutomationState {
    Invalid,
    Disabled,
    Off,
    Armed,
}

/// Where a session's transcript is, and which format it is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReference {
    pub file: PathBuf,
    /// Empty when the CLI never reported one (e.g. a run stopped before copilot's final event).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// A tag such as `claude/stream-json`. Unknown tags are reported, never guessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Consent for one automation, pinned to its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustRecord {
    pub key: String,
    pub dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<PathBuf>,
    /// `sha256:<hex>` over `AUTOMATION.md` and every script it can run.
    pub digest: String,
    pub armed_at: String,
    pub last_seen_at: String,
    /// Repository scope only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Script relative path -> the exact command shown at consent.
    #[serde(default)]
    pub interpreters: BTreeMap<String, String>,
    /// Relative path -> `sha256:<hex>`, or `None` for a file that was missing. Names what changed.
    #[serde(default)]
    pub files: BTreeMap<String, Option<String>>,
    /// The agent command line shown at consent, flags included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustFile {
    pub version: u32,
    pub records: BTreeMap<String, TrustRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TickSource {
    Job,
    Serve,
    Cli,
}

/// One finished check of an `on.watch` script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchState {
    pub checked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<String>,
    /// `sha256:<hex>` of the output that started the last run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_for: Option<String>,
}

/// Durable tick state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick_by: Option<TickSource>,
    /// key -> ISO of the last cron instant claimed.
    #[serde(default)]
    pub claimed: BTreeMap<String, String>,
    /// Repository roots to discover in at tick time.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    #[serde(default)]
    pub watches: BTreeMap<String, WatchState>,
    #[serde(default)]
    pub paused_watches: BTreeMap<String, String>,
    #[serde(default)]
    pub dismissed_runs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    Skipped,
    Failed,
    Missed,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind {
    Timeout,
    ModelUnavailable,
    Hook,
    Agent,
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Schedule,
    Manual,
    Watch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Pre,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookKind {
    Script,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookStatus {
    Ok,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookResult {
    pub phase: Phase,
    pub index: usize,
    pub kind: HookKind,
    /// Script relative path, or `prompt`.
    pub source: String,
    pub status: HookStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Where a node of a tracked run's chart got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Active,
    Waiting,
    Done,
    Failed,
    Skipped,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStep {
    pub id: String,
    pub label: String,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

/// Always http or https.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLink {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunNote {
    pub ts: String,
    pub text: String,
    #[serde(default)]
    pub links: Vec<RunLink>,
}

/// One thing a tracked run reported, in the order it happened: every step it set, every arrow it
/// took with why, every update, note and message. The decisions are the `next` entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum HistoryEntry {
    /// A node set directly: `step <node> --status <s> [-m text]`.
    Step {
        at: String,
        node: String,
        status: StepStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// An arrow taken: `next <to> --from <from> [-m why]`.
    Next {
        at: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// An update on a node: `say <text>`.
    Say {
        at: String,
        node: String,
        text: String,
    },
    /// Something the run published: `note <text> --link …`.
    Note {
        at: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<RunLink>,
    },
    /// The run's message for its reader: `message <text> --link …`.
    Message {
        at: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<RunLink>,
    },
    /// The run ended.
    Close { at: String, outcome: String },
}

/// What a tracked run reported, folded into the record at its end.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgress {
    pub chart: String,
    pub steps: Vec<RunStep>,
    /// Arrows taken, `(from, to)`.
    pub taken: Vec<(String, String)>,
    pub notes: Vec<RunNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<RunNote>,
    /// Everything it reported, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryEntry>,
}

/// One line of an automation's run history. Every run, and every skipped or missed instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    /// `20260917-140047-3fa2`
    pub id: String,
    pub key: String,
    pub name: String,
    pub scope: Scope,
    pub root: PathBuf,
    pub trigger: Trigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    pub late: bool,
    pub recorded_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
    /// The agent CLI that ran the body, e.g. `claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<CliIdName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<HookResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<RunOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<RunStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taken: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<RunNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<RunNote>,
    /// Everything the run reported on its flowchart, in order: steps, decisions, updates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryEntry>,
    /// Missed records: how many instants this one covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// The agent CLI name as stored in records (`pi`, `copilot`, `claude`, `codex`, `gemini`,
/// `opencode`). A string, so records from a newer fm still load.
pub type CliIdName = String;

/// Who holds an automation's run lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunLockInfo {
    pub run_id: String,
    pub pid: u32,
    /// Process start time, so a reused pid is not mistaken for the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_started_at: Option<String>,
    pub started_at: String,
    pub trigger: Trigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<CliIdName>,
}

/// Everything the list page needs about one automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub key: String,
    pub name: String,
    pub scope: Scope,
    pub dir: PathBuf,
    pub state: AutomationState,
    /// For invalid and off: one line saying why.
    pub reason: Option<String>,
    pub problems: Vec<String>,
    pub automation: Option<Automation>,
    /// The CLI that would run it, after config default and per-automation override.
    pub agent: Option<CliIdName>,
    pub effective_timeout_ms: u64,
    /// ISO with offset; armed automations with a schedule only.
    pub next_fire: Option<String>,
    pub next_check: Option<String>,
    pub watch: Option<WatchState>,
    pub watch_paused_at: Option<String>,
    pub running: Option<RunLockInfo>,
    /// Has a Mermaid chart to track progress on.
    pub flow: bool,
    pub last: Option<LastRun>,
    /// pi-automations has this same automation armed too, so it could run twice.
    pub also_armed_in_pa: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    pub outcome: Outcome,
    pub at: String,
    pub reason: Option<String>,
    pub duration_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_session_id_is_left_out_and_reads_back() {
        let s = SessionReference {
            file: "s.jsonl".into(),
            id: String::new(),
            format: Some("copilot/jsonl".into()),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("\"id\""), "{json}");
        let back: SessionReference = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}

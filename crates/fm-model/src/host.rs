//! What the TUI asks of the host. The real implementation lives in the `fm` crate; the TUI is built
//! and tested against `fm_tui::fake::FakeHost`.

use crate::{CliId, LiveEvent, RunRecord, Status};

/// What arming would allow, shown before consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentPreview {
    pub key: String,
    pub name: String,
    /// One line per trigger, e.g. `schedule: 0 9 * * 1-5 (America/Los_Angeles), catch-up once`.
    pub triggers: Vec<String>,
    pub model: String,
    pub timeout: String,
    pub agent: Option<CliId>,
    /// The exact agent command line, flags included.
    pub agent_command: Option<String>,
    /// `(phase, what runs)`, the exact command for scripts.
    pub hooks: Vec<(String, String)>,
    /// `(step id, what runs it)`: each `nodes:` entry's agent, model and command.
    pub nodes: Vec<(String, String)>,
    /// `on.watch`: the exact command and how often.
    pub watch: Option<String>,
    /// Files whose content the consent is pinned to.
    pub files: Vec<String>,
    /// Shown above everything, e.g. "also armed in pi-automations — it would run twice".
    pub warnings: Vec<String>,
    /// Repository scope: when consent expires.
    pub expires: Option<String>,
}

/// The result of an action, in one line for the status bar.
pub type ActionResult = Result<String, String>;

/// A tracked run's progress, read while it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveRun {
    pub run_id: String,
    pub events: Vec<LiveEvent>,
    pub progress: Option<crate::RunProgress>,
}

pub trait Host {
    /// Re-discover and return every automation, in list order.
    fn list(&mut self) -> Vec<Status>;
    /// Newest first, at most `limit`.
    fn runs(&mut self, key: &str, limit: usize) -> Vec<RunRecord>;
    /// The running run's live view, if one is going.
    fn live(&mut self, key: &str) -> Option<LiveRun>;
    fn preview(&mut self, key: &str) -> Result<ConsentPreview, String>;
    fn arm(&mut self, key: &str) -> ActionResult;
    fn disarm(&mut self, key: &str) -> ActionResult;
    fn set_enabled(&mut self, key: &str, enabled: bool) -> ActionResult;
    /// Start a manual run in the background. Refused if one is going.
    fn run_now(&mut self, key: &str) -> ActionResult;
    fn stop(&mut self, key: &str) -> ActionResult;
    fn pause_watch(&mut self, key: &str, paused: bool) -> ActionResult;
    /// Set or clear (`None`) this automation's agent CLI override.
    fn set_agent(&mut self, key: &str, agent: Option<CliId>) -> ActionResult;
    /// CLIs found on this machine, with their versions.
    fn agents(&mut self) -> Vec<(CliId, Option<String>)>;
    /// The `AUTOMATION.md` path to open in an editor.
    fn edit_path(&mut self, key: &str) -> Option<std::path::PathBuf>;
    /// Draw a chart with node statuses as text, `width` columns wide. `Err` falls back to the step
    /// list.
    fn render_chart(
        &mut self,
        chart: &str,
        progress: Option<&crate::RunProgress>,
        width: u16,
    ) -> Result<String, String>;
    /// The watcher (OS job or `fm serve`): one line, e.g. `on — last tick 12s ago`.
    fn watcher_status(&mut self) -> String;
}

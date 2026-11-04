//! The glyph vocabulary, the only place these characters appear. Mirrors pi-automations'
//! `glyphs.ts`, with an ASCII set for terminals and fonts that cannot draw them.

use fm_model::{Outcome, StepStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    pub armed: &'static str,
    pub off: &'static str,
    pub disabled: &'static str,
    pub invalid: &'static str,
    pub ok: &'static str,
    pub failed: &'static str,
    pub problem: &'static str,
    pub paused: &'static str,
    pub active: &'static str,
    pub todo: &'static str,
    pub waiting: &'static str,
    pub skipped: &'static str,
    pub interrupted: &'static str,
    pub unknown: &'static str,
    pub missed: &'static str,
    pub aborted: &'static str,
    pub link: &'static str,
    pub reason: &'static str,
    pub cursor: &'static str,
    pub dash: &'static str,
    pub ellipsis: &'static str,
    pub more_above: &'static str,
    pub more_below: &'static str,
}

impl Glyphs {
    pub const UNICODE: Glyphs = Glyphs {
        armed: "●",
        off: "◌",
        disabled: "○",
        invalid: "✗",
        ok: "✓",
        failed: "✗",
        problem: "!",
        paused: "‖",
        active: "▶",
        todo: "·",
        waiting: "⏸",
        skipped: "↷",
        interrupted: "‼",
        unknown: "?",
        missed: "–",
        aborted: "■",
        link: "↗",
        reason: "└",
        cursor: "❯",
        dash: "—",
        ellipsis: "…",
        more_above: "↑ more above",
        more_below: "↓ more below",
    };

    pub const ASCII: Glyphs = Glyphs {
        armed: "*",
        off: "o",
        disabled: "-",
        invalid: "x",
        ok: "+",
        failed: "x",
        problem: "!",
        paused: "=",
        active: ">",
        todo: ".",
        waiting: "=",
        skipped: "~",
        interrupted: "!",
        unknown: "?",
        missed: "-",
        aborted: "#",
        link: "->",
        reason: "`",
        cursor: ">",
        dash: "-",
        ellipsis: "~",
        more_above: "^ more above",
        more_below: "v more below",
    };

    pub fn new(ascii: bool) -> Glyphs {
        if ascii {
            Glyphs::ASCII
        } else {
            Glyphs::UNICODE
        }
    }

    /// The mark beside a run in a list of runs.
    pub fn outcome(&self, outcome: Outcome) -> &'static str {
        match outcome {
            Outcome::Ok => self.ok,
            Outcome::Failed => self.failed,
            Outcome::Skipped => self.skipped,
            Outcome::Missed => self.missed,
            Outcome::Aborted => self.aborted,
        }
    }

    /// The step tree on a run page: waiting shows as active, interrupted as failed.
    pub fn step_tree(&self, status: StepStatus) -> &'static str {
        match status {
            StepStatus::Pending => self.todo,
            StepStatus::Active | StepStatus::Waiting => self.active,
            StepStatus::Done => self.ok,
            StepStatus::Failed | StepStatus::Interrupted => self.failed,
            StepStatus::Skipped => self.skipped,
        }
    }

    /// The step list that stands in for a flowchart: one glyph per node status.
    pub fn step_list(&self, status: StepStatus) -> &'static str {
        match status {
            StepStatus::Pending => self.todo,
            StepStatus::Active => self.active,
            StepStatus::Waiting => self.waiting,
            StepStatus::Done => self.ok,
            StepStatus::Failed => self.failed,
            StepStatus::Skipped => self.skipped,
            StepStatus::Interrupted => self.interrupted,
        }
    }
}

impl Default for Glyphs {
    fn default() -> Self {
        Glyphs::UNICODE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn every_status_glyph_is_one_column() {
        for g in [Glyphs::UNICODE, Glyphs::ASCII] {
            for s in [
                g.armed, g.off, g.disabled, g.ok, g.failed, g.active, g.todo, g.skipped, g.cursor,
            ] {
                assert_eq!(s.width(), 1, "{s:?}");
            }
        }
    }

    #[test]
    fn ascii_set_is_ascii() {
        let g = Glyphs::ASCII;
        for s in [g.armed, g.ok, g.link, g.more_above, g.dash, g.ellipsis] {
            assert!(s.is_ascii(), "{s:?}");
        }
    }

    #[test]
    fn step_tree_folds_waiting_and_interrupted() {
        let g = Glyphs::UNICODE;
        assert_eq!(g.step_tree(StepStatus::Waiting), "▶");
        assert_eq!(g.step_tree(StepStatus::Interrupted), "✗");
        assert_eq!(g.step_list(StepStatus::Waiting), "⏸");
        assert_eq!(g.step_list(StepStatus::Interrupted), "‼");
    }
}

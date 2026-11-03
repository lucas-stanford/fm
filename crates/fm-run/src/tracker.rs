//! The run's own moves on its progress: hooks and the body move `pre0`, `body`, `post0`… when the
//! chart has them. Problems are swallowed after the first: tracking never changes a run.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use fm_model::{RunProgress, StatePaths, StepStatus};
use fm_spec::{flow_order, parse_chart, successors};
use fm_state::{
    ChartNode, NextArgs, OpenArgs, SayArgs, StepArgs, close_progress, open_progress, progress_next,
    progress_say, progress_step,
};

use crate::guide::{notes_guide, progress_guide};

/// The variables that let anything a run starts report to it.
pub const ENV_RUN: &str = "AUTOMATION_RUN";
pub const ENV_NODE: &str = "AUTOMATION_NODE";
pub const ENV_STATE_DIR: &str = "AUTOMATION_STATE_DIR";

/// A chart's nodes in flow order, with their successors, as the progress store wants them.
pub fn chart_nodes(chart: &str) -> Vec<ChartNode> {
    let parsed = parse_chart(chart);
    flow_order(&parsed)
        .into_iter()
        .map(|n| ChartNode {
            successors: successors(&parsed, &n.id)
                .into_iter()
                .map(str::to_string)
                .collect(),
            id: n.id,
            label: n.label,
        })
        .collect()
}

pub(crate) struct Tracker {
    paths: StatePaths,
    run_id: String,
    chart: Option<String>,
    strict: bool,
    started: bool,
    quiet: bool,
    known: HashSet<String>,
    active: Option<String>,
}

impl Tracker {
    /// Open the run's progress. With no chart, only notes and a message are kept.
    pub(crate) fn start(
        paths: &StatePaths,
        run_id: &str,
        key: &str,
        name: &str,
        chart: Option<String>,
        strict: bool,
        now: DateTime<Utc>,
    ) -> Tracker {
        let mut t = Tracker {
            paths: paths.clone(),
            run_id: run_id.to_string(),
            chart,
            strict,
            started: false,
            quiet: false,
            known: HashSet::new(),
            active: None,
        };
        let nodes = t.chart.as_deref().map(chart_nodes).unwrap_or_default();
        let args = OpenArgs {
            run_id: run_id.to_string(),
            key: key.to_string(),
            name: name.to_string(),
            chart: t.chart.clone().unwrap_or_default(),
            nodes,
            strict,
        };
        match open_progress(paths, &args, now) {
            Ok(snap) => {
                t.known = snap.nodes.iter().map(|n| n.id.clone()).collect();
                t.started = true;
            }
            Err(_) => t.quiet = true,
        }
        t
    }

    fn charted(&self) -> bool {
        !self.known.is_empty()
    }

    fn live(&self, node: &str) -> bool {
        self.started && !self.quiet && self.known.contains(node)
    }

    fn check<T, E>(&mut self, r: Result<T, E>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(_) => {
                self.quiet = true;
                None
            }
        }
    }

    /// Settle the node going now and start `node`, recording the arrow.
    pub(crate) fn next(&mut self, node: &str, text: Option<&str>, now: DateTime<Utc>) {
        if !self.live(node) {
            return;
        }
        let text = text.map(str::to_string);
        let r = match self.active.clone().filter(|a| self.known.contains(a)) {
            Some(from) => progress_next(
                &self.paths,
                Some(&self.run_id),
                &NextArgs {
                    to: node.to_string(),
                    from: Some(from),
                    env_node: None,
                    text,
                },
                now,
            ),
            None => progress_step(
                &self.paths,
                Some(&self.run_id),
                &StepArgs {
                    node: node.to_string(),
                    status: None,
                    text,
                },
                now,
            ),
        };
        if self.check(r).is_some() {
            self.active = Some(node.to_string());
        }
    }

    /// Start `node` without settling another.
    pub(crate) fn step(&mut self, node: &str, text: Option<&str>, now: DateTime<Utc>) {
        if !self.live(node) {
            return;
        }
        let r = progress_step(
            &self.paths,
            Some(&self.run_id),
            &StepArgs {
                node: node.to_string(),
                status: None,
                text: text.map(str::to_string),
            },
            now,
        );
        if self.check(r).is_some() {
            self.active = Some(node.to_string());
        }
    }

    pub(crate) fn mark(
        &mut self,
        node: &str,
        status: StepStatus,
        text: Option<&str>,
        now: DateTime<Utc>,
    ) {
        if !self.live(node) {
            return;
        }
        let r = progress_step(
            &self.paths,
            Some(&self.run_id),
            &StepArgs {
                node: node.to_string(),
                status: Some(status),
                text: text.map(str::to_string),
            },
            now,
        );
        self.check(r);
    }

    /// An update on the node going now.
    pub(crate) fn say(&mut self, text: &str, now: DateTime<Utc>) {
        let Some(target) = self.active.clone() else {
            return;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() || !self.live(&target) {
            return;
        }
        let r = progress_say(
            &self.paths,
            Some(&self.run_id),
            &SayArgs {
                text: trimmed.to_string(),
                node: Some(target),
                env_node: None,
                request_id: None,
            },
            now,
        );
        self.check(r);
    }

    /// Close the progress and return what the run reported.
    pub(crate) fn end(&mut self, outcome: &str, now: DateTime<Utc>) -> Option<RunProgress> {
        if !self.started {
            return None;
        }
        self.started = false;
        close_progress(&self.paths, &self.run_id, outcome, now)
            .ok()
            .flatten()
    }

    /// `AUTOMATION_RUN`, `AUTOMATION_STATE_DIR`, and `AUTOMATION_NODE` only when the chart has `node`, so a defaulted
    /// `fm next` finds the running node instead.
    pub(crate) fn env(&self, node: &str) -> Vec<(String, String)> {
        if !self.started {
            return Vec::new();
        }
        let mut env = vec![
            (ENV_RUN.to_string(), self.run_id.clone()),
            (
                ENV_STATE_DIR.to_string(),
                self.paths.root.to_string_lossy().into_owned(),
            ),
        ];
        if self.known.contains(node) {
            env.push((ENV_NODE.to_string(), node.to_string()));
        }
        env
    }

    /// The block appended to the body prompt.
    pub(crate) fn guide(&self) -> Option<String> {
        if !self.started {
            return None;
        }
        match &self.chart {
            Some(chart) if self.charted() => Some(progress_guide(chart, self.strict)),
            _ => Some(notes_guide()),
        }
    }
}

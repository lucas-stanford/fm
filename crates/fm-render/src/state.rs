//! The `--state` document: each node's status and which arrows were taken.
//!
//! ```json
//! {"nodes": {"build": "done", "test": "active"}, "taken": [["build", "test"]]}
//! ```
//!
//! A node that is not listed is pending. Unknown fields are ignored, but an id
//! or arrow the flowchart does not have is an error rather than silently not
//! drawn.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{Code, Result, fail};
use crate::flow::Flow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Pending,
    Active,
    Waiting,
    Done,
    Failed,
    Skipped,
    Interrupted,
}

impl Status {
    pub const ALL: &'static [Status] = &[
        Status::Pending,
        Status::Active,
        Status::Waiting,
        Status::Done,
        Status::Failed,
        Status::Skipped,
        Status::Interrupted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.as_str() == value)
    }

    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "·",
            Self::Active => "▶",
            Self::Waiting => "⏸",
            Self::Done => "✓",
            Self::Failed => "✗",
            Self::Skipped => "↷",
            Self::Interrupted => "‼",
        }
    }
}

/// A state checked against its flowchart.
#[derive(Debug, Clone, Default)]
pub struct State {
    statuses: BTreeMap<String, Status>,
    taken: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    nodes: BTreeMap<String, String>,
    #[serde(default)]
    taken: Vec<(String, String)>,
}

impl State {
    pub fn parse(json: &str, flow: &Flow) -> Result<Self> {
        let document: Document = serde_json::from_str(json)
            .map_err(|error| fail!(Code::State, "state is not valid JSON: {error}"))?;

        let mut statuses = BTreeMap::new();
        for (id, value) in document.nodes {
            if flow.node(&id).is_none() {
                return Err(fail!(
                    Code::State,
                    "state names node '{id}', which is not in the flowchart"
                ));
            }
            let Some(status) = Status::parse(&value) else {
                let known: Vec<&str> = Status::ALL.iter().map(|s| s.as_str()).collect();
                return Err(fail!(
                    Code::State,
                    "node '{id}' has status '{value}'; expected one of {}",
                    known.join(", ")
                ));
            };
            statuses.insert(id, status);
        }

        for (from, to) in &document.taken {
            for id in [from, to] {
                if flow.node(id).is_none() {
                    return Err(fail!(
                        Code::State,
                        "taken arrow {from} -> {to} names node '{id}', which is not in the flowchart"
                    ));
                }
            }
            if !flow
                .edges
                .iter()
                .any(|edge| &edge.from == from && &edge.to == to)
            {
                return Err(fail!(
                    Code::State,
                    "taken arrow {from} -> {to} is not an arrow of the flowchart"
                ));
            }
        }

        Ok(Self {
            statuses,
            taken: document.taken,
        })
    }

    pub fn status(&self, id: &str) -> Status {
        self.statuses.get(id).copied().unwrap_or_default()
    }

    pub fn taken(&self, from: &str, to: &str) -> bool {
        self.taken
            .iter()
            .any(|(taken_from, taken_to)| taken_from == from && taken_to == to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::parse;

    fn flow() -> Flow {
        parse::parse("flowchart LR\n a[A] --> b[B]\n b -->|yes| c[C]\n").expect("plan")
    }

    #[test]
    fn statuses_and_taken_edges_are_read() {
        let state = State::parse(
            r#"{"nodes": {"a": "done", "b": "active"}, "taken": [["a", "b"]]}"#,
            &flow(),
        )
        .expect("state");
        assert_eq!(state.status("a"), Status::Done);
        assert_eq!(state.status("b"), Status::Active);
        assert!(state.taken("a", "b"));
        assert!(!state.taken("b", "c"));
    }

    #[test]
    fn a_missing_node_is_pending_and_unknown_fields_are_ignored() {
        let state = State::parse(r#"{"nodes": {}, "extra": 1}"#, &flow()).expect("state");
        assert_eq!(state.status("c"), Status::Pending);
        let state = State::parse("{}", &flow()).expect("empty document");
        assert_eq!(state.status("a"), Status::Pending);
    }

    #[test]
    fn every_status_name_round_trips() {
        for status in Status::ALL {
            assert_eq!(Status::parse(status.as_str()), Some(*status));
        }
        assert_eq!(Status::parse("running"), None);
    }

    #[test]
    fn an_unknown_node_is_an_error() {
        let error = State::parse(r#"{"nodes": {"zzz": "done"}}"#, &flow()).expect_err("unknown");
        assert_eq!(error.code, Code::State);
        assert!(error.message.contains("'zzz'"), "{}", error.message);
    }

    #[test]
    fn an_unknown_status_is_an_error() {
        let error = State::parse(r#"{"nodes": {"a": "finished"}}"#, &flow()).expect_err("status");
        assert_eq!(error.code, Code::State);
        assert!(error.message.contains("finished"), "{}", error.message);
    }

    #[test]
    fn a_taken_edge_must_exist_in_the_plan() {
        let error =
            State::parse(r#"{"taken": [["a", "zzz"]]}"#, &flow()).expect_err("unknown node");
        assert!(error.message.contains("'zzz'"), "{}", error.message);
        let error = State::parse(r#"{"taken": [["a", "c"]]}"#, &flow()).expect_err("no edge");
        assert!(error.message.contains("not an arrow"), "{}", error.message);
    }

    #[test]
    fn malformed_json_is_an_error() {
        let error = State::parse("{nodes:", &flow()).expect_err("bad json");
        assert_eq!(error.code, Code::State);
        let error = State::parse(r#"{"taken": [["a"]]}"#, &flow()).expect_err("bad pair");
        assert_eq!(error.code, Code::State);
    }
}

//! A run's flowchart with its node statuses, drawn by fm-render in this process.

use std::collections::BTreeMap;

use fm_model::{RunProgress, StepStatus};
use serde::Serialize;

/// `{"nodes": {"<id>": "<status>"}, "taken": [["<from>", "<to>"]]}`
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChartState {
    pub nodes: BTreeMap<String, StepStatus>,
    pub taken: Vec<(String, String)>,
}

impl ChartState {
    pub fn of(progress: Option<&RunProgress>) -> ChartState {
        let Some(p) = progress else {
            return ChartState::default();
        };
        ChartState {
            nodes: p.steps.iter().map(|s| (s.id.clone(), s.status)).collect(),
            taken: p.taken.clone(),
        }
    }
}

/// Draw `chart` with node statuses as text, in-process, as `fm render --format text` does.
pub fn render(chart: &str, state: &ChartState) -> Result<String, String> {
    let json = serde_json::to_string(state).map_err(|e| format!("flowchart: {e}"))?;
    fm_render::render_text(chart, &json).map_err(|e| format!("flowchart: {e}"))
}

/// The widest line, in columns (every character counted as one).
pub fn width_of(text: &str) -> usize {
    text.lines().map(|l| l.chars().count()).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_model::RunStep;

    #[test]
    fn state_json_matches_fm() {
        let p = RunProgress {
            chart: "flowchart TD\n a --> b".into(),
            steps: vec![RunStep {
                id: "a".into(),
                label: "A".into(),
                status: StepStatus::Done,
                text: None,
                started_at: None,
                ended_at: None,
            }],
            taken: vec![("a".into(), "b".into())],
            notes: vec![],
            message: None,
        };
        let json = serde_json::to_string(&ChartState::of(Some(&p))).unwrap();
        assert_eq!(json, r#"{"nodes":{"a":"done"},"taken":[["a","b"]]}"#);
        assert_eq!(
            serde_json::to_string(&ChartState::of(None)).unwrap(),
            r#"{"nodes":{},"taken":[]}"#
        );
    }

    #[test]
    fn draws_in_process() {
        let state = ChartState {
            nodes: [("a".to_string(), StepStatus::Done)].into_iter().collect(),
            taken: vec![("a".into(), "b".into())],
        };
        let text = render("flowchart TD\n a[Start] --> b[Next]", &state).unwrap();
        assert!(text.contains("Start ✓"), "{text}");
        assert!(
            render("not a chart", &ChartState::default())
                .unwrap_err()
                .starts_with("flowchart: ")
        );
    }

    #[test]
    fn width_counts_columns() {
        assert_eq!(width_of("ab\n┌──┐"), 4);
        assert_eq!(width_of(""), 0);
    }
}

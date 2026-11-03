//! Rendering: the plan plus its state, decorated as Mermaid, drawn by merman, or laid out as a
//! rail when merman's text layout cannot route it.

pub mod decorate;
pub mod rail;
pub mod svg;
pub mod text;
pub mod theme;

#[cfg(test)]
pub(crate) mod fixture {
    use crate::flow::{Flow, parse};
    use crate::state::State;

    /// A branch, labelled edges, a cycle back to `build`, and a rhombus.
    pub const PLAN: &str = "flowchart LR\n  \
        fetch[Fetch source] --> build[Build binary] --> test{Tests pass?}\n  \
        test -->|yes| deploy[Deploy]\n  \
        test -->|no| fix[Fix and retry] --> build\n";

    pub fn flow() -> Flow {
        parse::parse(PLAN).expect("fixture plan")
    }

    pub fn state(json: &str) -> State {
        State::parse(json, &flow()).expect("fixture state")
    }
}

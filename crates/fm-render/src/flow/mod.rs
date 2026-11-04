//! The parts of a plan fm needs: node ids and labels, and edges in source order.
//!
//! merman draws the plan from its source text; this model exists because
//! `linkStyle <n>` addresses edges by declaration index, which merman's public
//! API does not expose.

pub mod parse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    /// The authored label, or the id for a node written without a shape.
    pub label: String,
    shaped: bool,
}

/// One edge. Its index in [`Flow::edges`] is its `linkStyle` index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// The text on the arrow, written `-->|yes|` or `-- yes -->`.
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flow {
    /// In order of first mention.
    pub nodes: Vec<Node>,
    /// In declaration order; never re-sort.
    pub edges: Vec<Edge>,
}

impl Flow {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

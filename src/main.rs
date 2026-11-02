//! fm: draw a Mermaid flowchart with node statuses, as SVG or text.

mod cli;
mod error;
mod flow;
mod render;
mod state;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::main()
}

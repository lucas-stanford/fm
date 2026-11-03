//! fm: draw a Mermaid flowchart with node statuses, as SVG or text.

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    fm_render::cli::render_command(&argv)
}

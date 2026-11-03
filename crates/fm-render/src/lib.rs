//! fm's renderer: draw a Mermaid flowchart with node statuses, as SVG or text.
//!
//! `fm render` is [`cli::render_command`]; hosts that draw a run's chart in-process use
//! [`render_text`] with the same state JSON the command reads.

pub mod cli;
pub mod error;
pub mod flow;
pub mod render;
pub mod state;

pub use error::{Code, Failure};

/// `plan` with the node statuses in `state_json` as a Markdown ```mermaid block, for a page that
/// draws Mermaid itself, as `fm render --format markdown` prints it.
pub fn render_markdown(plan: &str, state_json: &str) -> error::Result<String> {
    let flow = flow::parse::parse(plan)?;
    flow::parse::validate_renderable(plan)?;
    let state = state::State::parse(state_json, &flow)?;
    Ok(render::markdown::render(
        plan,
        &flow,
        &state,
        render::theme::Theme::Light,
    ))
}

/// Draw `plan` with the node statuses in `state_json` (`{"nodes": {...}, "taken": [...]}`) as
/// text, exactly as `fm render --format text` prints it, without the trailing newline.
pub fn render_text(plan: &str, state_json: &str) -> error::Result<String> {
    let flow = flow::parse::parse(plan)?;
    flow::parse::validate_renderable(plan)?;
    let state = state::State::parse(state_json, &flow)?;
    Ok(render::text::render(plan, &flow, &state)?
        .trim_end()
        .to_string())
}

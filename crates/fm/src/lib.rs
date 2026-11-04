//! The `fm` binary's library: the clap CLI ([`cli`]), the real [`fm_model::Host`] the TUI runs on
//! ([`host::RealHost`]), and the seams to the run engine and the clock ([`seams`]).

pub mod cli;
pub mod env;
pub mod fm;
pub mod host;
pub mod notify;
pub mod proc;
pub mod resolve;
pub mod seams;
pub mod text;

//! The run engine: lock, hooks, the agent session, the record, and progress reporting.
//!
//! - [`fire()`]: one run, start to finish, in the specification's order, leaving one record.
//! - [`turn`]: one agent turn, streamed into the live file and the run's transcript.
//! - [`progress_cli()`]: `fm step|next|say|note|message|status`, the verbs a run reports with.
//! - [`process`]: bounded child processes whose whole tree dies on a timeout or a stop.

pub mod context;
pub mod fire;
pub mod guide;
pub mod live;
pub mod markdown;
pub mod node;
pub mod process;
pub mod progress_cli;
pub mod stop;
pub mod summary;
mod tracker;
pub mod turn;
pub mod worktree;

pub use context::{InputItem, RunInfo, body_prompt, detect_skip, escape_output, input_block};
pub use fire::{FireDeps, FireError, FireOptions, Notify, STOP_REASON, fire};
pub use guide::{notes_guide, progress_guide};
pub use live::{LiveWriter, read_live};
pub use progress_cli::{CliOutput, in_run_help_lines, is_in_run_verb, progress_cli};
pub use tracker::{ENV_NODE, ENV_RUN, ENV_STATE_DIR, chart_nodes};

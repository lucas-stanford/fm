//! Parse and validate `AUTOMATION.md`, cron, discovery, interpreters, digests and charts.
//!
//! Everything here reads; nothing writes. The behaviour follows the `site/content/` specification
//! and, where it leaves a choice, pi-automations. Every refusal is one line,
//! `<field path>: <what is wrong> — <what fixes it>`: see [`Refusal`].
//!
//! - [`parse_automation`]: the file checks, in the specification's order.
//! - [`cron`]: the Draft 0.1.0 grammar and date arithmetic in a zone, daylight saving included.
//! - [`discover()`], [`check_machine`]: finding automations and the checks that need the machine.
//! - [`resolve_interpreter`]: which program starts a script, per OS.
//! - [`digest_of`]: the consent pin.
//! - [`parse_chart`], [`flow_order`], [`check_strict_flow`]: Mermaid charts and `flow: strict`.

pub mod chart;
pub mod cron;
pub mod digest;
pub mod discover;
pub mod duration;
pub mod flow_block;
pub mod interpreters;
pub mod parse;
pub mod refusal;
pub mod strict_flow;
pub mod yaml;

pub use chart::{
    Chart, ChartEdge, ChartNode, components, flow_order, hook_label, is_reserved_node,
    mermaid_label, node_id_for, parse_chart, plan_from_automation, successors,
};
pub use cron::{
    CronSchedule, Zone, clock_in, describe_next, format_stagger, instants_between, is_iana_zone,
    iso_with_offset, next_after, stagger_cap_for, stagger_seconds, validate_cron,
};
pub use digest::{
    Digest, ScriptRef, changed_file, changed_files, digest_of, file_hashes_of, script_files,
    script_path,
};
pub use discover::{
    AgentCheck, DiscoverContext, Machine, MachineReport, automation_key, by_key, check_machine,
    discover, find_escaping_script, find_repo_root, interpreter_map, located, normalize,
    repository_roots, roots_for_registered, user_root,
};
pub use duration::{format_duration, parse_duration, parse_duration_with_days};
pub use flow_block::{FlowBlock, find_flow_block, read_chart};
pub use interpreters::{
    Interpreter, Platform, Which, has_executable_bit, resolve_interpreter, which, which_in,
};
pub use parse::{
    FIELDS, THINKING_LEVELS, TRIGGERS, is_valid_name, parse_automation, parse_model_spec,
    set_enabled_in_place, split_frontmatter,
};
pub use refusal::Refusal;
pub use strict_flow::{FlowProblem, check_strict_flow, flow_problem_line, plain_text};

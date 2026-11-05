//! fm's state folder: config, consent, tick state, locks and run records.
//!
//! Everything here is a file in the state folder (`FM_HOME`, or the per-OS folder from [`folder`]),
//! laid out as [`fm_model::StatePaths`]. Several processes share it: the tick, `fm` commands, the
//! TUI and every run. So every JSON file is changed read-change-write under a short lock and
//! written atomically (a temporary sibling, then a rename): nothing ever writes back a stale copy.
//!
//! Functions that depend on the time take `now`; functions that ask whether a process is alive take
//! a [`ProcessProbe`], so tests can inject both. This crate never starts a process.

pub mod config;
pub mod consent;
pub mod duration;
pub mod error;
pub mod folder;
pub mod fsio;
pub mod keys;
pub mod locks;
pub mod pa_trust;
pub mod process;
pub mod progress;
pub mod records;
pub mod state;
pub mod time;

pub use config::{
    ConfigDuration, duration_ms, load_config, parse_config, save_config, update_config,
};
pub use consent::{ArmShown, Consent, OffReason, Pin, changed_files, read_trust};
pub use duration::{format_duration, parse_duration};
pub use error::{Result, StateError};
pub use folder::{Env, FakeEnv, Os, ProcessEnv, open_state, state_dir, state_paths};
pub use keys::{AutomationKey, normalize_path, same_path};
pub use locks::{
    Acquire, RunLock, RunLockFile, StopOutcome, TickLock, acquire_run_lock, acquire_tick_lock,
    clear_stop, is_run_lock_stale, own_lock_info, read_run_lock, request_stop, stop_requested,
    stop_run, sweep_stale_run_locks, update_run_lock,
};
pub use pa_trust::{PaTrust, is_armed_in_pa, pa_armed_keys, pa_trust_path};
pub use process::{FakeProbe, ProcessProbe, ProcessState, SystemProbe, owner_alive};
pub use progress::{
    ChartNode, LinkedArgs, NextArgs, OpenArgs, ProgressCode, ProgressError, ProgressNode,
    ProgressResult, ProgressSnapshot, SayArgs, StepArgs, TaskArgs, TasksArgs, close_progress,
    fold_abandoned, open_progress, progress_message, progress_next, progress_note, progress_of,
    progress_say, progress_status, progress_step, progress_task, progress_tasks, read_progress,
    with_progress,
};
pub use records::{append_record, last_record, new_run_id, read_records, rotate_if_needed};
pub use state::{StateFileExt, StateStore};

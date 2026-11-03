//! Shared types and traits for fm.
//!
//! This crate is the contract between the other crates, so each one can be built and tested against
//! fakes. It holds plain data and traits only: no I/O, no processes, no clocks.
//!
//! The shapes mirror pi-automations (`src/types.ts`) where that costs nothing. Timestamps are ISO 8601
//! strings with an offset, as there.

pub mod agent;
pub mod config;
pub mod host;
pub mod live;
pub mod paths;
pub mod types;

pub use agent::*;
pub use config::*;
pub use host::*;
pub use live::*;
pub use paths::*;
pub use types::*;

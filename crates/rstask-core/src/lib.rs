//! rstask-core — the single writer behind every RsTask interface.
//!
//! One core, one SQLite database. CLI/TUI/Web/MCP are thin shells over
//! [`Store`]; no interface owns data.

pub mod config;
pub mod error;
pub mod models;
pub mod store;

pub use config::Config;
pub use error::{ConfigError, Error, Result};
pub use models::{Actor, ActorKind, Claim, Event, Module, ReleaseReason, Task, TaskStatus};
pub use store::Store;

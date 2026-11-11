//! rstask-core — the single writer behind every RsTask interface.
//!
//! One core, one SQLite database. CLI/TUI/Web/MCP are thin shells over
//! [`Store`]; no interface owns data.

pub mod config;
pub mod error;
pub mod event;
pub mod models;
pub mod store;
pub mod sync;

pub use config::Config;
pub use error::{ConfigError, Error, Result};
pub use event::{EventId, Signable, StoredEvent};
pub use models::{
    Actor, ActorKind, Claim, Event, Module, Rejected, ReleaseReason, Submission, Submitted, Task,
    TaskDetail, TaskStatus,
};
pub use store::Store;
pub use sync::{ActorRow, Bucket, Merge, SyncBundle};

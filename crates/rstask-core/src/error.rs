//! Error type shared by all interfaces.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("task not found: {0}")]
    TaskNotFound(String),

    #[error("actor not found: {0}")]
    ActorNotFound(String),

    #[error("task {task} is already claimed by {actor}")]
    ClaimConflict { task: String, actor: String },

    #[error("invalid workflow transition: {from} --{via}--> {to} (task {task})")]
    InvalidTransition {
        task: String,
        from: String,
        via: String,
        to: String,
    },

    #[error("actor {actor} is not permitted to {action}")]
    NotPermitted { actor: String, action: String },

    #[error("task {task} is blocked until these are done: {by}")]
    Blocked { task: String, by: String },

    #[error("task {task} cannot wait on {by}: that would be a cycle")]
    DependencyCycle { task: String, by: String },

    #[error("claim expired for task {0}")]
    ClaimExpired(String),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("invalid event {id}: {reason}")]
    InvalidEvent { id: String, reason: String },

    #[error("no signing key for actor {0} in this key directory")]
    NoSigningKey(String),

    #[error("key does not belong to actor {0}")]
    KeyMismatch(String),

    #[error(
        "task {task} was resubmitted: you reviewed {reviewed}, the current submission is {current}"
    )]
    StaleApproval {
        task: String,
        reviewed: String,
        current: String,
    },

    #[error("network: {0}")]
    Net(String),

    #[error("invalid sync bundle: {0}")]
    InvalidBundle(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },

    #[error("invalid config TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

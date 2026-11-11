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

    #[error("claim expired for task {0}")]
    ClaimExpired(String),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Config(#[from] ConfigError),
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

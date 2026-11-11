//! Layered configuration: defaults → config.toml → environment → CLI overrides.
//!
//! Environment variables use the `RSTASK_` prefix, e.g. `RSTASK_SERVER_PORT`,
//! `RSTASK_STORAGE_DB_PATH`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub storage: StorageConfig,
    pub p2p: P2pConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    pub cors_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// `offline` (default) or `oidc`.
    pub mode: String,
    /// `ed25519` (default); RSA only for legacy consumers.
    pub key_algorithm: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub db_path: PathBuf,
    pub markdown_root: PathBuf,
    pub attachment_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct P2pConfig {
    pub enabled: bool,
    pub transport: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 7700,
            cors_origins: vec!["http://localhost:3000".into()],
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: "offline".into(),
            key_algorithm: "ed25519".into(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from(".rstask/rstask.sqlite"),
            markdown_root: PathBuf::from(".rstask/content"),
            attachment_root: PathBuf::from(".rstask/attachments"),
        }
    }
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: "quic".into(),
        }
    }
}

impl Config {
    /// Load configuration with the layering `defaults → file → env`.
    ///
    /// `path` defaults to `./config.toml`; a missing file is not an error.
    /// CLI flags should be applied by callers after this returns.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut cfg = Config::default();

        let path = path
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("config.toml"));
        if path.exists() {
            let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
                path: path.display().to_string(),
                source,
            })?;
            cfg = toml::from_str(&text).map_err(ConfigError::Parse)?;
        }

        cfg.apply_env();
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("RSTASK_SERVER_PORT") {
            if let Ok(port) = v.parse() {
                self.server.port = port;
            }
        }
        if let Ok(v) = std::env::var("RSTASK_SERVER_CORS_ORIGINS") {
            self.server.cors_origins = v.split(',').map(str::trim).map(String::from).collect();
        }
        if let Ok(v) = std::env::var("RSTASK_AUTH_MODE") {
            self.auth.mode = v;
        }
        if let Ok(v) = std::env::var("RSTASK_STORAGE_DB_PATH") {
            self.storage.db_path = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("RSTASK_STORAGE_MARKDOWN_ROOT") {
            self.storage.markdown_root = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("RSTASK_P2P_ENABLED") {
            self.p2p.enabled = matches!(v.as_str(), "1" | "true" | "yes");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env-layer tests mutate process env; serialize them so parallel
    /// tests never observe each other's `RSTASK_*` variables.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn defaults_match_spec() {
        let _g = ENV_LOCK.lock().unwrap();
        let cfg = Config::default();
        assert_eq!(cfg.server.port, 7700);
        assert_eq!(cfg.auth.mode, "offline");
        assert_eq!(cfg.auth.key_algorithm, "ed25519");
        assert_eq!(cfg.storage.db_path, PathBuf::from(".rstask/rstask.sqlite"));
        assert!(!cfg.p2p.enabled);
        assert_eq!(cfg.p2p.transport, "quic");
    }

    #[test]
    fn file_layer_overrides_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("rstask-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[server]\nport = 8800\n").unwrap();

        let cfg = Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.server.port, 8800);
        assert_eq!(cfg.auth.mode, "offline"); // untouched layer keeps default
    }

    #[test]
    fn env_beats_file() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("rstask-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[server]\nport = 8800\n").unwrap();

        // SAFETY: single-threaded test; env is unset at the end.
        unsafe { std::env::set_var("RSTASK_SERVER_PORT", "9900") };
        let cfg = Config::load(Some(&path)).unwrap();
        unsafe { std::env::remove_var("RSTASK_SERVER_PORT") };
        assert_eq!(cfg.server.port, 9900);
    }
}

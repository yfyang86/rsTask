//! Where actor secret keys live: outside the workspace database (PRD-v3 R1).
//!
//! The database holds public keys only. A process can act as an actor only
//! if its key directory holds `<actor id>.key`; give each agent a directory
//! with nothing but its own key.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use ed25519_dalek::SigningKey;

use crate::error::{Error, Result};

#[derive(Debug)]
pub(crate) enum Keyring {
    /// One hex-encoded `<actor id>.key` file per actor.
    Dir(PathBuf),
    /// For in-memory stores (tests).
    Memory(Mutex<HashMap<String, [u8; 32]>>),
}

impl Keyring {
    fn path(dir: &std::path::Path, actor_id: &str) -> PathBuf {
        dir.join(format!("{actor_id}.key"))
    }

    pub(crate) fn get(&self, actor_id: &str) -> Result<SigningKey> {
        let secret: Option<[u8; 32]> = match self {
            Keyring::Dir(dir) => std::fs::read_to_string(Self::path(dir, actor_id))
                .ok()
                .and_then(|hex| hex::decode(hex.trim()).ok())
                .and_then(|bytes| bytes.try_into().ok()),
            Keyring::Memory(map) => map.lock().expect("keyring lock").get(actor_id).copied(),
        };
        secret
            .map(|s| SigningKey::from_bytes(&s))
            .ok_or_else(|| Error::NoSigningKey(actor_id.to_string()))
    }

    pub(crate) fn put(&self, actor_id: &str, secret: &[u8; 32]) -> Result<()> {
        match self {
            Keyring::Dir(dir) => {
                std::fs::create_dir_all(dir)?;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                    options.mode(0o600);
                }
                let mut file = options.open(Self::path(dir, actor_id))?;
                writeln!(file, "{}", hex::encode(secret))?;
            }
            Keyring::Memory(map) => {
                map.lock()
                    .expect("keyring lock")
                    .insert(actor_id.to_string(), *secret);
            }
        }
        Ok(())
    }
}

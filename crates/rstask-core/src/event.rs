//! Signed, content-addressed event envelopes: the sync unit of the workspace DAG
//! (PRD-v2 D3, D4).
//!
//! ```text
//! signable = postcard(Signable { lamport, parents, actor_id, task_id, kind, payload, at })
//! id       = BLAKE3(signable)
//! sig      = Ed25519(signing_key, signable)
//! wire     = postcard(StoredEvent { body: Signable, pubkey, sig })
//! ```

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const EVENT_ID_LEN: usize = 32;
pub type EventId = [u8; 32];

/// The signed (and hashed) body of an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signable {
    pub lamport: u64,
    pub parents: Vec<EventId>,
    pub actor_id: String,
    pub task_id: String,
    pub kind: String,
    /// JSON object, e.g. `{ "title": "…" }`.
    pub payload: String,
    pub at: DateTime<Utc>,
}

/// Full envelope as stored in `events` and exchanged on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub body: Signable,
    pub pubkey: [u8; 32],
    #[serde(with = "sig64")]
    pub sig: [u8; 64],
}

/// serde supports arrays only up to 32 elements; the signature needs a nudge.
mod sig64 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(sig: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        sig.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let v = Vec::<u8>::deserialize(d)?;
        v.try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))
    }
}

impl StoredEvent {
    pub fn id(&self) -> EventId {
        signable_id(&self.body)
    }

    pub fn parent_ids(&self) -> &[EventId] {
        &self.body.parents
    }

    pub fn sign(signing_key: &SigningKey, body: Signable) -> StoredEvent {
        let signable = postcard::to_stdvec(&body).expect("postcard encode");
        let sig: Signature = signing_key.sign(&signable);
        StoredEvent {
            body,
            pubkey: signing_key.verifying_key().to_bytes(),
            sig: sig.to_bytes(),
        }
    }

    /// Recomputes the id, checks the signature. Returns the id on success.
    pub fn verify(&self) -> Result<EventId> {
        let signable = postcard::to_stdvec(&self.body).expect("postcard encode");
        let id = blake3::hash(&signable);
        let verifying_key =
            VerifyingKey::from_bytes(&self.pubkey).map_err(|e| Error::InvalidEvent {
                id: hex::encode(id.as_bytes()),
                reason: format!("bad pubkey: {e}"),
            })?;
        verifying_key
            .verify(&signable, &Signature::from_bytes(&self.sig))
            .map_err(|_| Error::InvalidEvent {
                id: hex::encode(id.as_bytes()),
                reason: "signature mismatch".into(),
            })?;
        Ok(*id.as_bytes())
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("postcard encode")
    }

    pub fn decode(bytes: &[u8]) -> Result<StoredEvent> {
        postcard::from_bytes(bytes).map_err(|e| Error::InvalidEvent {
            id: String::new(),
            reason: format!("undecodable event: {e}"),
        })
    }
}

pub fn signable_id(body: &Signable) -> EventId {
    *blake3::hash(&postcard::to_stdvec(body).expect("postcard encode")).as_bytes()
}

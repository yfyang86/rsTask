//! Sync bundles and set-reconciliation helpers (PRD-v2 D2).
//!
//! The wire/storage unit is [`SyncBundle`]: the postcard-encoded signed event
//! DAG. Actors are part of it, as `actor.created` events. Sprint 02's
//! network protocol carries the same bundle, bucket-diffed.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::event::EventId;

/// Lamport events per fingerprint bucket (sync step 1).
pub const BUCKET: u64 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub index: u64,
    pub count: u64,
    pub digest: EventId,
}

/// Outcome of applying a peer bundle.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Merge {
    pub added: usize,
    /// Waiting on missing parents.
    pub pending: usize,
    pub duplicates: usize,
    pub rejected: usize,
    /// Human-readable reasons, for CLI output and UAT evidence.
    pub rejected_ids: Vec<String>,
    /// Claims this replica held as live that the merge ended.
    pub lost_claims: Vec<LostClaim>,
}

/// A claim that was live here before a merge and is not after it — because
/// an earlier claim from elsewhere won the task, or a human force-released
/// it. The holder's own release or submission is not a loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LostClaim {
    pub task_id: String,
    pub claim_id: String,
    pub actor_id: String,
    /// `superseded` or `force_released`.
    pub reason: String,
}

/// Full-state sync unit: `rstask sync export|import` today, the payload of the
/// sprint-02 wire protocol tomorrow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBundle {
    /// Each entry is a postcard-encoded `StoredEvent`.
    pub events: Vec<Vec<u8>>,
}

pub fn encode_bundle(bundle: &SyncBundle) -> Vec<u8> {
    postcard::to_stdvec(bundle).expect("postcard encode")
}

pub fn decode_bundle(bytes: &[u8]) -> Result<SyncBundle> {
    postcard::from_bytes(bytes).map_err(|e| Error::InvalidBundle(e.to_string()))
}

/// Bucket indices whose fingerprint differs between two replicas
/// (or exists on only one side) — sync step 1 output.
pub fn differing_buckets(local: &[Bucket], remote: &[Bucket]) -> Vec<u64> {
    let mut diff = Vec::new();
    for l in local {
        match remote.iter().find(|r| r.index == l.index) {
            Some(r) if r.digest == l.digest && r.count == l.count => {}
            _ => diff.push(l.index),
        }
    }
    for r in remote {
        if !local.iter().any(|l| l.index == r.index) {
            diff.push(r.index);
        }
    }
    diff.sort_unstable();
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(index: u64, count: u64, digest: u8) -> Bucket {
        Bucket {
            index,
            count,
            digest: [digest; 32],
        }
    }

    #[test]
    fn buckets_diff_by_digest_or_presence() {
        let local = vec![bucket(0, 2, 1), bucket(1, 3, 2), bucket(2, 1, 3)];
        let remote = vec![bucket(0, 2, 1), bucket(1, 3, 9), bucket(3, 1, 4)];
        assert_eq!(differing_buckets(&local, &remote), vec![1, 2, 3]);
    }

    #[test]
    fn identical_buckets_diff_empty() {
        let local = vec![bucket(0, 2, 1)];
        assert!(differing_buckets(&local, &local).is_empty());
    }

    #[test]
    fn count_change_alone_differs() {
        let local = vec![bucket(0, 2, 1)];
        let remote = vec![bucket(0, 3, 1)];
        assert_eq!(differing_buckets(&local, &remote), vec![0]);
    }

    #[test]
    fn bundle_roundtrip() {
        let bundle = SyncBundle {
            events: vec![vec![1, 2, 3]],
        };
        let decoded = decode_bundle(&encode_bundle(&bundle)).unwrap();
        assert_eq!(decoded.events, bundle.events);
    }
}

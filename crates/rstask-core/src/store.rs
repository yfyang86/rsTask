//! Storage layer: the single writer behind every interface (CLI/TUI/Web/MCP).
//!
//! Invariants enforced here, regardless of caller:
//! - A task has at most one active claim (atomic, transactional).
//! - Stale claims (no heartbeat past TTL) are swept lazily on each claim.
//! - Status moves only along the workflow gate: queue → work → review → done.
//! - Every mutation appends to the immutable event log in the same transaction.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::event::{EventId, Signable, StoredEvent};
use crate::models::{Actor, ActorKind, Claim, Event, ReleaseReason, Task, TaskStatus};
use crate::sync::{ActorRow, Bucket, Merge, SyncBundle, BUCKET};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_event_dag.sql");
const SCHEMA_VERSION: i64 = 2;

/// Ordered, versioned migrations guarded by `PRAGMA user_version`, so reopening
/// an existing database is a no-op (ALTER migrations are not idempotent).
async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    let (version,): (i64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    if version < 1 {
        sqlx::raw_sql(MIGRATION_0001).execute(pool).await?;
    }
    if version < 2 {
        sqlx::raw_sql(MIGRATION_0002).execute(pool).await?;
    }
    if version != SCHEMA_VERSION {
        sqlx::raw_sql(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct Store {
    pool: SqlitePool,
    /// Keeps the shared in-memory database alive for `open_in_memory`.
    #[allow(dead_code)]
    keepalive: Option<SqliteConnection>,
}

impl Store {
    /// Open (creating if needed) the SQLite database and run migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        run_migrations(&pool).await?;
        Ok(Self {
            pool,
            keepalive: None,
        })
    }

    /// In-memory store for tests.
    ///
    /// Uses a shared-cache in-memory database: a dedicated keep-alive
    /// connection holds the database open so all pool connections share one
    /// schema (a plain `:memory:` database is per-connection).
    pub async fn open_in_memory() -> Result<Self> {
        let url = format!(
            "file:rstask_mem_{}?mode=memory&cache=shared",
            Uuid::new_v4()
        );
        let keepalive = SqliteConnection::connect(&url).await?;

        let options = SqliteConnectOptions::from_str(&url)?.foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        run_migrations(&pool).await?;
        Ok(Self {
            pool,
            keepalive: Some(keepalive),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---------------------------------------------------------------- actors

    pub async fn create_actor(&self, kind: ActorKind, name: &str) -> Result<Actor> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        sqlx::query("INSERT INTO actors (id, kind, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(kind)
            .bind(name)
            .bind(now)
            .execute(&self.pool)
            .await?;
        // Interim plaintext keystore inside the workspace DB (PRD-v2 D5).
        sqlx::query("INSERT INTO actor_keys (actor_id, pubkey, secret) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(signing_key.verifying_key().to_bytes().to_vec())
            .bind(signing_key.to_bytes().to_vec())
            .execute(&self.pool)
            .await?;
        Ok(Actor {
            id,
            kind,
            name: name.to_string(),
            created_at: now,
            last_heartbeat_at: None,
        })
    }

    /// The actor's Ed25519 signing key (local actors only).
    async fn signing_key(&self, actor_id: &str) -> Result<SigningKey> {
        let row: Option<(Option<Vec<u8>>,)> =
            sqlx::query_as("SELECT secret FROM actor_keys WHERE actor_id = ?")
                .bind(actor_id)
                .fetch_optional(&self.pool)
                .await?;
        match row.and_then(|(s,)| s) {
            Some(bytes) if bytes.len() == 32 => Ok(SigningKey::from_bytes(
                bytes.as_slice().try_into().expect("len checked"),
            )),
            _ => Err(Error::NoSigningKey(actor_id.to_string())),
        }
    }

    pub async fn get_actor(&self, id: &str) -> Result<Actor> {
        sqlx::query_as::<_, Actor>("SELECT * FROM actors WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::ActorNotFound(id.to_string()))
    }

    pub async fn heartbeat(&self, actor_id: &str) -> Result<()> {
        let res = sqlx::query("UPDATE actors SET last_heartbeat_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(actor_id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(Error::ActorNotFound(actor_id.to_string()));
        }
        Ok(())
    }

    // ----------------------------------------------------------------- tasks

    pub async fn create_task(&self, title: &str, body_md: &str, created_by: &str) -> Result<Task> {
        // Fail fast on unknown actor (FK would too, but with a worse message).
        self.get_actor(created_by).await?;
        let signing_key = self.signing_key(created_by).await?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO tasks (id, title, body_md, status, created_by, created_at, updated_at)
             VALUES (?, ?, ?, 'queue', ?, ?, ?)",
        )
        .bind(&id)
        .bind(title)
        .bind(body_md)
        .bind(created_by)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        self.append_event_tx(
            &mut tx,
            &signing_key,
            created_by,
            &id,
            "task.created",
            json!({ "title": title, "body_md": body_md }),
            now,
        )
        .await?;

        tx.commit().await?;
        self.get_task(&id).await
    }

    pub async fn get_task(&self, id: &str) -> Result<Task> {
        sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::TaskNotFound(id.to_string()))
    }

    pub async fn list_tasks(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        match status {
            Some(s) => {
                sqlx::query_as::<_, Task>(
                    "SELECT * FROM tasks WHERE status = ? ORDER BY priority DESC, created_at",
                )
                .bind(s)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<_, Task>("SELECT * FROM tasks ORDER BY priority DESC, created_at")
                    .fetch_all(&self.pool)
                    .await
            }
        }
        .map_err(Error::from)
    }

    /// Update title/body/priority. The caller must hold an active claim on
    /// the task (this is enforced by `task_update` in the MCP/agent path).
    pub async fn update_task(
        &self,
        id: &str,
        title: Option<&str>,
        body_md: Option<&str>,
        priority: Option<i64>,
        actor_id: &str,
    ) -> Result<Task> {
        let signing_key = self.signing_key(actor_id).await?;
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE tasks SET
                title     = COALESCE(?, title),
                body_md   = COALESCE(?, body_md),
                priority  = COALESCE(?, priority),
                updated_at = ?
             WHERE id = ?",
        )
        .bind(title)
        .bind(body_md)
        .bind(priority)
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        // Only the provided fields are replayed on merge (PRD-v2).
        let mut payload = serde_json::Map::new();
        if let Some(t) = title {
            payload.insert("title".into(), json!(t));
        }
        if let Some(b) = body_md {
            payload.insert("body_md".into(), json!(b));
        }
        if let Some(p) = priority {
            payload.insert("priority".into(), json!(p));
        }
        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            id,
            "task.updated",
            payload.into(),
            now,
        )
        .await?;

        tx.commit().await?;
        self.get_task(id).await
    }

    // ---------------------------------------------------------------- claims

    /// Atomically claim a task: sweeps expired claims, rejects if another
    /// active claim exists, moves queue → work, appends events.
    ///
    /// Returns `Error::ClaimConflict` if the task is already claimed, and
    /// `Error::InvalidTransition` if the task is not in `queue`.
    pub async fn claim_task(&self, task_id: &str, actor_id: &str, ttl: Duration) -> Result<Claim> {
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.pool.begin().await?;

        // Lazy sweep of expired claims (no heartbeat past TTL → back to pool).
        // Any `work` task left without an active claim returns to `queue`.
        sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = 'expired'
             WHERE released_at IS NULL AND expires_at <= ?",
        )
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE tasks SET status = 'queue', updated_at = ?
             WHERE status = 'work'
               AND id NOT IN (SELECT task_id FROM claims WHERE released_at IS NULL)",
        )
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;

        let task_row = sqlx::query("SELECT status FROM tasks WHERE id = ?")
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(task_row) = task_row else {
            return Err(Error::TaskNotFound(task_id.to_string()));
        };
        let status: TaskStatus = task_row.get("status");

        if status != TaskStatus::Queue {
            // If it's in `work`, surface who holds it.
            if status == TaskStatus::Work {
                let holder: Option<(String,)> = sqlx::query_as(
                    "SELECT actor_id FROM claims WHERE task_id = ? AND released_at IS NULL",
                )
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some((holder,)) = holder {
                    return Err(Error::ClaimConflict {
                        task: task_id.to_string(),
                        actor: holder,
                    });
                }
            }
            return Err(Error::InvalidTransition {
                task: task_id.to_string(),
                from: status.to_string(),
                via: "claim".into(),
                to: TaskStatus::Work.to_string(),
            });
        }

        self.require_actor_tx(&mut tx, actor_id).await?;

        let now = Utc::now();
        let claim = Claim {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
            actor_id: actor_id.to_string(),
            claimed_at: now,
            expires_at: now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX),
            released_at: None,
            release_reason: None,
        };

        sqlx::query(
            "INSERT INTO claims (id, task_id, actor_id, claimed_at, expires_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&claim.id)
        .bind(&claim.task_id)
        .bind(&claim.actor_id)
        .bind(claim.claimed_at)
        .bind(claim.expires_at)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE tasks SET status = 'work', updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;

        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            "task.claimed",
            json!({ "claim_id": claim.id, "expires_at": claim.expires_at }),
            now,
        )
        .await?;

        tx.commit().await?;
        Ok(claim)
    }

    /// Release a claim. `forced = true` models the human force-release path
    /// for stale agent claims.
    pub async fn release_claim(&self, task_id: &str, actor_id: &str, forced: bool) -> Result<()> {
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.pool.begin().await?;

        let claim_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM claims WHERE task_id = ? AND actor_id = ? AND released_at IS NULL",
        )
        .bind(task_id)
        .bind(actor_id)
        .fetch_optional(&mut *tx)
        .await?;

        let reason = if forced {
            ReleaseReason::Forced
        } else {
            ReleaseReason::Released
        };
        let res = sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = ?
             WHERE id = ? AND released_at IS NULL",
        )
        .bind(Utc::now())
        .bind(reason)
        .bind(claim_id.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await?;

        if res.rows_affected() == 0 {
            return Err(Error::ClaimConflict {
                task: task_id.to_string(),
                actor: actor_id.to_string(),
            });
        }

        // If the task was still in `work` because of this claim, return it to
        // the queue so another actor can pick it up.
        sqlx::query(
            "UPDATE tasks SET status = 'queue', updated_at = ?
             WHERE id = ? AND status = 'work'",
        )
        .bind(Utc::now())
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            if forced {
                "claim.force_released"
            } else {
                "claim.released"
            },
            json!({ "claim_id": claim_id }),
            Utc::now(),
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Renew a claim's TTL (heartbeat from the claim holder).
    pub async fn renew_claim(&self, task_id: &str, actor_id: &str, ttl: Duration) -> Result<()> {
        let expires_at =
            Utc::now() + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
        let res = sqlx::query(
            "UPDATE claims SET expires_at = ?
             WHERE task_id = ? AND actor_id = ? AND released_at IS NULL",
        )
        .bind(expires_at)
        .bind(task_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(Error::ClaimExpired(task_id.to_string()));
        }
        Ok(())
    }

    // -------------------------------------------------------------- workflow

    /// Submit for review: work → review. Releases the claim and requires
    /// that the submitter actually holds it (agents can't move others' work).
    pub async fn submit_for_review(&self, task_id: &str, actor_id: &str) -> Result<Task> {
        let signing_key = self.signing_key(actor_id).await?;
        let at = Utc::now();
        let mut tx = self.pool.begin().await?;
        let claim_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM claims WHERE task_id = ? AND actor_id = ? AND released_at IS NULL",
        )
        .bind(task_id)
        .bind(actor_id)
        .fetch_optional(&mut *tx)
        .await?;
        self.transition_tx(
            &mut tx,
            &signing_key,
            task_id,
            actor_id,
            TaskStatus::Work,
            TaskStatus::Review,
            "task.submitted",
            json!({ "claim_id": claim_id }),
            at,
        )
        .await?;
        // Release the submitter's claim: work on this task is done.
        sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = 'done'
             WHERE task_id = ? AND actor_id = ? AND released_at IS NULL",
        )
        .bind(at)
        .bind(task_id)
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    /// Approve: review → done. Deliberately has no claim requirement — any
    /// actor (in practice: a human reviewer) can approve, which is what
    /// enforces "agents can't self-approve" at the gate level.
    pub async fn approve(&self, task_id: &str, actor_id: &str) -> Result<Task> {
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.pool.begin().await?;
        self.transition_tx(
            &mut tx,
            &signing_key,
            task_id,
            actor_id,
            TaskStatus::Review,
            TaskStatus::Done,
            "task.approved",
            json!({}),
            Utc::now(),
        )
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    /// Send back for rework: review → queue.
    pub async fn request_changes(&self, task_id: &str, actor_id: &str) -> Result<Task> {
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.pool.begin().await?;
        self.transition_tx(
            &mut tx,
            &signing_key,
            task_id,
            actor_id,
            TaskStatus::Review,
            TaskStatus::Queue,
            "task.changes_requested",
            json!({}),
            Utc::now(),
        )
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    #[allow(clippy::too_many_arguments)] // internal helper; params are one logical transition
    async fn transition_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        signing_key: &SigningKey,
        task_id: &str,
        actor_id: &str,
        from: TaskStatus,
        to: TaskStatus,
        event_kind: &str,
        payload: serde_json::Value,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let res =
            sqlx::query("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ? AND status = ?")
                .bind(to)
                .bind(at)
                .bind(task_id)
                .bind(from)
                .execute(&mut **tx)
                .await?;
        if res.rows_affected() == 0 {
            let current: TaskStatus = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
                .bind(task_id)
                .fetch_one(&mut **tx)
                .await?;
            return Err(Error::InvalidTransition {
                task: task_id.to_string(),
                from: current.to_string(),
                via: event_kind.to_string(),
                to: to.to_string(),
            });
        }
        self.append_event_tx(tx, signing_key, actor_id, task_id, event_kind, payload, at)
            .await?;
        Ok(())
    }

    // ----------------------------------------------------------------- board

    /// Aggregate board state for the read-only Web/TUI views:
    /// tasks grouped by workflow status.
    pub async fn board_state(&self) -> Result<Vec<(TaskStatus, Vec<Task>)>> {
        let all = self.list_tasks(None).await?;
        let mut board: Vec<(TaskStatus, Vec<Task>)> = vec![
            (TaskStatus::Queue, vec![]),
            (TaskStatus::Work, vec![]),
            (TaskStatus::Review, vec![]),
            (TaskStatus::Done, vec![]),
        ];
        for task in all {
            if let Some(slot) = board.iter_mut().find(|(s, _)| *s == task.status) {
                slot.1.push(task);
            }
        }
        Ok(board)
    }

    // ---------------------------------------------------------------- events

    pub async fn list_events(&self, task_id: &str) -> Result<Vec<Event>> {
        sqlx::query_as::<_, Event>(
            "SELECT * FROM events WHERE task_id = ? ORDER BY lamport, event_id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Append a signed envelope on the current heads, inside the caller's
    /// transaction, so state and log move together (README invariant).
    /// `at` is the caller's mutation timestamp: state rows and replicas that
    /// derive state from this event must see the exact same value.
    #[allow(clippy::too_many_arguments)]
    async fn append_event_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        signing_key: &SigningKey,
        actor_id: &str,
        task_id: &str,
        kind: &str,
        payload: serde_json::Value,
        at: DateTime<Utc>,
    ) -> Result<EventId> {
        let parents = heads_tx(tx).await?;
        let lamport = next_lamport_tx(tx).await?;
        let ev = StoredEvent::sign(
            signing_key,
            Signable {
                lamport,
                parents: parents.clone(),
                actor_id: actor_id.to_string(),
                task_id: task_id.to_string(),
                kind: kind.to_string(),
                payload: payload.to_string(),
                at,
            },
        );
        let id = ev.id();
        sqlx::query(
            "INSERT INTO events
                 (task_id, actor_id, kind, payload, at, event_id, lamport, parents, signature, pubkey)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(actor_id)
        .bind(kind)
        .bind(&ev.body.payload)
        .bind(ev.body.at)
        .bind(id.to_vec())
        .bind(lamport as i64)
        .bind(parents_json(&parents))
        .bind(ev.sig.to_vec())
        .bind(ev.pubkey.to_vec())
        .execute(&mut **tx)
        .await?;
        insert_heads_tx(tx, &id, &parents).await?;
        Ok(id)
    }

    // ------------------------------------------------------------------ sync

    /// Current DAG heads.
    pub async fn heads(&self) -> Result<Vec<EventId>> {
        let rows: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT event_id FROM heads ORDER BY event_id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(id,)| to_id(&id)).collect())
    }

    /// Events in the synced DAG (v1 rows excluded).
    pub async fn dag_event_count(&self) -> Result<u64> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE event_id IS NOT NULL")
            .fetch_one(&self.pool)
            .await?;
        Ok(n as u64)
    }

    /// Events parked waiting for parents.
    pub async fn pending_count(&self) -> Result<u64> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pending_events")
            .fetch_one(&self.pool)
            .await?;
        Ok(n as u64)
    }

    /// Per-bucket BLAKE3 fingerprints of the DAG event set (sync step 1).
    pub async fn buckets(&self) -> Result<Vec<Bucket>> {
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT lamport, event_id FROM events
             WHERE event_id IS NOT NULL ORDER BY lamport / ?1, event_id",
        )
        .bind(BUCKET as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut out: Vec<Bucket> = Vec::new();
        let mut hasher = blake3::Hasher::new();
        for (lamport, id) in rows {
            let index = (lamport as u64) / BUCKET;
            if out.last().is_none_or(|b| b.index != index) {
                if let Some(last) = out.last_mut() {
                    last.digest = *hasher.finalize().as_bytes();
                }
                hasher = blake3::Hasher::new();
                out.push(Bucket {
                    index,
                    count: 0,
                    digest: [0; 32],
                });
            }
            hasher.update(&id);
            out.last_mut().expect("just pushed").count += 1;
        }
        if let Some(last) = out.last_mut() {
            last.digest = *hasher.finalize().as_bytes();
        }
        Ok(out)
    }

    /// Event ids in the given buckets (sync step 2).
    pub async fn ids_in_buckets(&self, indices: &[u64]) -> Result<Vec<EventId>> {
        if indices.is_empty() {
            return Ok(vec![]);
        }
        let marks: Vec<String> = vec!["?".into(); indices.len()];
        let sql = format!(
            "SELECT event_id FROM events
             WHERE event_id IS NOT NULL AND lamport / ?1 IN ({}) ORDER BY event_id",
            marks.join(",")
        );
        let mut q = sqlx::query_as::<_, (Vec<u8>,)>(&sql).bind(BUCKET as i64);
        for i in indices {
            q = q.bind(*i as i64);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(id,)| to_id(&id)).collect())
    }

    /// The signed DAG + actor directory, for `rstask sync export`.
    pub async fn export_bundle(&self) -> Result<SyncBundle> {
        // Alias keeps clippy's type_complexity quiet.
        type Row = (
            i64,
            String,
            String,
            String,
            String,
            String,
            DateTime<Utc>,
            Vec<u8>,
            Vec<u8>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT lamport, parents, actor_id, task_id, kind, payload, at, signature, pubkey
                 FROM events
                 WHERE event_id IS NOT NULL AND signature IS NOT NULL
                 ORDER BY lamport, event_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let events = rows
            .into_iter()
            .map(
                |(lamport, parents, actor_id, task_id, kind, payload, at, sig, pubkey)| {
                    StoredEvent {
                        body: Signable {
                            lamport: lamport as u64,
                            parents: parse_parents(&parents),
                            actor_id,
                            task_id,
                            kind,
                            payload,
                            at,
                        },
                        pubkey: to_id(&pubkey),
                        sig: sig.as_slice().try_into().expect("sig is 64 bytes"),
                    }
                    .encode()
                },
            )
            .collect();

        type ActorRowCols = (String, ActorKind, String, DateTime<Utc>, Vec<u8>);
        let arows: Vec<ActorRowCols> = sqlx::query_as(
            "SELECT a.id, a.kind, a.name, a.created_at, k.pubkey
                 FROM actors a JOIN actor_keys k ON k.actor_id = a.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let actors = arows
            .into_iter()
            .map(|(id, kind, name, created_at, pubkey)| ActorRow {
                id,
                kind,
                name,
                created_at,
                pubkey: to_id(&pubkey),
            })
            .collect();

        Ok(SyncBundle { events, actors })
    }

    /// Apply a peer bundle (PRD-v2 sync). Unknown actors are learned
    /// (trust-on-first-use within the trusted workspace); events are verified,
    /// parked on missing parents, drained, and applied for their state effect.
    pub async fn receive_bundle(&self, bundle: &SyncBundle) -> Result<Merge> {
        let mut merge = Merge::default();
        let mut tx = self.pool.begin().await?;
        for row in &bundle.actors {
            sqlx::query(
                "INSERT OR IGNORE INTO actors (id, kind, name, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(&row.id)
            .bind(row.kind)
            .bind(&row.name)
            .bind(row.created_at)
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT OR IGNORE INTO actor_keys (actor_id, pubkey) VALUES (?, ?)")
                .bind(&row.id)
                .bind(row.pubkey.to_vec())
                .execute(&mut *tx)
                .await?;
        }
        for bytes in &bundle.events {
            match StoredEvent::decode(bytes) {
                Ok(ev) => match insert_remote_tx(&mut tx, &ev).await? {
                    Insert::Added => merge.added += 1,
                    Insert::Parked => merge.pending += 1,
                    Insert::Duplicate => merge.duplicates += 1,
                    Insert::Rejected(reason) => {
                        merge.rejected += 1;
                        let id = ev.id();
                        merge
                            .rejected_ids
                            .push(format!("{}: {reason}", hex::encode(id)));
                    }
                },
                Err(e) => {
                    merge.rejected += 1;
                    merge.rejected_ids.push(e.to_string());
                }
            }
        }
        tx.commit().await?;
        Ok(merge)
    }

    async fn require_actor_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        actor_id: &str,
    ) -> Result<()> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM actors WHERE id = ?)")
            .bind(actor_id)
            .fetch_one(&mut **tx)
            .await?;
        if !exists {
            return Err(Error::ActorNotFound(actor_id.to_string()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// DAG helpers (tx-level, free functions)
// ---------------------------------------------------------------------------------------

enum Insert {
    Added,
    Parked,
    Duplicate,
    Rejected(String),
}

fn to_id(bytes: &[u8]) -> EventId {
    bytes.try_into().expect("event ids are 32 bytes")
}

fn parents_json(parents: &[EventId]) -> String {
    serde_json::to_string(&parents.iter().map(hex::encode).collect::<Vec<_>>())
        .expect("json encode")
}

fn parse_parents(json: &str) -> Vec<EventId> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .iter()
        .map(|h| hex::decode(h).map(|b| to_id(&b)).unwrap_or([0; 32]))
        .collect()
}

async fn heads_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<Vec<EventId>> {
    let rows: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT event_id FROM heads ORDER BY event_id")
        .fetch_all(&mut **tx)
        .await?;
    Ok(rows.iter().map(|(id,)| to_id(id)).collect())
}

/// Lamport value for a new event built on the current heads.
async fn next_lamport_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<u64> {
    let max: Option<i64> = sqlx::query_scalar(
        "SELECT max(e.lamport) FROM heads h JOIN events e ON e.event_id = h.event_id",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(max.map_or(0, |m| m as u64 + 1))
}

/// Replace `parents` with `id` in the heads set.
async fn insert_heads_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &EventId,
    parents: &[EventId],
) -> Result<()> {
    if !parents.is_empty() {
        let marks: Vec<String> = vec!["?".into(); parents.len()];
        let sql = format!("DELETE FROM heads WHERE event_id IN ({})", marks.join(","));
        let mut q = sqlx::query(&sql);
        for p in parents {
            q = q.bind(p.to_vec());
        }
        q.execute(&mut **tx).await?;
    }
    sqlx::query("INSERT OR IGNORE INTO heads (event_id) VALUES (?)")
        .bind(id.to_vec())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Verify and insert one remote event: park on missing parents, else apply.
async fn insert_remote_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
) -> Result<Insert> {
    let id = match ev.verify() {
        Ok(id) => id,
        Err(e) => return Ok(Insert::Rejected(e.to_string())),
    };
    let known: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_id = ?1)
            OR EXISTS(SELECT 1 FROM pending_events WHERE event_id = ?1)",
    )
    .bind(id.to_vec())
    .fetch_one(&mut **tx)
    .await?;
    if known {
        return Ok(Insert::Duplicate);
    }
    let pubkey: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT pubkey FROM actor_keys WHERE actor_id = ?")
            .bind(&ev.body.actor_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    match pubkey {
        None => return Ok(Insert::Rejected("unknown actor".into())),
        Some(pk) if to_id(&pk) != ev.pubkey => {
            return Ok(Insert::Rejected("actor pubkey mismatch".into()));
        }
        _ => {}
    }
    if parents_known_tx(tx, ev.parent_ids()).await? {
        insert_dag_tx(tx, ev, &id).await?;
        apply_effect_tx(tx, ev).await?;
        drain_pending_tx(tx).await?;
        Ok(Insert::Added)
    } else {
        sqlx::query(
            "INSERT OR IGNORE INTO pending_events (event_id, body, parents, received_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(id.to_vec())
        .bind(ev.encode())
        .bind(parents_json(ev.parent_ids()))
        .bind(Utc::now())
        .execute(&mut **tx)
        .await?;
        Ok(Insert::Parked)
    }
}

async fn parents_known_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    parents: &[EventId],
) -> Result<bool> {
    if parents.is_empty() {
        return Ok(true);
    }
    let marks: Vec<String> = vec!["?".into(); parents.len()];
    let sql = format!(
        "SELECT count(*) FROM events WHERE event_id IN ({})",
        marks.join(",")
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for p in parents {
        q = q.bind(p.to_vec());
    }
    Ok(q.fetch_one(&mut **tx).await? == parents.len() as i64)
}

/// Insert an already-verified event row and move the heads.
async fn insert_dag_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
    id: &EventId,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO events
             (task_id, actor_id, kind, payload, at, event_id, lamport, parents, signature, pubkey)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&ev.body.task_id)
    .bind(&ev.body.actor_id)
    .bind(&ev.body.kind)
    .bind(&ev.body.payload)
    .bind(ev.body.at)
    .bind(id.to_vec())
    .bind(ev.body.lamport as i64)
    .bind(parents_json(ev.parent_ids()))
    .bind(ev.sig.to_vec())
    .bind(ev.pubkey.to_vec())
    .execute(&mut **tx)
    .await?;
    insert_heads_tx(tx, id, ev.parent_ids()).await?;
    Ok(())
}

/// Parked events whose parents have arrived, inserted and applied (repeating,
/// since one unblock can cascade).
async fn drain_pending_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    loop {
        let rows: Vec<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT event_id, body FROM pending_events ORDER BY received_at")
                .fetch_all(&mut **tx)
                .await?;
        let mut progress = false;
        for (eid, body) in rows {
            let ev = StoredEvent::decode(&body)?;
            if parents_known_tx(tx, ev.parent_ids()).await? {
                insert_dag_tx(tx, &ev, &to_id(&eid)).await?;
                apply_effect_tx(tx, &ev).await?;
                sqlx::query("DELETE FROM pending_events WHERE event_id = ?")
                    .bind(&eid)
                    .execute(&mut **tx)
                    .await?;
                progress = true;
            }
        }
        if !progress {
            return Ok(());
        }
    }
}

/// Deterministic state effect of one applied event (PRD-v2 sync).
async fn apply_effect_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
) -> Result<()> {
    let payload: serde_json::Value =
        serde_json::from_str(&ev.body.payload).unwrap_or(serde_json::json!({}));
    match ev.body.kind.as_str() {
        "task.created" => {
            sqlx::query(
                "INSERT OR IGNORE INTO tasks
                     (id, title, body_md, status, created_by, created_at, updated_at)
                 VALUES (?, ?, ?, 'queue', ?, ?, ?)",
            )
            .bind(&ev.body.task_id)
            .bind(payload["title"].as_str().unwrap_or(""))
            .bind(payload["body_md"].as_str().unwrap_or(""))
            .bind(&ev.body.actor_id)
            .bind(ev.body.at)
            .bind(ev.body.at)
            .execute(&mut **tx)
            .await?;
        }
        "task.updated" => {
            let get_str = |k: &str| payload.get(k).and_then(|v| v.as_str());
            let title = get_str("title");
            let body_md = get_str("body_md");
            let priority = payload.get("priority").and_then(|v| v.as_i64());
            sqlx::query(
                "UPDATE tasks SET
                     title = COALESCE(?, title), body_md = COALESCE(?, body_md),
                     priority = COALESCE(?, priority), updated_at = ?
                 WHERE id = ?",
            )
            .bind(title)
            .bind(body_md)
            .bind(priority)
            .bind(ev.body.at)
            .bind(&ev.body.task_id)
            .execute(&mut **tx)
            .await?;
        }
        // Claims fold per task below (D8); approved/rejected are status-guarded.
        "task.claimed" | "claim.released" | "claim.force_released" | "task.submitted" => {
            refold_task_tx(tx, &ev.body.task_id).await?;
        }
        "task.approved" => {
            sqlx::query(
                "UPDATE tasks SET status = 'done', updated_at = ? WHERE id = ? AND status = 'review'",
            )
            .bind(ev.body.at)
            .bind(&ev.body.task_id)
            .execute(&mut **tx)
            .await?;
        }
        "task.changes_requested" => {
            sqlx::query(
                "UPDATE tasks SET status = 'queue', updated_at = ? WHERE id = ? AND status = 'review'",
            )
            .bind(ev.body.at)
            .bind(&ev.body.task_id)
            .execute(&mut **tx)
            .await?;
        }
        _ => {} // Unknown kinds are logged but have no effect (forward compat).
    }
    Ok(())
}

/// Re-fold a task's claim events in `(lamport, event_id)` order (PRD-v2 D8):
/// the first claim wins the lock; later concurrent claims stay in the log but
/// are ineffective. Recomputed wholesale on every claim-affecting merge so the
/// result never depends on arrival order.
async fn refold_task_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, task_id: &str) -> Result<()> {
    let rows: Vec<(String, String, DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT kind, actor_id, at, payload FROM events
         WHERE task_id = ?1 AND event_id IS NOT NULL
           AND kind IN ('task.claimed','claim.released','claim.force_released','task.submitted')
         ORDER BY lamport, event_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;

    let now = Utc::now();
    // (claim_id, actor_id, claimed_at, expires_at)
    let mut active: Option<(String, String, DateTime<Utc>, DateTime<Utc>)> = None;
    let mut folded_out: Vec<(String, ReleaseReason)> = Vec::new();
    let mut submitted = false;

    for (kind, actor_id, at, payload) in rows {
        let payload: serde_json::Value =
            serde_json::from_str(&payload).unwrap_or(serde_json::json!({}));
        let claim_id = payload["claim_id"].as_str().unwrap_or("").to_string();
        match kind.as_str() {
            "task.claimed" => {
                if active.is_none() {
                    let exp: Option<DateTime<Utc>> =
                        serde_json::from_value(payload["expires_at"].clone()).ok();
                    if let Some(exp) = exp.filter(|e| *e > now) {
                        active = Some((claim_id, actor_id, at, exp));
                    }
                    // An already-expired claim never activates (lazy-sweep parity).
                }
                // An active claim already exists: this one is ineffective (D8).
            }
            "claim.released" | "claim.force_released" | "task.submitted" => {
                if let Some((cid, _, _, _)) = &active {
                    if *cid == claim_id {
                        let reason = match kind.as_str() {
                            "claim.force_released" => ReleaseReason::Forced,
                            "task.submitted" => ReleaseReason::Done,
                            _ => ReleaseReason::Released,
                        };
                        folded_out.push((cid.clone(), reason));
                        submitted |= kind == "task.submitted";
                        active = None;
                    }
                }
            }
            _ => {}
        }
    }

    // Natural expiry, then fold every non-winning claim out of the active set.
    sqlx::query(
        "UPDATE claims SET released_at = ?, release_reason = 'expired'
         WHERE task_id = ? AND released_at IS NULL AND expires_at <= ?",
    )
    .bind(now)
    .bind(task_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    for (cid, reason) in folded_out {
        sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = ? WHERE id = ? AND released_at IS NULL",
        )
        .bind(now)
        .bind(reason)
        .bind(cid)
        .execute(&mut **tx)
        .await?;
    }
    match active {
        Some((cid, actor_id, claimed_at, expires_at)) => {
            // The fold winner owns the lock: materialize its claim row on this
            // replica (it may have been decided by an event received here).
            sqlx::query(
                "INSERT OR IGNORE INTO claims (id, task_id, actor_id, claimed_at, expires_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&cid)
            .bind(task_id)
            .bind(actor_id)
            .bind(claimed_at)
            .bind(expires_at)
            .execute(&mut **tx)
            .await?;
            // Any other still-active claim is a fold loser (D8).
            sqlx::query(
                "UPDATE claims SET released_at = ?, release_reason = 'released'
                 WHERE task_id = ? AND released_at IS NULL AND id != ?",
            )
            .bind(now)
            .bind(task_id)
            .bind(cid)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE tasks SET status = 'work', updated_at = ? WHERE id = ? AND status = 'queue'",
            )
            .bind(now)
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
        }
        None => {
            sqlx::query(
                "UPDATE tasks SET status = 'queue', updated_at = ? WHERE id = ? AND status = 'work'",
            )
            .bind(now)
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    if submitted {
        sqlx::query(
            "UPDATE tasks SET status = 'review', updated_at = ?
             WHERE id = ? AND status IN ('work', 'queue')",
        )
        .bind(now)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

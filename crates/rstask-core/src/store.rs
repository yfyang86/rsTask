//! Storage layer: the single writer behind every interface (CLI/TUI/Web/MCP).
//!
//! Invariants enforced here, regardless of caller:
//! - A task has at most one active claim (atomic, transactional).
//! - Stale claims (no heartbeat past TTL) are swept lazily on each claim.
//! - Status moves only along the workflow gate: queue → work → review → done.
//! - Every mutation appends to the immutable event log in the same transaction.

use std::collections::HashMap;
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
use crate::keys::Keyring;
use crate::models::{
    Actor, ActorKind, Claim, Event, Peer, Rejected, ReleaseReason, Role, Roles, Submission,
    Submitted, Task, TaskDetail, TaskStatus,
};
use crate::sync::{Bucket, Merge, SyncBundle, BUCKET};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_event_dag.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_links_roles.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_protected.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_peers.sql");
const SCHEMA_VERSION: i64 = 5;

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
    if version < 3 {
        sqlx::raw_sql(MIGRATION_0003).execute(pool).await?;
    }
    if version < 4 {
        sqlx::raw_sql(MIGRATION_0004).execute(pool).await?;
    }
    if version < 5 {
        sqlx::raw_sql(MIGRATION_0005).execute(pool).await?;
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
    /// Actor secret keys: outside the database (PRD-v3 R1).
    keys: Keyring,
    /// Keeps the shared in-memory database alive for `open_in_memory`.
    #[allow(dead_code)]
    keepalive: Option<SqliteConnection>,
}

impl Store {
    /// Open (creating if needed) the SQLite database and run migrations.
    /// Secret keys are read from `keys/` beside the database.
    pub async fn open(path: &Path) -> Result<Self> {
        let key_dir = path.parent().unwrap_or(Path::new("")).join("keys");
        Self::open_with_keys(path, &key_dir).await
    }

    /// Open the database with secret keys in `key_dir` (PRD-v3 R1).
    pub async fn open_with_keys(path: &Path, key_dir: &Path) -> Result<Self> {
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
        let keys = Keyring::Dir(key_dir.to_path_buf());
        // Workspaces from before sprint 07 kept secrets in the database:
        // move them out.
        let legacy: Vec<(String, Vec<u8>)> =
            sqlx::query_as("SELECT actor_id, secret FROM actor_keys WHERE secret IS NOT NULL")
                .fetch_all(&pool)
                .await?;
        let scrub = !legacy.is_empty();
        for (actor_id, secret) in legacy {
            if let Ok(secret) = <[u8; 32]>::try_from(secret.as_slice()) {
                keys.put(&actor_id, &secret)?;
            }
            sqlx::query("UPDATE actor_keys SET secret = NULL WHERE actor_id = ?")
                .bind(&actor_id)
                .execute(&pool)
                .await?;
        }
        if scrub {
            // NULLing a column leaves the old bytes in freed pages and in the
            // write-ahead log: rewrite the file and empty the log.
            sqlx::raw_sql("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
                .execute(&pool)
                .await?;
        }
        Ok(Self {
            pool,
            keys,
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
            keys: Keyring::Memory(Default::default()),
            keepalive: Some(keepalive),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Every mutation runs in a `BEGIN IMMEDIATE` transaction: it takes the
    /// write lock up front and waits its turn. A deferred transaction that
    /// reads first is refused outright (`database is locked`) if another
    /// writer commits before its first write.
    async fn begin_write(&self) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }

    // ---------------------------------------------------------------- actors

    /// An actor holding every role, provisioned by the workspace root.
    pub async fn create_actor(&self, kind: ActorKind, name: &str) -> Result<Actor> {
        self.provision_actor(kind, name, &Role::ALL, None).await
    }

    /// An actor limited to `roles` (PRD-v3 M5); none given means all.
    pub async fn create_actor_with_roles(
        &self,
        kind: ActorKind,
        name: &str,
        roles: &[Role],
    ) -> Result<Actor> {
        self.provision_actor(kind, name, roles, None).await
    }

    /// Create an actor as a signed `actor.created` event (PRD-v3 R3). The
    /// first actor of a workspace is its root: human and self-signed. Every
    /// later actor is signed by a human — `by`, or the root when `None` —
    /// whose key must be in this key directory. The new actor's key is
    /// written there too.
    pub async fn provision_actor(
        &self,
        kind: ActorKind,
        name: &str,
        roles: &[Role],
        by: Option<&str>,
    ) -> Result<Actor> {
        let roles = Roles(if roles.is_empty() {
            Role::ALL.to_vec()
        } else {
            Role::ALL
                .into_iter()
                .filter(|r| roles.contains(r))
                .collect()
        });
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let new_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let pubkey = new_key.verifying_key().to_bytes();

        let mut tx = self.begin_write().await?;
        let root: Option<String> = match root_actor_tx(&mut tx).await? {
            Some(root) => Some(root),
            // Workspaces from before sprint 07 have no root event: their
            // oldest human stands in.
            None => {
                sqlx::query_scalar(
                    "SELECT id FROM actors WHERE kind = 'human' ORDER BY created_at LIMIT 1",
                )
                .fetch_optional(&mut *tx)
                .await?
            }
        };
        let (signer, signer_key) = match by.map(str::to_string).or(root) {
            None => {
                if kind != ActorKind::Human {
                    return Err(Error::NotPermitted {
                        actor: name.to_string(),
                        action: "be the first actor: the workspace root must be human".into(),
                    });
                }
                (id.clone(), new_key.clone())
            }
            Some(signer) => {
                let signer_kind: Option<ActorKind> =
                    sqlx::query_scalar("SELECT kind FROM actors WHERE id = ?")
                        .bind(&signer)
                        .fetch_optional(&mut *tx)
                        .await?;
                match signer_kind {
                    None => return Err(Error::ActorNotFound(signer)),
                    Some(ActorKind::Human) => {}
                    Some(ActorKind::Agent) => {
                        return Err(Error::NotPermitted {
                            actor: signer,
                            action: "provision actors (humans only)".into(),
                        });
                    }
                }
                let key = self.keys.get(&signer)?;
                (signer, key)
            }
        };

        insert_actor_tx(&mut tx, &id, kind, name, &roles, &pubkey, now).await?;
        self.append_event_tx(
            &mut tx,
            &signer_key,
            &signer,
            &id,
            "actor.created",
            json!({
                "kind": kind,
                "name": name,
                "roles": roles,
                "pubkey": hex::encode(pubkey),
            }),
            now,
        )
        .await?;
        self.keys.put(&id, &new_key.to_bytes())?;
        tx.commit().await?;
        Ok(Actor {
            id,
            kind,
            name: name.to_string(),
            created_at: now,
            last_heartbeat_at: None,
            roles,
        })
    }

    /// The actor's secret key, to carry it to another key directory.
    pub fn export_key(&self, actor_id: &str) -> Result<[u8; 32]> {
        Ok(self.keys.get(actor_id)?.to_bytes())
    }

    /// Add an actor's secret key to this key directory; it must match the
    /// public key the workspace knows the actor by.
    pub async fn import_key(&self, actor_id: &str, secret: &[u8; 32]) -> Result<()> {
        let known: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT pubkey FROM actor_keys WHERE actor_id = ?")
                .bind(actor_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(known) = known else {
            return Err(Error::ActorNotFound(actor_id.to_string()));
        };
        if SigningKey::from_bytes(secret).verifying_key().to_bytes()[..] != known[..] {
            return Err(Error::KeyMismatch(actor_id.to_string()));
        }
        self.keys.put(actor_id, secret)
    }

    /// `Ok` if the actor holds `role`; the error names the role it lacks.
    async fn require_role(&self, actor_id: &str, role: Role, action: &str) -> Result<()> {
        if self.get_actor(actor_id).await?.roles.0.contains(&role) {
            return Ok(());
        }
        Err(Error::NotPermitted {
            actor: actor_id.to_string(),
            action: format!("{action} (needs role {role})"),
        })
    }

    /// The actor's Ed25519 signing key, if this key directory holds it.
    async fn signing_key(&self, actor_id: &str) -> Result<SigningKey> {
        self.keys.get(actor_id)
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
        self.create_task_under(None, title, body_md, created_by)
            .await
    }

    /// Create a subtask: the parent is not claimable until its subtasks are
    /// done (PRD-v3 M4).
    pub async fn create_subtask(
        &self,
        parent_id: &str,
        title: &str,
        body_md: &str,
        created_by: &str,
    ) -> Result<Task> {
        self.create_task_under(Some(parent_id), title, body_md, created_by)
            .await
    }

    async fn create_task_under(
        &self,
        parent_id: Option<&str>,
        title: &str,
        body_md: &str,
        created_by: &str,
    ) -> Result<Task> {
        self.require_role(created_by, Role::Planner, "create tasks")
            .await?;
        let signing_key = self.signing_key(created_by).await?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let mut tx = self.begin_write().await?;
        if let Some(parent_id) = parent_id {
            require_task_tx(&mut tx, parent_id).await?;
        }
        sqlx::query(
            "INSERT INTO tasks
                 (id, title, body_md, status, created_by, created_at, updated_at, parent_id)
             VALUES (?, ?, ?, 'queue', ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(title)
        .bind(body_md)
        .bind(created_by)
        .bind(now)
        .bind(now)
        .bind(parent_id)
        .execute(&mut *tx)
        .await?;

        let mut payload = json!({ "title": title, "body_md": body_md });
        if let Some(parent_id) = parent_id {
            payload["parent"] = json!(parent_id);
        }
        self.append_event_tx(
            &mut tx,
            &signing_key,
            created_by,
            &id,
            "task.created",
            payload,
            now,
        )
        .await?;

        tx.commit().await?;
        self.get_task(&id).await
    }

    /// Declare that `task_id` waits until `blocked_by` is done (PRD-v3 M4).
    pub async fn add_dependency(
        &self,
        task_id: &str,
        blocked_by: &str,
        actor_id: &str,
    ) -> Result<()> {
        self.require_role(actor_id, Role::Planner, "edit dependencies")
            .await?;
        let signing_key = self.signing_key(actor_id).await?;
        let at = Utc::now();
        let mut tx = self.begin_write().await?;
        require_task_tx(&mut tx, task_id).await?;
        require_task_tx(&mut tx, blocked_by).await?;
        // Would `blocked_by` end up waiting — through blockers or subtasks —
        // on `task_id` itself?
        let cycle: bool = sqlx::query_scalar(
            "WITH RECURSIVE waits(id) AS (
                 SELECT ?1
                 UNION SELECT d.blocked_by FROM task_deps d JOIN waits ON d.task_id = waits.id
                 UNION SELECT c.id FROM tasks c JOIN waits ON c.parent_id = waits.id)
             SELECT EXISTS(SELECT 1 FROM waits WHERE id = ?2)",
        )
        .bind(blocked_by)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        if cycle {
            return Err(Error::DependencyCycle {
                task: task_id.to_string(),
                by: blocked_by.to_string(),
            });
        }
        sqlx::query("INSERT OR IGNORE INTO task_deps (task_id, blocked_by) VALUES (?, ?)")
            .bind(task_id)
            .bind(blocked_by)
            .execute(&mut *tx)
            .await?;
        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            "task.blocked",
            json!({ "by": blocked_by }),
            at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Withdraw a dependency declared with `add_dependency`.
    pub async fn remove_dependency(
        &self,
        task_id: &str,
        blocked_by: &str,
        actor_id: &str,
    ) -> Result<()> {
        self.require_role(actor_id, Role::Planner, "edit dependencies")
            .await?;
        let signing_key = self.signing_key(actor_id).await?;
        let at = Utc::now();
        let mut tx = self.begin_write().await?;
        require_task_tx(&mut tx, task_id).await?;
        sqlx::query("DELETE FROM task_deps WHERE task_id = ? AND blocked_by = ?")
            .bind(task_id)
            .bind(blocked_by)
            .execute(&mut *tx)
            .await?;
        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            "task.unblocked",
            json!({ "by": blocked_by }),
            at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Return stale claims' tasks to the pool, so reads never show dead
    /// `work` (PRD-v3 M1).
    async fn sweep_expired(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sweep_expired_tx(&mut tx, Utc::now()).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_task(&self, id: &str) -> Result<Task> {
        self.sweep_expired().await?;
        sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::TaskNotFound(id.to_string()))
    }

    /// The task plus its live claim and the latest handoff each way.
    pub async fn task_detail(&self, id: &str) -> Result<TaskDetail> {
        let task = self.get_task(id).await?;
        let claim = sqlx::query_as::<_, Claim>(
            "SELECT * FROM claims WHERE task_id = ? AND released_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let submission = self
            .latest_event(id, "task.submitted")
            .await?
            .map(|(id, by, at, p)| Submitted {
                id,
                by,
                at,
                submission: serde_json::from_value(p).unwrap_or_default(),
            });
        let rejection =
            self.latest_event(id, "task.changes_requested")
                .await?
                .map(|(_, by, at, p)| Rejected {
                    by,
                    at,
                    reason: p["reason"].as_str().unwrap_or("").to_string(),
                });
        let blocked_by: Vec<String> = sqlx::query_scalar(
            "SELECT blocked_by FROM task_deps WHERE task_id = ? ORDER BY blocked_by",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        let children: Vec<String> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE parent_id = ? ORDER BY created_at, id")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        let mut conn = self.pool.acquire().await?;
        let waiting_on = waiting_on(&mut conn, id).await?;
        Ok(TaskDetail {
            task,
            claim,
            submission,
            rejection,
            blocked_by,
            children,
            waiting_on,
        })
    }

    /// Actor, time and payload of the task's latest event of `kind`.
    async fn latest_event(
        &self,
        task_id: &str,
        kind: &str,
    ) -> Result<Option<(String, String, DateTime<Utc>, serde_json::Value)>> {
        let row: Option<(Vec<u8>, String, DateTime<Utc>, String)> = sqlx::query_as(
            "SELECT event_id, actor_id, at, payload FROM events
             WHERE task_id = ? AND kind = ? AND event_id IS NOT NULL
             ORDER BY lamport DESC, event_id DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, by, at, payload)| {
            let payload = serde_json::from_str(&payload).unwrap_or(json!({}));
            (hex::encode(id), by, at, payload)
        }))
    }

    pub async fn list_tasks(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        self.sweep_expired().await?;
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

    /// Update title/body/priority. While the task is claimed only the holder
    /// may edit it; otherwise it takes a planner.
    pub async fn update_task(
        &self,
        id: &str,
        title: Option<&str>,
        body_md: Option<&str>,
        priority: Option<i64>,
        actor_id: &str,
    ) -> Result<Task> {
        let signing_key = self.signing_key(actor_id).await?;
        let planner = self
            .require_role(actor_id, Role::Planner, "edit an unclaimed task")
            .await;
        let now = Utc::now();
        let mut tx = self.begin_write().await?;
        sweep_expired_tx(&mut tx, now).await?;
        require_task_tx(&mut tx, id).await?;
        let holder: Option<String> = sqlx::query_scalar(
            "SELECT actor_id FROM claims WHERE task_id = ? AND released_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        match holder {
            Some(holder) if holder != actor_id => {
                return Err(Error::ClaimConflict {
                    task: id.to_string(),
                    actor: holder,
                });
            }
            Some(_) => {}
            None => planner?,
        }
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
        self.require_role(actor_id, Role::Worker, "claim tasks")
            .await?;
        let signing_key = self.signing_key(actor_id).await?;
        let now = Utc::now();
        let mut tx = self.begin_claim_tx(now).await?;
        let claim = self
            .claim_tx(&mut tx, &signing_key, task_id, actor_id, ttl, now)
            .await?;
        tx.commit().await?;
        Ok(claim)
    }

    /// Atomically claim the next eligible task — highest priority, then
    /// oldest, skipping tasks still waiting on a blocker or a subtask — so
    /// agents never list-then-race (PRD-v3 M3, M4). `None` when nothing is
    /// claimable.
    pub async fn claim_next(&self, actor_id: &str, ttl: Duration) -> Result<Option<Claim>> {
        self.require_role(actor_id, Role::Worker, "claim tasks")
            .await?;
        let signing_key = self.signing_key(actor_id).await?;
        let now = Utc::now();
        let mut tx = self.begin_claim_tx(now).await?;
        let next: Option<String> = sqlx::query_scalar(
            "SELECT t.id FROM tasks t WHERE t.status = 'queue'
               AND NOT EXISTS (SELECT 1 FROM task_deps d JOIN tasks b ON b.id = d.blocked_by
                               WHERE d.task_id = t.id AND b.status != 'done')
               AND NOT EXISTS (SELECT 1 FROM tasks c
                               WHERE c.parent_id = t.id AND c.status != 'done')
             ORDER BY t.priority DESC, t.created_at LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(task_id) = next else {
            return Ok(None);
        };
        let claim = self
            .claim_tx(&mut tx, &signing_key, &task_id, actor_id, ttl, now)
            .await?;
        tx.commit().await?;
        Ok(Some(claim))
    }

    /// Open a claim transaction, stale claims swept.
    async fn begin_claim_tx(
        &self,
        now: DateTime<Utc>,
    ) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>> {
        let mut tx = self.begin_write().await?;

        // Sweep expired claims (not renewed past TTL → back to pool).
        // Any `work` task left without an active claim returns to `queue`.
        sweep_expired_tx(&mut tx, now).await?;
        sqlx::query(
            "UPDATE tasks SET status = 'queue', updated_at = ?
             WHERE status = 'work'
               AND id NOT IN (SELECT task_id FROM claims WHERE released_at IS NULL)",
        )
        .bind(now)
        .execute(&mut *tx)
        .await?;
        Ok(tx)
    }

    async fn claim_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        signing_key: &SigningKey,
        task_id: &str,
        actor_id: &str,
        ttl: Duration,
        now: DateTime<Utc>,
    ) -> Result<Claim> {
        let task_row = sqlx::query("SELECT status FROM tasks WHERE id = ?")
            .bind(task_id)
            .fetch_optional(&mut **tx)
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
                .fetch_optional(&mut **tx)
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

        self.require_actor_tx(tx, actor_id).await?;
        let waiting = waiting_on(tx, task_id).await?;
        if !waiting.is_empty() {
            return Err(Error::Blocked {
                task: task_id.to_string(),
                by: waiting.join(", "),
            });
        }

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
        .execute(&mut **tx)
        .await?;

        sqlx::query("UPDATE tasks SET status = 'work', updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(task_id)
            .execute(&mut **tx)
            .await?;

        self.append_event_tx(
            tx,
            signing_key,
            actor_id,
            task_id,
            "task.claimed",
            json!({ "claim_id": claim.id, "expires_at": claim.expires_at }),
            now,
        )
        .await?;
        Ok(claim)
    }

    /// Release a claim. Plain release: the actor's own claim. `forced = true`
    /// is the human force-release path: a human actor releases whoever holds
    /// the task (PRD-v3 G3).
    pub async fn release_claim(&self, task_id: &str, actor_id: &str, forced: bool) -> Result<()> {
        if forced && self.get_actor(actor_id).await?.kind != ActorKind::Human {
            return Err(Error::NotPermitted {
                actor: actor_id.to_string(),
                action: "force-release a claim (humans only)".into(),
            });
        }
        let signing_key = self.signing_key(actor_id).await?;
        let at = Utc::now();
        let mut tx = self.begin_write().await?;
        sweep_expired_tx(&mut tx, at).await?;

        let held: Option<String> = if forced {
            sqlx::query_scalar("SELECT id FROM claims WHERE task_id = ? AND released_at IS NULL")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?
        } else {
            None
        };
        let (kind, reason) = if forced {
            ("claim.force_released", ReleaseReason::Forced)
        } else {
            ("claim.released", ReleaseReason::Released)
        };
        // Nothing to force, or a plain release: the caller's own live claim,
        // or the error that says who holds the task instead.
        let claim_id = match held {
            Some(id) => id,
            None => require_claim_tx(&mut tx, task_id, actor_id, kind, TaskStatus::Queue).await?,
        };
        sqlx::query("UPDATE claims SET released_at = ?, release_reason = ? WHERE id = ?")
            .bind(at)
            .bind(reason)
            .bind(&claim_id)
            .execute(&mut *tx)
            .await?;

        // If the task was still in `work` because of this claim, return it to
        // the queue so another actor can pick it up.
        sqlx::query(
            "UPDATE tasks SET status = 'queue', updated_at = ?
             WHERE id = ? AND status = 'work'",
        )
        .bind(at)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            kind,
            json!({ "claim_id": claim_id }),
            at,
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Renew the holder's claim: a signed `task.renewed` event moves the
    /// expiry on every replica (PRD-v3 M1). An expired claim cannot be renewed.
    pub async fn renew_claim(&self, task_id: &str, actor_id: &str, ttl: Duration) -> Result<Claim> {
        let signing_key = self.signing_key(actor_id).await?;
        let at = Utc::now();
        let expires_at = at + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
        let mut tx = self.begin_write().await?;
        sweep_expired_tx(&mut tx, at).await?;
        let claim_id =
            require_claim_tx(&mut tx, task_id, actor_id, "task.renewed", TaskStatus::Work).await?;
        sqlx::query("UPDATE claims SET expires_at = ? WHERE id = ?")
            .bind(expires_at)
            .bind(&claim_id)
            .execute(&mut *tx)
            .await?;
        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            "task.renewed",
            json!({ "claim_id": claim_id, "expires_at": expires_at }),
            at,
        )
        .await?;
        let claim = sqlx::query_as::<_, Claim>("SELECT * FROM claims WHERE id = ?")
            .bind(&claim_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(claim)
    }

    // -------------------------------------------------------------- workflow

    /// Submit for review: work → review. Only the holder of a live claim
    /// submits; the claim is released and the submission (PRD-v3 M2) rides on
    /// the event for the reviewer.
    pub async fn submit_for_review(
        &self,
        task_id: &str,
        actor_id: &str,
        submission: &Submission,
    ) -> Result<Task> {
        let signing_key = self.signing_key(actor_id).await?;
        let at = Utc::now();
        let mut tx = self.begin_write().await?;
        sweep_expired_tx(&mut tx, at).await?;
        let claim_id = require_claim_tx(
            &mut tx,
            task_id,
            actor_id,
            "task.submitted",
            TaskStatus::Review,
        )
        .await?;
        self.transition_tx(
            &mut tx,
            &signing_key,
            task_id,
            actor_id,
            TaskStatus::Work,
            TaskStatus::Review,
            "task.submitted",
            json!({
                "claim_id": claim_id,
                "summary": submission.summary,
                "refs": submission.refs,
                "evidence": submission.evidence,
            }),
            at,
        )
        .await?;
        // Release the submitter's claim: work on this task is done.
        sqlx::query("UPDATE claims SET released_at = ?, release_reason = 'done' WHERE id = ?")
            .bind(at)
            .bind(&claim_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    /// Approve: review → done. Takes a reviewer; an agent cannot approve its
    /// own submission (PRD-v3 G2) or a protected task (R5). `submission`, if
    /// given, is the id the reviewer read: a resubmission since then refuses.
    pub async fn approve(
        &self,
        task_id: &str,
        actor_id: &str,
        submission: Option<&str>,
    ) -> Result<Task> {
        self.require_role(actor_id, Role::Reviewer, "review tasks")
            .await?;
        let agent = self.get_actor(actor_id).await?.kind == ActorKind::Agent;
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.begin_write().await?;
        let latest: Option<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT event_id, actor_id FROM events
             WHERE task_id = ? AND kind = 'task.submitted' AND event_id IS NOT NULL
             ORDER BY lamport DESC, event_id DESC LIMIT 1",
        )
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (current, submitter) = latest.map(|(id, by)| (hex::encode(id), by)).unzip();
        if agent && submitter.as_deref() == Some(actor_id) {
            return Err(Error::NotPermitted {
                actor: actor_id.to_string(),
                action: "approve its own submission".into(),
            });
        }
        if let (Some(reviewed), Some(current)) = (submission, &current) {
            if reviewed != current {
                return Err(Error::StaleApproval {
                    task: task_id.to_string(),
                    reviewed: reviewed.to_string(),
                    current: current.clone(),
                });
            }
        }
        let protected: Option<bool> =
            sqlx::query_scalar("SELECT protected FROM tasks WHERE id = ?")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?;
        if agent && protected == Some(true) {
            return Err(Error::NotPermitted {
                actor: actor_id.to_string(),
                action: "approve a protected task (humans only)".into(),
            });
        }
        // The approval names the submission it approves (PRD-v3 R4): on any
        // replica it counts only while that submission is the one in review.
        self.transition_tx(
            &mut tx,
            &signing_key,
            task_id,
            actor_id,
            TaskStatus::Review,
            TaskStatus::Done,
            "task.approved",
            json!({ "submission": current }),
            Utc::now(),
        )
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    /// Mark a task protected: from now on only a human's approval counts
    /// (PRD-v3 R5). There is no way back.
    pub async fn protect_task(&self, task_id: &str, actor_id: &str) -> Result<Task> {
        self.require_role(actor_id, Role::Planner, "protect tasks")
            .await?;
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.begin_write().await?;
        require_task_tx(&mut tx, task_id).await?;
        sqlx::query("UPDATE tasks SET protected = 1 WHERE id = ?")
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        self.append_event_tx(
            &mut tx,
            &signing_key,
            actor_id,
            task_id,
            "task.protected",
            json!({}),
            Utc::now(),
        )
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    /// Send back for rework: review → queue.    /// Send back for rework: review → queue. The reason rides on the event
    /// for whoever picks the task up next (PRD-v3 M2).
    pub async fn request_changes(
        &self,
        task_id: &str,
        actor_id: &str,
        reason: &str,
    ) -> Result<Task> {
        self.require_role(actor_id, Role::Reviewer, "review tasks")
            .await?;
        let signing_key = self.signing_key(actor_id).await?;
        let mut tx = self.begin_write().await?;
        self.transition_tx(
            &mut tx,
            &signing_key,
            task_id,
            actor_id,
            TaskStatus::Review,
            TaskStatus::Queue,
            "task.changes_requested",
            json!({ "reason": reason }),
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
            let current: Option<TaskStatus> =
                sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
                    .bind(task_id)
                    .fetch_optional(&mut **tx)
                    .await?;
            let Some(current) = current else {
                return Err(Error::TaskNotFound(task_id.to_string()));
            };
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
        // Plain `?` placeholders throughout: sqlx numbers them from 1 again
        // after a `?1`, so mixing the two styles binds the wrong values.
        let marks = vec!["?"; indices.len()].join(",");
        let sql = format!(
            "SELECT event_id FROM events
             WHERE event_id IS NOT NULL AND lamport / ? IN ({marks}) ORDER BY event_id"
        );
        let mut q = sqlx::query_as::<_, (Vec<u8>,)>(&sql).bind(BUCKET as i64);
        for i in indices {
            q = q.bind(*i as i64);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(id,)| to_id(&id)).collect())
    }

    /// The signed DAG, for `rstask sync export`. Actors travel in it as
    /// `actor.created` events.
    pub async fn export_bundle(&self) -> Result<SyncBundle> {
        let rows: Vec<EventRow> = sqlx::query_as(&format!(
            "SELECT {EVENT_COLUMNS} FROM events
             WHERE event_id IS NOT NULL AND signature IS NOT NULL
             ORDER BY lamport, event_id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(SyncBundle {
            events: rows.into_iter().map(encode_row).collect(),
        })
    }

    /// The encoded events with these ids, parents first (sync step 3).
    /// Unknown ids are skipped.
    pub async fn events_by_id(&self, ids: &[EventId]) -> Result<Vec<Vec<u8>>> {
        let mut rows: Vec<EventRow> = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT {EVENT_COLUMNS} FROM events
                 WHERE signature IS NOT NULL AND event_id IN ({marks})"
            );
            let mut q = sqlx::query_as::<_, EventRow>(&sql);
            for id in chunk {
                q = q.bind(id.to_vec());
            }
            rows.extend(q.fetch_all(&self.pool).await?);
        }
        // Lamport order is causal order: a parent's clock is always lower.
        rows.sort_by_key(|row| row.0);
        Ok(rows.into_iter().map(encode_row).collect())
    }

    /// Merge encoded events from a peer: `receive_bundle` without the wrapper.
    pub async fn receive_events(&self, events: &[Vec<u8>]) -> Result<Merge> {
        self.receive_bundle(&SyncBundle {
            events: events.to_vec(),
        })
        .await
    }

    // ----------------------------------------------------------------- peers

    /// Remember a peer to sync with (replaces its addresses).
    pub async fn add_peer(&self, id: &str, addrs: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO peers (id, addrs) VALUES (?, ?)
             ON CONFLICT (id) DO UPDATE SET addrs = excluded.addrs",
        )
        .bind(id)
        .bind(addrs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_peers(&self) -> Result<Vec<Peer>> {
        Ok(sqlx::query_as::<_, Peer>("SELECT * FROM peers ORDER BY id")
            .fetch_all(&self.pool)
            .await?)
    }

    /// Record a successful sync with a peer, learning it (and any direct
    /// addresses it was dialed at) if it was unknown.
    pub async fn touch_peer(&self, id: &str, addrs: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO peers (id, addrs, last_seen) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET last_seen = ?3,
                 addrs = CASE WHEN ?2 = '' THEN addrs ELSE ?2 END",
        )
        .bind(id)
        .bind(addrs)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Apply a peer bundle (PRD-v2 sync). Events are verified, parked on
    /// missing parents, checked for authority (PRD-v3 R2, R3), drained, and
    /// applied for their state effect.
    pub async fn receive_bundle(&self, bundle: &SyncBundle) -> Result<Merge> {
        let mut merge = Merge::default();
        let mut tx = self.begin_write().await?;
        for bytes in &bundle.events {
            match StoredEvent::decode(bytes) {
                Ok(ev) => match insert_remote_tx(&mut tx, &ev, &mut merge.rejected_ids).await? {
                    Insert::Added => merge.added += 1,
                    Insert::Parked => merge.pending += 1,
                    Insert::Duplicate => merge.duplicates += 1,
                    Insert::Rejected => {}
                },
                Err(e) => merge.rejected_ids.push(e.to_string()),
            }
        }
        merge.rejected = merge.rejected_ids.len();
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

/// Return every stale claim's task to the pool as of `at` (PRD-v3 M1): a
/// claim is live in `[claimed_at, expires_at)`. The expiry instant stamps the
/// rows — the same values the fold derives on other replicas.
async fn sweep_expired_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "UPDATE tasks SET status = 'queue',
             updated_at = max(updated_at, (SELECT c.expires_at FROM claims c
                 WHERE c.task_id = tasks.id AND c.released_at IS NULL))
         WHERE status = 'work'
           AND id IN (SELECT task_id FROM claims
                      WHERE released_at IS NULL AND expires_at <= ?)",
    )
    .bind(at)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE claims SET released_at = expires_at, release_reason = 'expired'
         WHERE released_at IS NULL AND expires_at <= ?",
    )
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The workspace root: the actor whose self-signed `actor.created` is the
/// DAG's first event (PRD-v3 R3).
async fn root_actor_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT actor_id FROM events
         WHERE kind = 'actor.created' AND parents = '[]' AND event_id IS NOT NULL LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?)
}

async fn insert_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
    kind: ActorKind,
    name: &str,
    roles: &Roles,
    pubkey: &[u8; 32],
    at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("INSERT INTO actors (id, kind, name, created_at, roles) VALUES (?, ?, ?, ?, ?)")
        .bind(id)
        .bind(kind)
        .bind(name)
        .bind(at)
        .bind(roles.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO actor_keys (actor_id, pubkey) VALUES (?, ?)")
        .bind(id)
        .bind(pubkey.to_vec())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn require_task_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, id: &str) -> Result<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?)")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(Error::TaskNotFound(id.to_string()))
    }
}

/// What the task still waits for: declared blockers and subtasks that are
/// not `done` (PRD-v3 M4). Empty means claimable.
async fn waiting_on(conn: &mut SqliteConnection, task_id: &str) -> Result<Vec<String>> {
    let mut ids: Vec<String> = sqlx::query_scalar(
        "SELECT b.id FROM task_deps d JOIN tasks b ON b.id = d.blocked_by
         WHERE d.task_id = ? AND b.status != 'done' ORDER BY b.id",
    )
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    let children: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM tasks WHERE parent_id = ? AND status != 'done' ORDER BY created_at, id",
    )
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    ids.extend(children);
    Ok(ids)
}

/// Rebuild a task's dependency set from its `task.blocked` / `task.unblocked`
/// events in `(lamport, event_id)` order, so concurrent edits converge.
async fn refold_deps_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, task_id: &str) -> Result<()> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT kind, payload FROM events
         WHERE task_id = ? AND event_id IS NOT NULL
           AND kind IN ('task.blocked', 'task.unblocked')
         ORDER BY lamport, event_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut deps: Vec<String> = Vec::new();
    for (kind, payload) in rows {
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap_or(json!({}));
        let Some(by) = payload["by"].as_str() else {
            continue;
        };
        deps.retain(|d| d != by);
        if kind == "task.blocked" {
            deps.push(by.to_string());
        }
    }
    sqlx::query("DELETE FROM task_deps WHERE task_id = ?")
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    for by in deps {
        sqlx::query("INSERT INTO task_deps (task_id, blocked_by) VALUES (?, ?)")
            .bind(task_id)
            .bind(by)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// The actor's live claim on the task, or why it has none. Call after
/// `sweep_expired_tx`. `via`/`to` describe the attempted move for the error.
async fn require_claim_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    actor_id: &str,
    via: &str,
    to: TaskStatus,
) -> Result<String> {
    let own: Option<(String, Option<ReleaseReason>)> = sqlx::query_as(
        "SELECT id, release_reason FROM claims
         WHERE task_id = ? AND actor_id = ? ORDER BY claimed_at DESC LIMIT 1",
    )
    .bind(task_id)
    .bind(actor_id)
    .fetch_optional(&mut **tx)
    .await?;
    match own {
        Some((id, None)) => return Ok(id),
        Some((_, Some(ReleaseReason::Expired))) => {
            return Err(Error::ClaimExpired(task_id.to_string()));
        }
        _ => {}
    }
    let holder: Option<String> =
        sqlx::query_scalar("SELECT actor_id FROM claims WHERE task_id = ? AND released_at IS NULL")
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(holder) = holder {
        return Err(Error::ClaimConflict {
            task: task_id.to_string(),
            actor: holder,
        });
    }
    let status: Option<TaskStatus> = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?;
    match status {
        None => Err(Error::TaskNotFound(task_id.to_string())),
        Some(from) => Err(Error::InvalidTransition {
            task: task_id.to_string(),
            from: from.to_string(),
            via: via.to_string(),
            to: to.to_string(),
        }),
    }
}

/// An event row as needed to rebuild its signed envelope.
type EventRow = (
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
const EVENT_COLUMNS: &str =
    "lamport, parents, actor_id, task_id, kind, payload, at, signature, pubkey";

fn encode_row(row: EventRow) -> Vec<u8> {
    let (lamport, parents, actor_id, task_id, kind, payload, at, sig, pubkey) = row;
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
}

enum Insert {
    Added,
    Parked,
    Duplicate,
    /// The reason went to the caller's rejection list.
    Rejected,
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

/// Verify and insert one remote event: park on missing parents, else check
/// its authority and apply. Rejections (of this event, or of parked events it
/// unblocks) are appended to `rejected` as `<event id>: <reason>`.
async fn insert_remote_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
    rejected: &mut Vec<String>,
) -> Result<Insert> {
    let id = match ev.verify() {
        Ok(id) => id,
        Err(e) => {
            rejected.push(e.to_string());
            return Ok(Insert::Rejected);
        }
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
    if parents_known_tx(tx, ev.parent_ids()).await? {
        let admitted = admit_tx(tx, ev, &id, rejected).await?;
        drain_pending_tx(tx, rejected).await?;
        Ok(if admitted {
            Insert::Added
        } else {
            Insert::Rejected
        })
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

/// Admit an event whose parents are known: authority first (PRD-v3 R2, R3),
/// then the DAG and its state effect. A refused event never enters the DAG,
/// so nothing an honest replica writes can descend from it.
async fn admit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
    id: &EventId,
    rejected: &mut Vec<String>,
) -> Result<bool> {
    if let Some(reason) = refusal_tx(tx, ev).await? {
        rejected.push(format!("{}: {reason}", hex::encode(id)));
        return Ok(false);
    }
    if ev.body.kind == "actor.created" {
        // The event row references its author: for a root, the actor itself.
        apply_effect_tx(tx, ev).await?;
        insert_dag_tx(tx, ev, id).await?;
    } else {
        insert_dag_tx(tx, ev, id).await?;
        apply_effect_tx(tx, ev).await?;
    }
    Ok(true)
}

/// A new actor as described by an `actor.created` event.
struct NewActor {
    kind: ActorKind,
    name: String,
    roles: Roles,
    pubkey: [u8; 32],
}

fn new_actor(payload: &serde_json::Value) -> Option<NewActor> {
    Some(NewActor {
        kind: serde_json::from_value(payload["kind"].clone()).ok()?,
        name: payload["name"].as_str()?.to_string(),
        roles: serde_json::from_value(payload["roles"].clone()).ok()?,
        pubkey: hex::decode(payload["pubkey"].as_str()?)
            .ok()?
            .try_into()
            .ok()?,
    })
}

/// Why this event must not enter the DAG, if it must not. These checks
/// depend only on the event, its parents and its author's fixed attributes,
/// so every replica reaches the same verdict. What depends on the task's
/// state at that point in the log is decided by the fold.
async fn refusal_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
) -> Result<Option<String>> {
    let body = &ev.body;
    let refuse = |reason: &str| Ok(Some(reason.to_string()));
    let payload: serde_json::Value = serde_json::from_str(&body.payload).unwrap_or(json!({}));

    // A Lamport clock is one past its parents': it cannot be rewound to win
    // a first-wins race.
    let parent_max: Option<i64> = if body.parents.is_empty() {
        None
    } else {
        let marks = vec!["?"; body.parents.len()].join(",");
        let sql = format!("SELECT max(lamport) FROM events WHERE event_id IN ({marks})");
        let mut q = sqlx::query_scalar::<_, Option<i64>>(&sql);
        for p in &body.parents {
            q = q.bind(p.to_vec());
        }
        q.fetch_one(&mut **tx).await?
    };
    if body.lamport != parent_max.map_or(0, |m| m as u64 + 1) {
        return refuse("lamport clock does not follow its parents");
    }

    if body.kind == "actor.created" {
        let Some(actor) = new_actor(&payload) else {
            return refuse("malformed actor.created");
        };
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM actors WHERE id = ?)")
            .bind(&body.task_id)
            .fetch_one(&mut **tx)
            .await?;
        if exists {
            return refuse("actor already exists");
        }
        if body.parents.is_empty() {
            // The workspace root: the one self-signed human at the start.
            let started: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM events WHERE event_id IS NOT NULL)",
            )
            .fetch_one(&mut **tx)
            .await?;
            if started {
                return refuse("a different workspace: this one already has its root");
            }
            if body.actor_id != body.task_id
                || actor.pubkey != ev.pubkey
                || actor.kind != ActorKind::Human
            {
                return refuse("the workspace root must be a self-signed human");
            }
            return Ok(None);
        }
    } else if body.parents.is_empty() {
        return refuse("only the workspace root has no parents");
    }

    let signer: Option<(ActorKind, String, Vec<u8>)> = sqlx::query_as(
        "SELECT a.kind, a.roles, k.pubkey FROM actors a
         JOIN actor_keys k ON k.actor_id = a.id WHERE a.id = ?",
    )
    .bind(&body.actor_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((kind, roles, pubkey)) = signer else {
        return refuse("unknown actor");
    };
    if pubkey[..] != ev.pubkey[..] {
        return refuse("actor pubkey mismatch");
    }
    let roles = Roles::from(roles);
    let needs = |role: Role| {
        if roles.0.contains(&role) {
            Ok(None)
        } else {
            Ok(Some(format!("{} needs role {role}", body.kind)))
        }
    };
    match body.kind.as_str() {
        "actor.created" | "claim.force_released" if kind != ActorKind::Human => {
            refuse(&format!("{} is for humans only", body.kind))
        }
        "task.created" | "task.blocked" | "task.unblocked" | "task.protected" => {
            needs(Role::Planner)
        }
        "task.claimed" => needs(Role::Worker),
        "task.approved" | "task.changes_requested" => needs(Role::Reviewer),
        _ => Ok(None),
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
async fn drain_pending_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rejected: &mut Vec<String>,
) -> Result<()> {
    loop {
        let rows: Vec<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT event_id, body FROM pending_events ORDER BY received_at")
                .fetch_all(&mut **tx)
                .await?;
        let mut progress = false;
        for (eid, body) in rows {
            let ev = StoredEvent::decode(&body)?;
            if parents_known_tx(tx, ev.parent_ids()).await? {
                admit_tx(tx, &ev, &to_id(&eid), rejected).await?;
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

/// Deterministic state effect of one admitted event (PRD-v2 sync).
async fn apply_effect_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ev: &StoredEvent,
) -> Result<()> {
    let payload: serde_json::Value =
        serde_json::from_str(&ev.body.payload).unwrap_or(serde_json::json!({}));
    match ev.body.kind.as_str() {
        "actor.created" => {
            if let Some(a) = new_actor(&payload) {
                let id = &ev.body.task_id;
                insert_actor_tx(tx, id, a.kind, &a.name, &a.roles, &a.pubkey, ev.body.at).await?;
            }
        }
        "task.created" => {
            sqlx::query(
                "INSERT OR IGNORE INTO tasks
                     (id, title, body_md, status, created_by, created_at, updated_at, parent_id)
                 VALUES (?, ?, ?, 'queue', ?, ?, ?, ?)",
            )
            .bind(&ev.body.task_id)
            .bind(payload["title"].as_str().unwrap_or(""))
            .bind(payload["body_md"].as_str().unwrap_or(""))
            .bind(&ev.body.actor_id)
            .bind(ev.body.at)
            .bind(ev.body.at)
            .bind(payload["parent"].as_str())
            .execute(&mut **tx)
            .await?;
        }
        "task.blocked" | "task.unblocked" => {
            refold_deps_tx(tx, &ev.body.task_id).await?;
        }
        // Content, claims and status fold per task below (D8, PRD-v3 G1, R2).
        "task.updated"
        | "task.protected"
        | "task.claimed"
        | "task.renewed"
        | "claim.released"
        | "claim.force_released"
        | "task.submitted"
        | "task.approved"
        | "task.changes_requested" => {
            refold_task_tx(tx, &ev.body.task_id).await?;
        }
        _ => {} // Unknown kinds are logged but have no effect (forward compat).
    }
    Ok(())
}

/// A claim the fold currently considers the lock holder.
struct Held {
    id: String,
    actor_id: String,
    claimed_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// Fold state for one task.
struct Fold {
    held: Option<Held>,
    /// Claims that stopped holding the lock: (claim id, why, when).
    ended: Vec<(String, ReleaseReason, DateTime<Utc>)>,
    status: TaskStatus,
    status_at: Option<DateTime<Utc>>,
}

impl Fold {
    fn set(&mut self, status: TaskStatus, at: DateTime<Utc>) {
        self.status = status;
        self.status_at = Some(at);
    }

    /// A claim is live in `[claimed_at, expires_at)`: by `at`, has it lapsed?
    fn expire(&mut self, at: DateTime<Utc>) {
        if let Some(h) = self.held.take_if(|h| h.expires_at <= at) {
            self.set(TaskStatus::Queue, h.expires_at);
            self.ended
                .push((h.id, ReleaseReason::Expired, h.expires_at));
        }
    }
}

/// Re-fold a task's events in `(lamport, event_id)` order into its content,
/// claim and status. The first claim wins the lock; later concurrent claims
/// stay in the log but are ineffective (PRD-v2 D8). The status walks the gate,
/// and an event that does not fit the state reached so far — or whose author
/// had no right to it at that point (PRD-v3 R2: not the holder, approving its
/// own or a superseded submission, an agent on a protected task) — is
/// ineffective too. Expiry is judged against each event's own `at` — the local
/// clock only decides whether the last holder has lapsed by now — and a
/// renewal the holder made before its claim lapsed always counts, wherever it
/// sorts in the log (PRD-v3 M1). Recomputed wholesale on every merge that
/// touches the task, so the result never depends on arrival order.
async fn refold_task_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, task_id: &str) -> Result<()> {
    // (event id, kind, actor, at, payload)
    type Row = (Vec<u8>, String, String, DateTime<Utc>, String);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT event_id, kind, actor_id, at, payload FROM events
         WHERE task_id = ?1 AND event_id IS NOT NULL
           AND kind IN ('task.created','task.updated','task.protected','task.claimed',
                        'task.renewed','claim.released','claim.force_released',
                        'task.submitted','task.approved','task.changes_requested')
         ORDER BY lamport, event_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    let actors: HashMap<String, (ActorKind, Roles)> =
        sqlx::query_as::<_, (String, ActorKind, String)>("SELECT id, kind, roles FROM actors")
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|(id, kind, roles)| (id, (kind, roles.into())))
            .collect();

    let json = |payload: &str| -> serde_json::Value {
        serde_json::from_str(payload).unwrap_or(serde_json::json!({}))
    };
    let claim_id_of = |p: &serde_json::Value| p["claim_id"].as_str().unwrap_or("").to_string();
    let expiry_of = |p: &serde_json::Value| {
        serde_json::from_value::<DateTime<Utc>>(p["expires_at"].clone()).ok()
    };
    // Each claim's final expiry: its own TTL, moved by its holder's renewals
    // in time order, each counting only if made before the claim lapsed.
    let mut expiry: HashMap<String, (&str, DateTime<Utc>)> = HashMap::new();
    let mut renewals = Vec::new();
    for (_, kind, actor_id, at, payload) in &rows {
        let p = json(payload);
        match (kind.as_str(), expiry_of(&p)) {
            ("task.claimed", Some(exp)) => {
                expiry.insert(claim_id_of(&p), (actor_id.as_str(), exp));
            }
            ("task.renewed", Some(exp)) => {
                renewals.push((*at, claim_id_of(&p), actor_id.as_str(), exp));
            }
            _ => {}
        }
    }
    renewals.sort();
    for (at, claim_id, actor_id, exp) in renewals {
        if let Some((holder, current)) = expiry.get_mut(&claim_id) {
            if *holder == actor_id && at < *current {
                *current = exp;
            }
        }
    }
    let expiry: HashMap<String, DateTime<Utc>> =
        expiry.into_iter().map(|(id, (_, exp))| (id, exp)).collect();

    let now = Utc::now();
    let mut f = Fold {
        held: None,
        ended: Vec::new(),
        status: TaskStatus::Queue,
        status_at: None,
    };
    // Content, and its latest change; with `status_at` it derives `updated_at`.
    let (mut title, mut body_md, mut priority) = (None, None, None);
    let mut content_at: Option<DateTime<Utc>> = None;
    let mut protected = false;
    // The submission now in review: (event id, submitter).
    let mut in_review: Option<(String, String)> = None;

    for (event_id, kind, actor_id, at, payload) in rows {
        let p = json(&payload);
        let claim_id = claim_id_of(&p);
        let who = actors.get(&actor_id);
        let human = matches!(who, Some((ActorKind::Human, _)));
        let planner = who.is_some_and(|(_, roles)| roles.0.contains(&Role::Planner));
        f.expire(at);
        match kind.as_str() {
            "task.created" => {
                title = p["title"].as_str().map(str::to_string);
                body_md = p["body_md"].as_str().map(str::to_string);
                content_at = content_at.max(Some(at));
            }
            // While claimed only the holder edits; otherwise a planner.
            "task.updated" if f.held.as_ref().map_or(planner, |h| h.actor_id == actor_id) => {
                if let Some(t) = p["title"].as_str() {
                    title = Some(t.to_string());
                }
                if let Some(b) = p["body_md"].as_str() {
                    body_md = Some(b.to_string());
                }
                if let Some(n) = p["priority"].as_i64() {
                    priority = Some(n);
                }
                content_at = content_at.max(Some(at));
            }
            "task.protected" => protected = true,
            "task.claimed" if f.held.is_none() && f.status == TaskStatus::Queue => {
                if let Some(&expires_at) = expiry.get(&claim_id) {
                    f.held = Some(Held {
                        id: claim_id,
                        actor_id,
                        claimed_at: at,
                        expires_at,
                    });
                    f.set(TaskStatus::Work, at);
                }
            }
            // Any other claim: the task is held or past `queue` — ineffective (D8).
            "claim.released" | "claim.force_released" | "task.submitted" => {
                // Only the holder releases or submits; a human may force.
                let forced = kind == "claim.force_released";
                if let Some(h) = f
                    .held
                    .take_if(|h| h.id == claim_id && (forced || h.actor_id == actor_id))
                {
                    let (reason, status) = match kind.as_str() {
                        "claim.force_released" => (ReleaseReason::Forced, TaskStatus::Queue),
                        "task.submitted" => (ReleaseReason::Done, TaskStatus::Review),
                        _ => (ReleaseReason::Released, TaskStatus::Queue),
                    };
                    if kind == "task.submitted" {
                        in_review = Some((hex::encode(&event_id), actor_id));
                    }
                    f.ended.push((h.id, reason, at));
                    f.set(status, at);
                }
            }
            "task.approved" if f.status == TaskStatus::Review => {
                // It must name the submission in review (R4), not be the
                // submitting agent's own (G2), and be a human's on a
                // protected task (R5).
                let counts = in_review.as_ref().is_some_and(|(id, submitter)| {
                    p["submission"].as_str() == Some(id.as_str())
                        && (human || *submitter != actor_id)
                }) && (human || !protected);
                if counts {
                    f.set(TaskStatus::Done, at);
                }
            }
            "task.changes_requested" if f.status == TaskStatus::Review => {
                f.set(TaskStatus::Queue, at);
            }
            _ => {}
        }
    }
    f.expire(now);

    // Materialize the claims table from the fold.
    for (cid, reason, at) in f.ended {
        sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = ? WHERE id = ? AND released_at IS NULL",
        )
        .bind(at)
        .bind(reason)
        .bind(cid)
        .execute(&mut **tx)
        .await?;
    }
    let holder_id = f.held.as_ref().map_or(String::new(), |h| h.id.clone());
    if let Some(h) = f.held {
        // The winner's row may be missing here (claimed elsewhere) or stale
        // (swept before its renewal arrived).
        sqlx::query(
            "INSERT INTO claims (id, task_id, actor_id, claimed_at, expires_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 expires_at = excluded.expires_at, released_at = NULL, release_reason = NULL",
        )
        .bind(&h.id)
        .bind(task_id)
        .bind(h.actor_id)
        .bind(h.claimed_at)
        .bind(h.expires_at)
        .execute(&mut **tx)
        .await?;
    }
    // Any other still-active claim is a fold loser (D8).
    sqlx::query(
        "UPDATE claims SET released_at = ?, release_reason = 'released'
         WHERE task_id = ? AND released_at IS NULL AND id != ?",
    )
    .bind(now)
    .bind(task_id)
    .bind(holder_id)
    .execute(&mut **tx)
    .await?;
    // The events that decided status and content stamp `updated_at`, so
    // replicas agree on it as well.
    sqlx::query(
        "UPDATE tasks SET
             title = COALESCE(?, title), body_md = COALESCE(?, body_md),
             priority = COALESCE(?, priority), protected = max(protected, ?),
             status = ?, updated_at = COALESCE(?, updated_at)
         WHERE id = ?",
    )
    .bind(title)
    .bind(body_md)
    .bind(priority)
    .bind(protected)
    .bind(f.status)
    .bind(f.status_at.max(content_at))
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

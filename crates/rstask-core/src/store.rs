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

use chrono::Utc;
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::models::{Actor, ActorKind, Claim, Event, ReleaseReason, Task, TaskStatus};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");

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
        sqlx::raw_sql(MIGRATION_0001).execute(&pool).await?;
        Ok(Self { pool, keepalive: None })
    }

    /// In-memory store for tests.
    ///
    /// Uses a shared-cache in-memory database: a dedicated keep-alive
    /// connection holds the database open so all pool connections share one
    /// schema (a plain `:memory:` database is per-connection).
    pub async fn open_in_memory() -> Result<Self> {
        let url = format!("file:rstask_mem_{}?mode=memory&cache=shared", Uuid::new_v4());
        let mut keepalive = SqliteConnection::connect(&url).await?;
        sqlx::raw_sql(MIGRATION_0001).execute(&mut keepalive).await?;

        let options = SqliteConnectOptions::from_str(&url)?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(MIGRATION_0001).execute(&pool).await?;
        Ok(Self { pool, keepalive: Some(keepalive) })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---------------------------------------------------------------- actors

    pub async fn create_actor(&self, kind: ActorKind, name: &str) -> Result<Actor> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO actors (id, kind, name, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(kind)
        .bind(name)
        .bind(now)
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

    pub async fn create_task(
        &self,
        title: &str,
        body_md: &str,
        created_by: &str,
    ) -> Result<Task> {
        // Fail fast on unknown actor (FK would too, but with a worse message).
        self.get_actor(created_by).await?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
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
        .execute(&self.pool)
        .await?;

        self.log_event(&id, Some(created_by), "task.created", json!({ "title": title }))
            .await?;

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
                sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE status = ? ORDER BY priority DESC, created_at")
                    .bind(s)
                    .fetch_all(&self.pool)
                    .await
            }
            None => {
                sqlx::query_as::<_, Task>(
                    "SELECT * FROM tasks ORDER BY priority DESC, created_at",
                )
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
        let task = self.get_task(id).await?;
        let now = Utc::now();
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
        .execute(&self.pool)
        .await?;

        self.log_event(
            id,
            Some(actor_id),
            "task.updated",
            json!({
                "old_title": task.title,
                "old_priority": task.priority,
            }),
        )
        .await?;

        self.get_task(id).await
    }

    // ---------------------------------------------------------------- claims

    /// Atomically claim a task: sweeps expired claims, rejects if another
    /// active claim exists, moves queue → work, appends events.
    ///
    /// Returns `Error::ClaimConflict` if the task is already claimed, and
    /// `Error::InvalidTransition` if the task is not in `queue`.
    pub async fn claim_task(
        &self,
        task_id: &str,
        actor_id: &str,
        ttl: Duration,
    ) -> Result<Claim> {
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
                let holder: Option<(String,)> =
                    sqlx::query_as("SELECT actor_id FROM claims WHERE task_id = ? AND released_at IS NULL")
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

        self.log_event_tx(
            &mut tx,
            task_id,
            Some(actor_id),
            "task.claimed",
            json!({ "claim_id": claim.id, "expires_at": claim.expires_at }),
        )
        .await?;

        tx.commit().await?;
        Ok(claim)
    }

    /// Release a claim. `forced = true` models the human force-release path
    /// for stale agent claims.
    pub async fn release_claim(
        &self,
        task_id: &str,
        actor_id: &str,
        forced: bool,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        let reason = if forced {
            ReleaseReason::Forced
        } else {
            ReleaseReason::Released
        };
        let res = sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = ?
             WHERE task_id = ? AND actor_id = ? AND released_at IS NULL",
        )
        .bind(Utc::now())
        .bind(reason)
        .bind(task_id)
        .bind(actor_id)
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

        self.log_event_tx(
            &mut tx,
            task_id,
            Some(actor_id),
            if forced { "claim.force_released" } else { "claim.released" },
            json!({}),
        )
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Renew a claim's TTL (heartbeat from the claim holder).
    pub async fn renew_claim(&self, task_id: &str, actor_id: &str, ttl: Duration) -> Result<()> {
        let expires_at = Utc::now()
            + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
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
        let mut tx = self.pool.begin().await?;
        self.transition_tx(
            &mut tx,
            task_id,
            actor_id,
            TaskStatus::Work,
            TaskStatus::Review,
            "task.submitted",
            json!({}),
        )
        .await?;
        // Release the submitter's claim: work on this task is done.
        sqlx::query(
            "UPDATE claims SET released_at = ?, release_reason = 'done'
             WHERE task_id = ? AND actor_id = ? AND released_at IS NULL",
        )
        .bind(Utc::now())
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
        let mut tx = self.pool.begin().await?;
        self.transition_tx(
            &mut tx,
            task_id,
            actor_id,
            TaskStatus::Review,
            TaskStatus::Done,
            "task.approved",
            json!({}),
        )
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    /// Send back for rework: review → queue.
    pub async fn request_changes(&self, task_id: &str, actor_id: &str) -> Result<Task> {
        let mut tx = self.pool.begin().await?;
        self.transition_tx(
            &mut tx,
            task_id,
            actor_id,
            TaskStatus::Review,
            TaskStatus::Queue,
            "task.changes_requested",
            json!({}),
        )
        .await?;
        tx.commit().await?;
        self.get_task(task_id).await
    }

    #[allow(clippy::too_many_arguments)] // internal helper; params are one logical transition
    async fn transition_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: &str,
        actor_id: &str,
        from: TaskStatus,
        to: TaskStatus,
        event_kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        let res = sqlx::query("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ? AND status = ?")
            .bind(to)
            .bind(Utc::now())
            .bind(task_id)
            .bind(from)
            .execute(&mut **tx)
            .await?;
        if res.rows_affected() == 0 {
            let current: TaskStatus =
                sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
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
        self.log_event_tx(tx, task_id, Some(actor_id), event_kind, payload)
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
            "SELECT * FROM events WHERE task_id = ? ORDER BY seq",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    async fn log_event(
        &self,
        task_id: &str,
        actor_id: Option<&str>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO events (task_id, actor_id, kind, payload, at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(actor_id)
        .bind(kind)
        .bind(payload.to_string())
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn log_event_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: &str,
        actor_id: Option<&str>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO events (task_id, actor_id, kind, payload, at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(actor_id)
        .bind(kind)
        .bind(payload.to_string())
        .bind(Utc::now())
        .execute(&mut **tx)
        .await?;
        Ok(())
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

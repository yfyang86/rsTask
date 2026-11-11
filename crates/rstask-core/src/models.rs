//! Domain models. IDs are UUIDs serialized as TEXT in SQLite.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;

/// Workflow gate: queue → work → review → done.
/// Agents cannot move a task into `done` themselves (no self-approval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queue,
    Work,
    Review,
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "queue",
            TaskStatus::Work => "work",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Type, Serialize, Deserialize)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    Done,
    Released,
    Expired,
    Forced,
}

/// Built-in module identifiers. Modules are orthogonal: a task may carry
/// any combination of these via `task_modules` rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Module {
    Sprint,
    Kanban,
    Okr,
    Waterfall,
    Brainstorm,
    Pipeline,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Actor {
    pub id: String,
    pub kind: ActorKind,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub body_md: String,
    pub status: TaskStatus,
    pub priority: i64,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Claim {
    pub id: String,
    pub task_id: String,
    pub actor_id: String,
    pub claimed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub release_reason: Option<ReleaseReason>,
}

/// Immutable log entry. `seq` is a monotonic AUTOINCREMENT — the event log
/// is append-only by construction.
///
/// DAG columns (PRD-v2 D3): `event_id`/`lamport`/`parents`/`signature`/`pubkey`
/// are NULL on v1 rows, which stay local-only and are never synced.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Event {
    pub seq: i64,
    pub task_id: String,
    pub actor_id: Option<String>,
    pub kind: String,
    pub payload: String,
    pub at: DateTime<Utc>,
    pub event_id: Option<Vec<u8>>,
    pub lamport: Option<i64>,
    /// JSON array of hex parent ids.
    pub parents: String,
    pub signature: Option<Vec<u8>>,
    pub pubkey: Option<Vec<u8>>,
}

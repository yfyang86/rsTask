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

/// What an actor may do (PRD-v3 M5): planners create and shape tasks, workers
/// claim them, reviewers approve or reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Planner,
    Worker,
    Reviewer,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Planner, Role::Worker, Role::Reviewer];

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Planner => "planner",
            Role::Worker => "worker",
            Role::Reviewer => "reviewer",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An actor's roles; stored as comma-separated TEXT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Roles(pub Vec<Role>);

impl From<String> for Roles {
    fn from(s: String) -> Self {
        Roles(
            Role::ALL
                .into_iter()
                .filter(|r| s.split(',').any(|part| part == r.as_str()))
                .collect(),
        )
    }
}

impl std::fmt::Display for Roles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.0.iter().map(Role::as_str).collect();
        f.write_str(&names.join(","))
    }
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
    #[sqlx(try_from = "String")]
    pub roles: Roles,
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
    /// Set on subtasks; a parent waits for its subtasks.
    pub parent_id: Option<String>,
    /// Only a human's approval counts (PRD-v3 R5).
    pub protected: bool,
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

/// What a worker hands to the reviewer with `task.submitted` (PRD-v3 M2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Submission {
    pub summary: String,
    /// Commit ids, PR links, file paths — whatever locates the work.
    pub refs: Vec<String>,
    /// Test output or other proof, as Markdown.
    pub evidence: String,
}

/// The latest `task.submitted` on a task.
#[derive(Debug, Clone, Serialize)]
pub struct Submitted {
    /// Id of the `task.submitted` event: what an approval must name (R4).
    pub id: String,
    pub by: String,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub submission: Submission,
}

/// The latest `task.changes_requested` on a task.
#[derive(Debug, Clone, Serialize)]
pub struct Rejected {
    pub by: String,
    pub at: DateTime<Utc>,
    pub reason: String,
}

/// A task with what an actor needs to pick it up or review it: who holds it
/// the latest handoff in each direction (PRD-v3 M3) and what it waits for.
#[derive(Debug, Clone, Serialize)]
pub struct TaskDetail {
    #[serde(flatten)]
    pub task: Task,
    pub claim: Option<Claim>,
    pub submission: Option<Submitted>,
    pub rejection: Option<Rejected>,
    /// Tasks this one was declared to wait for (PRD-v3 M4).
    pub blocked_by: Vec<String>,
    /// Its subtasks.
    pub children: Vec<String>,
    /// Blockers and subtasks not yet `done`: empty means claimable.
    pub waiting_on: Vec<String>,
}

/// A replica this one syncs with (PRD-v2 D1).
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Peer {
    /// iroh endpoint id.
    pub id: String,
    /// Comma-separated direct socket addresses; empty means dial by id.
    pub addrs: String,
    /// Last successful sync.
    pub last_seen: Option<DateTime<Utc>>,
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

//! rstask-mcp — MCP server exposing agent tools over the shared core.
//!
//! One server process per agent, over stdio. The actor is fixed at startup
//! (`--actor` or `RSTASK_ACTOR`); no tool takes an actor argument, so an agent
//! cannot act as someone else through this interface. Every tool calls
//! `rstask_core::Store`, so claim TTL, the workflow gate and the event log
//! hold for agents exactly as they do for humans.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use rstask_core::{Actor, Config, Store, Submission, TaskStatus};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(name = "rstask-mcp", about = "RsTask MCP server for one agent (stdio)")]
struct Cli {
    /// The actor this server acts as (or env RSTASK_ACTOR).
    #[arg(long)]
    actor: Option<String>,

    /// Path to the SQLite database (flag > RSTASK_STORAGE_DB_PATH > config).
    #[arg(long)]
    db: Option<PathBuf>,

    /// Path to config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Default claim TTL, as in the CLI: 15 min.
const DEFAULT_TTL_SECS: u64 = 900;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Status {
    Queue,
    Work,
    Review,
    Done,
}

impl From<Status> for TaskStatus {
    fn from(s: Status) -> Self {
        match s {
            Status::Queue => TaskStatus::Queue,
            Status::Work => TaskStatus::Work,
            Status::Review => TaskStatus::Review,
            Status::Done => TaskStatus::Done,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListArgs {
    /// Only tasks in this status.
    status: Option<Status>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct IdArgs {
    /// Task id.
    id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct TtlArgs {
    /// Claim lifetime in seconds (default 900). Renew before it runs out.
    ttl_secs: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ClaimArgs {
    /// Task id.
    id: String,
    /// Claim lifetime in seconds (default 900). Renew before it runs out.
    ttl_secs: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct CreateArgs {
    title: String,
    /// Markdown body: what to do and how it will be judged.
    body_md: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct UpdateArgs {
    /// Task id.
    id: String,
    title: Option<String>,
    body_md: Option<String>,
    /// Higher is claimed first by `claim_next`.
    priority: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct CompleteArgs {
    /// Task id.
    id: String,
    /// What was done.
    summary: String,
    /// Where the work is: commit ids, PR links, file paths.
    refs: Option<Vec<String>>,
    /// Proof it works: test output, as Markdown.
    evidence: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RejectArgs {
    /// Task id.
    id: String,
    /// What must change, for whoever picks the task up next.
    reason: String,
}

fn ttl(secs: Option<u64>) -> Duration {
    Duration::from_secs(secs.unwrap_or(DEFAULT_TTL_SECS))
}

/// Core results become tool results: JSON text on success, core's own
/// message as a tool error otherwise.
fn reply<T: Serialize>(result: rstask_core::Result<T>) -> CallToolResult {
    match result.map(|v| serde_json::to_string_pretty(&v)) {
        Ok(Ok(json)) => CallToolResult::success(vec![ContentBlock::text(json)]),
        Ok(Err(e)) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
    }
}

struct Server {
    store: Store,
    actor: Actor,
    tool_router: ToolRouter<Self>,
}

#[tool_router(router = tool_router)]
impl Server {
    fn new(store: Store, actor: Actor) -> Self {
        Self {
            store,
            actor,
            tool_router: Self::tool_router(),
        }
    }

    fn me(&self) -> &str {
        &self.actor.id
    }

    #[tool(description = "List tasks, highest priority first; optionally one status only.")]
    async fn tasks_list(&self, Parameters(a): Parameters<ListArgs>) -> CallToolResult {
        reply(self.store.list_tasks(a.status.map(Into::into)).await)
    }

    #[tool(
        description = "One task with who holds it, the latest submission and the latest rejection reason."
    )]
    async fn task_get(&self, Parameters(a): Parameters<IdArgs>) -> CallToolResult {
        reply(self.store.task_detail(&a.id).await)
    }

    #[tool(description = "The board: tasks grouped by status (queue, work, review, done).")]
    async fn board_state(&self) -> CallToolResult {
        let board = self.store.board_state().await.map(|columns| {
            columns
                .into_iter()
                .map(|(status, tasks)| (status.to_string(), serde_json::json!(tasks)))
                .collect::<serde_json::Map<_, _>>()
        });
        reply(board)
    }

    #[tool(
        description = "Atomically claim the next task to work on (highest priority, then oldest). Returns the task as task_get does, or null when the queue is empty. Prefer this over picking from tasks_list."
    )]
    async fn claim_next(&self, Parameters(a): Parameters<TtlArgs>) -> CallToolResult {
        let detail = async {
            match self.store.claim_next(self.me(), ttl(a.ttl_secs)).await? {
                Some(claim) => self.store.task_detail(&claim.task_id).await.map(Some),
                None => Ok(None),
            }
        };
        reply(detail.await)
    }

    #[tool(description = "Claim a specific queued task. Fails if someone else holds it.")]
    async fn task_claim(&self, Parameters(a): Parameters<ClaimArgs>) -> CallToolResult {
        let detail = async {
            self.store
                .claim_task(&a.id, self.me(), ttl(a.ttl_secs))
                .await?;
            self.store.task_detail(&a.id).await
        };
        reply(detail.await)
    }

    #[tool(
        description = "Extend your claim before it expires. An expired claim is gone: the task returns to the queue."
    )]
    async fn task_renew(&self, Parameters(a): Parameters<ClaimArgs>) -> CallToolResult {
        reply(
            self.store
                .renew_claim(&a.id, self.me(), ttl(a.ttl_secs))
                .await,
        )
    }

    #[tool(description = "Give up your claim; the task returns to the queue.")]
    async fn task_release(&self, Parameters(a): Parameters<IdArgs>) -> CallToolResult {
        reply(
            self.store
                .release_claim(&a.id, self.me(), false)
                .await
                .map(|()| "released"),
        )
    }

    #[tool(description = "Create a task in the queue.")]
    async fn task_create(&self, Parameters(a): Parameters<CreateArgs>) -> CallToolResult {
        let body = a.body_md.unwrap_or_default();
        reply(self.store.create_task(&a.title, &body, self.me()).await)
    }

    #[tool(
        description = "Edit a task's title, body or priority. While a task is claimed only its holder may edit it."
    )]
    async fn task_update(&self, Parameters(a): Parameters<UpdateArgs>) -> CallToolResult {
        reply(
            self.store
                .update_task(
                    &a.id,
                    a.title.as_deref(),
                    a.body_md.as_deref(),
                    a.priority,
                    self.me(),
                )
                .await,
        )
    }

    #[tool(
        description = "Submit your claimed task for review with a summary, references and evidence. Releases your claim; someone else must approve."
    )]
    async fn task_complete(&self, Parameters(a): Parameters<CompleteArgs>) -> CallToolResult {
        let submission = Submission {
            summary: a.summary,
            refs: a.refs.unwrap_or_default(),
            evidence: a.evidence.unwrap_or_default(),
        };
        reply(
            self.store
                .submit_for_review(&a.id, self.me(), &submission)
                .await,
        )
    }

    #[tool(
        description = "Approve a task in review (review -> done). You cannot approve your own submission."
    )]
    async fn task_approve(&self, Parameters(a): Parameters<IdArgs>) -> CallToolResult {
        reply(self.store.approve(&a.id, self.me()).await)
    }

    #[tool(description = "Send a task in review back to the queue, with the reason.")]
    async fn task_reject(&self, Parameters(a): Parameters<RejectArgs>) -> CallToolResult {
        reply(
            self.store
                .request_changes(&a.id, self.me(), &a.reason)
                .await,
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(format!(
                "RsTask workspace. You act as {:?} actor \"{}\" ({}). \
                 Work loop: claim_next -> task_renew while working -> task_complete. \
                 Review loop: tasks_list status=review -> task_get -> task_approve or task_reject.",
                self.actor.kind, self.actor.name, self.actor.id
            ))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let actor_id = cli
        .actor
        .or_else(|| std::env::var("RSTASK_ACTOR").ok())
        .ok_or_else(|| anyhow::anyhow!("no actor: pass --actor <id> or set RSTASK_ACTOR"))?;
    let cfg = Config::load(cli.config.as_deref())?;
    let store = Store::open(&cli.db.unwrap_or(cfg.storage.db_path)).await?;
    let actor = store.get_actor(&actor_id).await?;

    // stdout carries the protocol; anything for humans goes to stderr.
    let service = Server::new(store, actor)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

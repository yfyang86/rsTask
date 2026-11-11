//! rstask — scriptable human operations over the shared RsTask core.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use rstask_core::{ActorKind, Config, Event, Store, Submission, Task, TaskStatus};

#[derive(Parser)]
#[command(name = "rstask", about = "Local-first, Markdown-native task workspace")]
struct Cli {
    /// Path to the SQLite database (CLI flag > RSTASK_STORAGE_DB_PATH > config).
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Path to config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the workspace database (idempotent).
    Init,

    /// Register a human or agent actor; prints the new actor id.
    Actor {
        #[command(subcommand)]
        command: ActorCommand,
    },

    /// Create and move tasks along the workflow gate.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },

    /// Kanban-style aggregate: tasks grouped by status.
    Board,

    /// Sync the signed event DAG with another replica (file bundles; wire in sprint 02).
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
}

#[derive(Subcommand)]
enum SyncCommand {
    /// Export the signed event DAG + actor directory as a binary bundle (stdout).
    Export,
    /// Import a bundle from stdin; prints merge stats as JSON.
    Import,
    /// Show sync state: heads, DAG event count, pending count.
    Status,
}

#[derive(Subcommand)]
enum ActorCommand {
    New {
        name: String,
        #[arg(long, value_enum, default_value = "human")]
        kind: KindArg,
    },
    Heartbeat {
        id: String,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum KindArg {
    Human,
    Agent,
}

#[derive(Subcommand)]
enum TaskCommand {
    New {
        title: String,
        #[arg(long, default_value = "")]
        body: String,
        /// Actor creating the task.
        #[arg(long)]
        actor: String,
    },
    List {
        #[arg(long, value_enum)]
        status: Option<StatusArg>,
    },
    Show {
        id: String,
    },
    /// Claim = atomic lock with TTL. Default TTL 15 min.
    Claim {
        id: String,
        #[arg(long)]
        actor: String,
        #[arg(long, default_value_t = 900)]
        ttl_secs: u64,
    },
    /// Release a claim (human force-release when stale).
    Release {
        id: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        force: bool,
    },
    /// Extend the holder's claim before it expires. Default TTL 15 min.
    Renew {
        id: String,
        #[arg(long)]
        actor: String,
        #[arg(long, default_value_t = 900)]
        ttl_secs: u64,
    },
    /// Work → review. Releases the claim; the result goes to the reviewer.
    Submit {
        id: String,
        #[arg(long)]
        actor: String,
        /// What was done.
        #[arg(long, default_value = "")]
        summary: String,
        /// Where the work is: commit id, PR link, file path (repeatable).
        #[arg(long = "ref")]
        refs: Vec<String>,
        /// Proof it works: test output, as Markdown.
        #[arg(long, default_value = "")]
        evidence: String,
    },
    /// Review → done (reviewer approval; agents can't self-approve).
    Approve {
        id: String,
        #[arg(long)]
        actor: String,
    },
    /// Review → queue (send back for rework).
    Reject {
        id: String,
        #[arg(long)]
        actor: String,
        /// What must change, for whoever picks the task up next.
        #[arg(long)]
        reason: String,
    },
    /// Show the immutable event log of a task.
    Log {
        id: String,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum StatusArg {
    Queue,
    Work,
    Review,
    Done,
}

impl From<StatusArg> for TaskStatus {
    fn from(s: StatusArg) -> Self {
        match s {
            StatusArg::Queue => TaskStatus::Queue,
            StatusArg::Work => TaskStatus::Work,
            StatusArg::Review => TaskStatus::Review,
            StatusArg::Done => TaskStatus::Done,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    let db = cli.db.unwrap_or(cfg.storage.db_path);
    let store = Store::open(&db).await?;

    match cli.command {
        Command::Init => {
            println!("initialized {}", db.display());
        }
        Command::Actor { command } => match command {
            ActorCommand::New { name, kind } => {
                let kind = match kind {
                    KindArg::Human => ActorKind::Human,
                    KindArg::Agent => ActorKind::Agent,
                };
                let actor = store.create_actor(kind, &name).await?;
                println!("{}", serde_json::to_string_pretty(&actor)?);
            }
            ActorCommand::Heartbeat { id } => {
                store.heartbeat(&id).await?;
            }
        },
        Command::Task { command } => match command {
            TaskCommand::New { title, body, actor } => {
                let task = store.create_task(&title, &body, &actor).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::List { status } => {
                let tasks = store.list_tasks(status.map(Into::into)).await?;
                print_task_table(&tasks);
            }
            TaskCommand::Show { id } => {
                let task = store.get_task(&id).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::Claim {
                id,
                actor,
                ttl_secs,
            } => {
                let claim = store
                    .claim_task(&id, &actor, Duration::from_secs(ttl_secs))
                    .await?;
                println!("{}", serde_json::to_string_pretty(&claim)?);
            }
            TaskCommand::Release { id, actor, force } => {
                store.release_claim(&id, &actor, force).await?;
            }
            TaskCommand::Renew {
                id,
                actor,
                ttl_secs,
            } => {
                let claim = store
                    .renew_claim(&id, &actor, Duration::from_secs(ttl_secs))
                    .await?;
                println!("{}", serde_json::to_string_pretty(&claim)?);
            }
            TaskCommand::Submit {
                id,
                actor,
                summary,
                refs,
                evidence,
            } => {
                let submission = Submission {
                    summary,
                    refs,
                    evidence,
                };
                let task = store.submit_for_review(&id, &actor, &submission).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::Approve { id, actor } => {
                let task = store.approve(&id, &actor).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::Reject { id, actor, reason } => {
                let task = store.request_changes(&id, &actor, &reason).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::Log { id } => {
                let events = store.list_events(&id).await?;
                let rendered: Vec<serde_json::Value> = events.iter().map(pretty_event).collect();
                println!("{}", serde_json::to_string_pretty(&rendered)?);
            }
        },
        Command::Board => {
            let board = store.board_state().await?;
            for (status, tasks) in board {
                println!("\n== {status} ({}) ==", tasks.len());
                print_task_table(&tasks);
            }
        }
        Command::Sync { command } => match command {
            SyncCommand::Export => {
                let bundle = store.export_bundle().await?;
                let bytes = rstask_core::sync::encode_bundle(&bundle);
                std::io::Write::write_all(&mut std::io::stdout(), &bytes)?;
            }
            SyncCommand::Import => {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)?;
                let bundle = rstask_core::sync::decode_bundle(&buf)?;
                let merge = store.receive_bundle(&bundle).await?;
                println!("{}", serde_json::to_string_pretty(&merge)?);
            }
            SyncCommand::Status => {
                let heads = store.heads().await?;
                let status = serde_json::json!({
                    "heads": heads.iter().map(hex::encode).collect::<Vec<_>>(),
                    "events": store.dag_event_count().await?,
                    "pending": store.pending_count().await?,
                });
                println!("{}", serde_json::to_string_pretty(&status)?);
            }
        },
    }
    Ok(())
}

/// Byte-array DAG fields render as hex for humans (WT-1); `null`s on pre-DAG
/// rows are left untouched.
fn pretty_event(ev: &Event) -> serde_json::Value {
    let mut v = serde_json::to_value(ev).unwrap_or_default();
    for key in ["event_id", "signature", "pubkey"] {
        let bytes = v.get(key).and_then(|b| b.as_array()).and_then(|arr| {
            arr.iter()
                .map(|n| n.as_u64().map(|x| x as u8))
                .collect::<Option<Vec<u8>>>()
        });
        if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
            v[key] = serde_json::json!(hex::encode(bytes));
        }
    }
    v
}

fn print_task_table(tasks: &[Task]) {
    if tasks.is_empty() {
        println!("(no tasks)");
        return;
    }
    println!("{:<8}  {:<8}  {:<5}  title", "id", "status", "prio");
    for t in tasks {
        let short = &t.id[..8.min(t.id.len())];
        println!(
            "{:<8}  {:<8}  {:<5}  {}",
            short, t.status, t.priority, t.title
        );
    }
}

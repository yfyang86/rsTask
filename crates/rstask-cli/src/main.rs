//! rstask — scriptable human operations over the shared RsTask core.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use rstask_core::net::{load_or_create_node_key, NetConfig, Node, PeerSpec};
use rstask_core::{ActorKind, Config, Event, Role, Store, Submission, Task, TaskStatus};

#[derive(Parser)]
#[command(name = "rstask", about = "Local-first, Markdown-native task workspace")]
struct Cli {
    /// Path to the SQLite database (CLI flag > RSTASK_STORAGE_DB_PATH > config).
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Path to config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Directory of actor secret keys (flag > RSTASK_AUTH_KEY_DIR > config >
    /// `keys/` beside the database).
    #[arg(long, global = true)]
    keys: Option<PathBuf>,

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

    /// Sync the signed event DAG with another replica through file bundles.
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },

    /// Sync with other replicas over the network (iroh QUIC). Needs the shared
    /// workspace token: RSTASK_P2P_TOKEN or `p2p.token`.
    P2p {
        #[command(subcommand)]
        command: P2pCommand,
    },
}

#[derive(Subcommand)]
enum P2pCommand {
    /// Serve this workspace to peers until interrupted; prints this node's
    /// spec (`ID@host:port`) for them to dial.
    Listen {
        /// UDP socket to bind, e.g. 0.0.0.0:7701 (default: RSTASK_P2P_BIND,
        /// `p2p.bind`, or a random port).
        #[arg(long)]
        bind: Option<std::net::SocketAddr>,
    },
    /// Remember peers to sync with.
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    /// Two-way sync with one peer (`ID` or `ID@host:port`), or with every
    /// known peer.
    Sync { peer: Option<String> },
    /// This node's id, relay mode and known peers.
    Status,
}

#[derive(Subcommand)]
enum PeerCommand {
    /// `ID` or `ID@host:port[,host:port…]`.
    Add {
        spec: String,
    },
    List,
}

#[derive(Subcommand)]
enum SyncCommand {
    /// Export the signed event DAG as a binary bundle (stdout).
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
        /// Limit the actor to these roles (repeatable); none given means all.
        #[arg(long = "role", value_enum)]
        roles: Vec<RoleArg>,
        /// The human provisioning this actor (default: the workspace root).
        /// Their key must be in the key directory.
        #[arg(long)]
        by: Option<String>,
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

#[derive(clap::ValueEnum, Clone)]
enum RoleArg {
    Planner,
    Worker,
    Reviewer,
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
        /// Make it a subtask: the parent waits until its subtasks are done.
        #[arg(long)]
        parent: Option<String>,
    },
    /// Make a task wait until another is done.
    Block {
        id: String,
        /// The task that must be done first.
        #[arg(long)]
        by: String,
        #[arg(long)]
        actor: String,
    },
    /// Withdraw a dependency added with `block`.
    Unblock {
        id: String,
        #[arg(long)]
        by: String,
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
        /// The submission you reviewed (`submission.id` in `task show`):
        /// refused if the task has been resubmitted since.
        #[arg(long)]
        submission: Option<String>,
    },
    /// From now on only a human's approval counts for this task. No way back.
    Protect {
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
    let store = Arc::new(match cli.keys.or(cfg.auth.key_dir) {
        Some(keys) => Store::open_with_keys(&db, &keys).await?,
        None => Store::open(&db).await?,
    });
    // This replica's network identity lives beside the database.
    let node_key = db.with_file_name("node.key");

    match cli.command {
        Command::Init => {
            println!("initialized {}", db.display());
        }
        Command::Actor { command } => match command {
            ActorCommand::New {
                name,
                kind,
                roles,
                by,
            } => {
                let kind = match kind {
                    KindArg::Human => ActorKind::Human,
                    KindArg::Agent => ActorKind::Agent,
                };
                let roles: Vec<Role> = roles
                    .into_iter()
                    .map(|r| match r {
                        RoleArg::Planner => Role::Planner,
                        RoleArg::Worker => Role::Worker,
                        RoleArg::Reviewer => Role::Reviewer,
                    })
                    .collect();
                let actor = store
                    .provision_actor(kind, &name, &roles, by.as_deref())
                    .await?;
                println!("{}", serde_json::to_string_pretty(&actor)?);
            }
            ActorCommand::Heartbeat { id } => {
                store.heartbeat(&id).await?;
            }
        },
        Command::Task { command } => match command {
            TaskCommand::New {
                title,
                body,
                actor,
                parent,
            } => {
                let task = match parent {
                    Some(parent) => store.create_subtask(&parent, &title, &body, &actor).await?,
                    None => store.create_task(&title, &body, &actor).await?,
                };
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::Block { id, by, actor } => {
                store.add_dependency(&id, &by, &actor).await?;
            }
            TaskCommand::Unblock { id, by, actor } => {
                store.remove_dependency(&id, &by, &actor).await?;
            }
            TaskCommand::List { status } => {
                let tasks = store.list_tasks(status.map(Into::into)).await?;
                print_task_table(&tasks);
            }
            TaskCommand::Show { id } => {
                let task = store.task_detail(&id).await?;
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
            TaskCommand::Approve {
                id,
                actor,
                submission,
            } => {
                let task = store.approve(&id, &actor, submission.as_deref()).await?;
                println!("{}", serde_json::to_string_pretty(&task)?);
            }
            TaskCommand::Protect { id, actor } => {
                let task = store.protect_task(&id, &actor).await?;
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
        Command::P2p { command } => match command {
            P2pCommand::Listen { bind } => {
                let node = start_node(&cfg.p2p, &node_key, store, bind).await?;
                let spec = serde_json::json!({
                    "node": node.id().to_string(),
                    "spec": node.peer_spec().to_string(),
                    "relay": cfg.p2p.relay,
                });
                println!("{spec}");
                stop_signal().await?;
                node.shutdown().await;
            }
            P2pCommand::Peer { command } => match command {
                PeerCommand::Add { spec } => {
                    let peer: PeerSpec = spec.parse()?;
                    store
                        .add_peer(&peer.id.to_string(), &peer.addrs_string())
                        .await?;
                }
                PeerCommand::List => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&store.list_peers().await?)?
                    );
                }
            },
            P2pCommand::Sync { peer } => {
                let peers: Vec<PeerSpec> = match peer {
                    Some(spec) => vec![spec.parse()?],
                    None => {
                        let mut known = Vec::new();
                        for p in store.list_peers().await? {
                            known.push(format!("{}@{}", p.id, p.addrs).parse()?);
                        }
                        known
                    }
                };
                anyhow::ensure!(
                    !peers.is_empty(),
                    "no peers: `rstask p2p peer add ID@HOST:PORT`"
                );
                let node = start_node(&cfg.p2p, &node_key, store, None).await?;
                let mut failed = 0;
                for peer in &peers {
                    let outcome = match node.sync_with(peer).await {
                        Ok(stats) => serde_json::json!({ "peer": peer.to_string(), "ok": stats }),
                        Err(e) => {
                            failed += 1;
                            serde_json::json!({ "peer": peer.to_string(), "error": e.to_string() })
                        }
                    };
                    println!("{outcome}");
                }
                node.shutdown().await;
                anyhow::ensure!(failed == 0, "{failed} of {} peers failed", peers.len());
            }
            P2pCommand::Status => {
                let status = serde_json::json!({
                    "node": load_or_create_node_key(&node_key)?.public().to_string(),
                    "relay": cfg.p2p.relay,
                    "token": if cfg.p2p.token.is_empty() { "unset" } else { "set" },
                    "peers": store.list_peers().await?,
                });
                println!("{}", serde_json::to_string_pretty(&status)?);
            }
        },
    }
    Ok(())
}

/// Wait for Ctrl-C or, on Unix, SIGTERM — a listener started in the
/// background by a script or a service manager never sees Ctrl-C.
async fn stop_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate())?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r?,
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

/// Bring up this replica's network node.
async fn start_node(
    p2p: &rstask_core::config::P2pConfig,
    node_key: &Path,
    store: Arc<Store>,
    bind: Option<std::net::SocketAddr>,
) -> anyhow::Result<Node> {
    let bind = match (bind, &p2p.bind) {
        (Some(addr), _) => Some(addr),
        (None, Some(addr)) => Some(addr.parse()?),
        (None, None) => None,
    };
    let cfg = NetConfig {
        relay: p2p.relay.parse()?,
        bind,
        token: p2p.token.clone(),
    };
    Ok(Node::spawn(load_or_create_node_key(node_key)?, cfg, store).await?)
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

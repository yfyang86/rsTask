//! rstask-server — local REST API (Axum).
//!
//! Read-only for now (Web dashboard): exposing mutation routes would bypass
//! the claim/workflow invariants only where they are enforced — in core —
//! so write endpoints must delegate to `Store` methods, never raw SQL.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::Json, routing::get, Router};
use rstask_core::{Config, Store, Task, TaskStatus};
use serde::Serialize;

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    version: &'static str,
}

#[derive(Serialize)]
struct BoardColumn {
    status: TaskStatus,
    tasks: Vec<Task>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Config::load(None)?;
    let store = Arc::new(Store::open(&cfg.storage.db_path).await?);
    let state = AppState { store };

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/api/tasks", get(list_tasks))
        .route("/api/tasks/{id}", get(get_task))
        .route("/api/board", get(board))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], cfg.server.port));
    tracing::info!(%addr, "rstask-server listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn list_tasks(State(st): State<AppState>) -> Result<Json<Vec<Task>>, StatusCode> {
    st.store
        .list_tasks(None)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_task(
    State(st): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Task>, StatusCode> {
    st.store.get_task(&id).await.map(Json).map_err(|e| match e {
        rstask_core::Error::TaskNotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    })
}

async fn board(State(st): State<AppState>) -> Result<Json<Vec<BoardColumn>>, StatusCode> {
    let columns = st
        .store
        .board_state()
        .await
        .map(|board| {
            board
                .into_iter()
                .map(|(status, tasks)| BoardColumn { status, tasks })
                .collect::<Vec<_>>()
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(columns))
}

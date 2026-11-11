//! rstask-mcp — MCP server exposing agent tools over the shared core.
//!
//! Stub. Planned tool surface (see spec):
//!   tasks_list, task_claim, task_update, task_complete,
//!   board_state, tree_get, tree_edit
//!
//! Tool implementations must call `rstask_core::Store` directly so that
//! claim TTL, the workflow gate, and the event log are enforced for agents
//! exactly as they are for humans. The `rmcp` dependency lands with the
//! first real tool implementation.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("rstask-mcp: stub — no tools registered yet (see crate docs)");
    Ok(())
}

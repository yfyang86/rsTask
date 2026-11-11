# RsTask — Simple Spec

**A local-first, Markdown-native task workspace for humans and AI agents.** CLI/TUI/Web for humans, MCP for agents. One core, one SQLite DB, Markdown for content.

---

## Architecture

```
CLI ─┐
TUI ─┼─► Core ─► SQLite (state, events, locks)
Web ─┤           Markdown + binaries (content)
MCP ─┘
         ▲
    Local REST (Axum) · P2P (p2panda, optional)
```

All interfaces share one core. No interface owns data.

---

## Actor Model

- **Human** and **Agent** are both actors: identity + role + heartbeat.
- **Claim = atomic lock with TTL.** No heartbeat → claim expires → task returns to pool.
- **Workflow gate:** `queue → work → review → done`. Agents can't self-approve.
- **Event log:** every claim/update/complete is immutable.
- **Humans can force-release** stale claims.

---

## Interfaces

| Interface | Purpose |
|---|---|
| **CLI** (`clap`) | Scriptable human ops |
| **TUI** (`ratatui`) | Kanban / tree / board views |
| **Web** | Read-only dashboard |
| **MCP** (`rmcp`) | Agent tools: `tasks_list`, `task_claim`, `task_update`, `task_complete`, `board_state`, `tree_get`, `tree_edit` |

---

## Task Modules (built-in)

| Module | Semantics |
|---|---|
| Sprint | Sprint membership, velocity |
| Kanban | Column position, WIP limits |
| OKR | Objective/KR links, rollup |
| Waterfall | Phases, dependencies, gates |
| Brainstorm | Tree in TOML/JSON (`tree_get`/`tree_edit`) |
| Pipeline | Custom state machine |

**Combination** = tasks are first-class; modules are orthogonal attachments (a card can be in a Sprint, on a Kanban board, and linked to an OKR).

---

## Storage

- **SQLite** = source of truth (tasks, actors, claims, events, module state).
- **Markdown** = rendered/exported content (portable, diffable). `rstask import` to reconcile manual edits.
- **Binaries** on disk, referenced by DB.

---

## Config (`config.toml`, layered: defaults → file → env → CLI)

```toml
[server]
port = 7700
cors_origins = ["http://localhost:3000"]

[auth]
mode = "offline"           # offline | oidc
key_algorithm = "ed25519"  # default; RSA only for legacy

[storage]
db_path = ".rstask/rstask.sqlite"
markdown_root = ".rstask/content"
attachment_root = ".rstask/attachments"

[p2p]
enabled = false            # off by default
transport = "quic"
```

---

## Stack

`clap` · `ratatui` · `tokio` · `sqlx` · `serde`/`toml` · `uuid` · `chrono` · `tracing` · `rmcp` · `p2panda` · `axum` · `ed25519-dalek`

---

## Deferred

- Dynamic plugin loading (WASM) — start with built-in modules.
- P2P claim-conflict strategy — design before enabling multi-node.
- Agent self-registration — start with offline provisioning.

---

**One-line pitch:** A workspace where a human and N agents claim, execute, and review tasks through a shared task graph — CLI for humans, MCP for agents, Markdown for portability.

# RsTask

Local-first, Markdown-native task workspace for humans and AI agents.
CLI/TUI/Web for humans, MCP for agents. One core, one SQLite DB, Markdown for content.

## Layout

```
crates/
  rstask-core    Models, SQLite storage, actor/claim/event semantics — the single writer
  rstask-cli     `rstask` binary (clap) — scriptable human ops
  rstask-tui     ratatui kanban view (skeleton)
  rstask-server  axum REST — read-only dashboard API (skeleton)
  rstask-mcp     MCP server (rmcp, stdio) — one process per agent
```

## Quick start

```sh
cargo build --workspace
cargo run -p rstask-cli -- init
ACTOR=$(cargo run -q -p rstask-cli -- actor new alice --kind human | jq -r .id)
TASK=$(cargo run -q -p rstask-cli -- task new "hello world" --actor "$ACTOR" | jq -r .id)
cargo run -p rstask-cli -- task claim "$TASK" --actor "$ACTOR"
cargo run -p rstask-cli -- task submit "$TASK" --actor "$ACTOR"
cargo run -p rstask-cli -- task approve "$TASK" --actor "$ACTOR"
cargo run -p rstask-cli -- board
```

## Invariants (enforced in rstask-core, regardless of interface)

- **Claim = atomic lock with TTL.** At most one active claim per task; the
  holder extends it with `task renew`; stale claims are swept on every read
  and write; humans can force-release.
- **Workflow gate:** `queue → work → review → done`. Only the claim holder
  submits (`--summary`, `--ref`, `--evidence` go to the reviewer); a rejection
  carries a `--reason`. Agents cannot self-approve.
- **Event log:** every claim/update/transition is appended immutably in the
  same transaction as the mutation.

## Agents (MCP)

Each agent runs its own server; the actor is fixed at startup, so no tool takes
an actor argument.

```sh
BOT=$(rstask actor new bot --kind agent | jq -r .id)
rstask-mcp --actor "$BOT"          # or RSTASK_ACTOR; --db / --config as the CLI
```

```json
{ "mcpServers": { "rstask": { "command": "rstask-mcp", "args": ["--db", "/path/to/rstask.sqlite", "--actor", "<actor id>"] } } }
```

Tools: `claim_next` (atomically take the next task — highest priority, then
oldest), `task_renew`, `task_complete`, `task_release`, `task_get`, `tasks_list`,
`board_state`, `task_create`, `task_update`, `task_claim`, `task_approve`,
`task_reject`. Errors come back as tool errors carrying core's message.

## Tests

```sh
cargo test --workspace
```

## Sync (sprint 01)

Every mutation is a signed (Ed25519), content-addressed (BLAKE3) event in a
Lamport-clock DAG; task state is derived from the log (see `PRD/PRD-v2.md`).

```sh
rstask --db wa.sqlite sync export > a.bundle   # signed DAG + actor directory
rstask --db wb.sqlite sync import < a.bundle   # verify → merge → converge
rstask sync status                             # heads, event count, pending
```

Concurrent claims resolve deterministically on every replica: first claim by
`(lamport, event_id)` wins, losers stay in the log but are ineffective. Wire
transport (iroh QUIC) is sprint 02.

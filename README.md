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
  rstask-mcp     MCP agent tools (stub; rmcp lands with the first tool)
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

- **Claim = atomic lock with TTL.** At most one active claim per task; stale
  claims are swept on each claim attempt; humans can force-release.
- **Workflow gate:** `queue → work → review → done`. Agents cannot self-approve.
- **Event log:** every claim/update/transition is appended immutably in the
  same transaction as the mutation.

## Tests

```sh
cargo test -p rstask-core
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

# RsTasks

Local-first, Markdown-native task workspace for humans and AI agents.
CLI/TUI/Web for humans, MCP for agents. One core, one SQLite DB, Markdown for content.

## Layout

```
crates/
  rstask-core    `rstasks-core` package: models, SQLite storage, actor/claim/event semantics
  rstask-cli     `rstasks` package and binary (clap) — scriptable human ops
  rstask-tui     `rstasks-tui` ratatui kanban view (skeleton)
  rstask-server  `rstasks-server` axum REST — read-only dashboard API (skeleton)
  rstask-mcp     `rstasks-mcp` MCP server (rmcp, stdio) — one process per agent
```

## Install from crates.io

```sh
cargo install rstasks
cargo install rstasks-mcp rstasks-server rstasks-tui
```

The `RSTASK_*` environment variables and `.rstask` workspace directory retain
their existing names for compatibility.

## Quick start

```sh
cargo build --workspace
cargo run -p rstasks -- init
ACTOR=$(cargo run -q -p rstasks -- actor new alice --kind human | jq -r .id)
TASK=$(cargo run -q -p rstasks -- task new "hello world" --actor "$ACTOR" | jq -r .id)
cargo run -p rstasks -- task claim "$TASK" --actor "$ACTOR"
cargo run -p rstasks -- task submit "$TASK" --actor "$ACTOR"
cargo run -p rstasks -- task approve "$TASK" --actor "$ACTOR"
cargo run -p rstasks -- board

## Another example with tui or API
export DB=.rstask/typecraft.sqlite && RSTASK_STORAGE_DB_PATH="$DB" rstasks-server
curl -s http://127.0.0.1:7700/api/board | jq
```

## Invariants (enforced in rstasks-core, regardless of interface)

- **Claim = atomic lock with TTL.** At most one active claim per task; the
  holder extends it with `task renew`; stale claims are swept on every read
  and write; humans can force-release.
- **Workflow gate:** `queue → work → review → done`. Only the claim holder
  submits (`--summary`, `--ref`, `--evidence` go to the reviewer); a rejection
  carries a `--reason`. Agents cannot self-approve.
- **Dependencies:** `task block A --by B` and `task new --parent P` — a task is
  claimable only when its blockers and subtasks are `done`; cycles are refused.
- **Roles:** `actor new --role planner|worker|reviewer` (repeatable) limits an
  actor to planning, claiming or reviewing; without `--role` it holds all three.
- **Keys and identity:** the first actor is the workspace root and must be
  human; every other actor is provisioned by a human (`actor new --by`).
  Secret keys live in a key directory (`--keys`, default `keys/` beside the
  database), never in the database: no key file, no signing.
- **Review:** `task approve --submission <id>` binds the approval to the
  submission that was read; `task protect` makes a task human-approval-only.
- **Authority on sync:** the same rules are applied to every merged event; a
  bundle from a workspace with a different root is refused.
- **Event log:** every claim/update/transition is appended immutably in the
  same transaction as the mutation.

## Agents (MCP)

Each agent runs its own server; the actor is fixed at startup, so no tool takes
an actor argument.

```sh
BOT=$(rstasks actor new bot --kind agent | jq -r .id)   # signed by the root human
mkdir -p ~/agents/bot && mv .rstask/keys/$BOT.key ~/agents/bot/
rstasks-mcp --actor "$BOT" --keys ~/agents/bot   # or RSTASK_ACTOR / RSTASK_AUTH_KEY_DIR
```

```json
{ "mcpServers": { "rstasks": { "command": "rstasks-mcp", "args": ["--db", "/path/to/rstask.sqlite", "--keys", "/path/to/agents/bot", "--actor", "<actor id>"] } } }
```

Tools: `claim_next` (atomically take the next task — highest priority, then
oldest), `task_renew`, `task_complete`, `task_release`, `task_get`, `tasks_list`,
`board_state`, `task_create`, `task_update`, `task_block`, `task_unblock`,
`task_protect`, `task_claim`, `task_approve` (needs `submission`), `task_reject`. Errors come back as tool errors
carrying core's message.

## Tests

```sh
cargo test --workspace
```

## Sync (sprint 01)

Every mutation is a signed (Ed25519), content-addressed (BLAKE3) event in a
Lamport-clock DAG; task state is derived from the log (see `PRD/PRD-v2.md`).

```sh
rstasks --db wa.sqlite sync export > a.bundle   # signed DAG (actors are events in it)
rstasks --db wb.sqlite sync import < a.bundle   # verify → merge → converge
rstasks sync status                             # heads, event count, pending
```

Concurrent claims resolve deterministically on every replica: first claim by
`(lamport, event_id)` wins, losers stay in the log but are ineffective.

## Sync over the network (sprint 02)

Replicas reconcile directly over iroh QUIC: bucket fingerprints → id diff →
only the missing events, both ways. Peers must share the workspace token; what
a received event may do is still decided by its signature and the merge rules.

```sh
export RSTASK_P2P_TOKEN=…                    # shared secret; or p2p.token
rstasks p2p listen --bind 0.0.0.0:7701        # host A: prints {"spec": "ID@host:port", …}
rstasks p2p sync ID@a.example:7701            # host B: two-way sync, remembers the peer
rstasks p2p sync                              # every known peer
rstasks p2p peer add ID@HOST:PORT; rstasks p2p peer list; rstasks p2p status
rstasks p2p sync --every 30                   # keep syncing; or: p2p listen --sync-every 30
```

`RSTASK_P2P_RELAY` / `p2p.relay`: `default` (n0's public relays and DNS, dial by
id alone), `disabled` (direct addresses only — LAN, tests), or your own relay
URL. The node's network identity is `node.key` beside the database. A sync reports any
claim it took away from this replica (`lost_claims`).

To add an actor on another machine without moving a secret key:

```sh
rstasks actor request bob --kind agent --role worker   # there: prints a request, keeps the key
rstasks actor admit '<request>'                        # here, as a human: signs bob's creation
```

After the next sync the new machine can act as `bob`.

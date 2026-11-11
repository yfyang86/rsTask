# Sprint 05 — Report

## Shipped
- **`rstask-mcp`** — a real MCP server (`rmcp 3.9`, stdio). One process per agent; the
  actor is fixed at startup and checked before serving, and no tool takes an actor.
  Tools: `claim_next`, `task_claim`, `task_renew`, `task_release`, `task_complete`,
  `task_get`, `tasks_list`, `board_state`, `task_create`, `task_update`,
  `task_approve`, `task_reject`. Success is JSON text; a core error is a tool error
  with core's message. The server instructions tell the agent who it is and the two loops.
- **`claim_next`** in core — sweep, pick (priority, then age), claim, in one write
  transaction.
- **`task_detail`** — task + live claim + latest submission + latest rejection; also
  what `rstask task show` now prints (sprint-04 debt closed).
- **`update_task`** refuses an unknown task and, while claimed, a non-holder (A7 part).

## What the tests caught (the loop worked)
1. **`database is locked` (A8).** The first hand-driven probe sent `claim_next` and
   `task_approve` back to back; the approve came back `database is locked`. A deferred
   SQLite transaction that reads before it writes is refused outright when another
   writer commits in between — no waiting. Reproduced as a red core test (reviewers
   approving while workers claim, 3/3 runs failing), fixed by opening every write
   transaction with `BEGIN IMMEDIATE`. This predates the sprint and affected the CLI
   too; the earlier 20-process claim race missed it because claims write first.
2. A non-holder's `task_release` answered "already claimed by *<the caller>*". Release
   now uses the same claim check as submit and renew and names the real holder.
   Releasing a task nobody holds is now `InvalidTransition`, not `ClaimConflict`
   (one existing test updated to say so).

## Decisions recorded
- One server per agent with a fixed actor, rather than an `actor` argument per call:
  an agent cannot act as another through this interface.
- Four tools beyond the PRD-v1 list (`task_create`, `task_release`, `task_approve`,
  `task_reject`): without them agents could neither plan nor review, and the loop
  could not close over MCP. PRD-v3 M3 updated.
- `task_complete` requires a `summary`; `refs` and `evidence` are optional.
- `claim_next` returns `null` on an empty queue (not an error) — agents poll.
- Unclaimed tasks are editable by any actor (planning); a claimed task only by its holder.

## Slipped / debt
- Agents poll; there is no push when a task appears or a claim is lost (F3).
- Tool results are JSON text, not `structuredContent` with an output schema.
- `task_create` cannot set a priority (use `task_update`).
- Backlog A7 remainder: `UNIQUE` claim index, concurrent `task.updated` ordering.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 40/40 green, three consecutive runs
  (7 lib + 20 store + 11 sync + 2 MCP stdio).
- `./uat.sh`: U1–U4 pass against real server processes; transcripts in
  `uat-evidence/`. Sprint-03 and sprint-04 `uat.sh` re-run: still pass.

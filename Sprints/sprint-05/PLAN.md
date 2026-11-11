# Sprint 05 — MCP server with `claim_next` (plan)

> PRD refs: §Multi-agent M3 · §Audit A7 (part), A8 · PRD: `PRD/PRD-v3.md` · Depends on: sprint-04

## Goal (one sentence)

An agent with nothing but an MCP client can take the next task, keep it, hand in its
result and review others' work — under the same gate as the CLI, and without racing
other agents.

## Scope

**In:**
* Core: `claim_next` (highest priority, then oldest, one transaction); `task_detail`
  (task + live claim + latest submission + latest rejection); `update_task` refuses an
  unknown task and, while the task is claimed, anyone but the holder.
* `rstask-mcp`: `rmcp 3.9` over stdio, one process per agent, actor fixed at startup
  (`--actor` / `RSTASK_ACTOR`), verified before serving. Twelve tools (PRD-v3 M3);
  none takes an actor. Core errors come back as tool errors (`isError: true`).
* CLI: `task show` prints the detail (task fields stay top-level).

**Out (explicitly):** HTTP transport; MCP resources/prompts; `tree_get`/`tree_edit`
(brainstorm module, PRD-v1); dependencies and roles in `claim_next` eligibility
(sprint 06); authenticating that the process starting the server *is* that actor
(sprint 07, A6); notifying an agent that its claim lost a merge (F3).

---

## Loop

### 1. Investigate
- [x] `rmcp` on crates.io is 3.9.0; API read from its source and tests
  (`#[tool_router]`, `#[tool_handler]`, `Parameters<T>`, `ServerConfig`,
  `serve(stdio())`), not from memory.
- [x] `claim_task` already swept and locked first — reusable as `claim_tx`.
- [x] `update_task` wrote an event for a non-existent task and ignored claims.

### 2. Setup UAT → `UAT.md`
Scenarios U1–U4 written before development.

### 3. Development
- [x] Core tests first, then `claim_next`, `task_detail`, `update_task` checks.
- [x] `rstask-mcp/src/main.rs`; `tests/stdio.rs` drives the real binary.

### 4. Test
- [x] `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [x] `./uat.sh` → `uat-evidence/` (8 server processes race for 3 tasks)

### 5. Fix / investigate
- [x] First manual probe of the server returned `database is locked` (see REPORT) →
  red core test → `BEGIN IMMEDIATE` for every write transaction.
- [x] `task_release` by a non-holder named the *caller* as the holder → fixed.

### 6. Plan next
- [x] REPORT.md; debt carried to sprint 06 / 07.

---

## Deliverables
- `crates/rstask-mcp` (server + stdio test); core `claim_next`, `task_detail`; this sprint's docs

## Definition of Done
- [x] Gate green (fmt, clippy `-D warnings`, tests)
- [x] 8 agents racing `claim_next` for 3 tasks: 3 distinct winners, 5 `null`, queue empty
- [x] Worker → reviewer → rework → approve completes over MCP only; every event signed by the acting agent
- [x] Non-holder update/complete/release and self-approval are tool errors with core's message
- [x] Unknown actor: the server exits non-zero with nothing on stdout

## Risks touched
- R8 — `rmcp` moves fast (3.x); pinned to `3.9` in the crate manifest.
- R9 — The server trusts whoever starts it with `--actor` (PRD-v3 A6): a boundary
  against confusion, not against a hostile process. Sprint 07.

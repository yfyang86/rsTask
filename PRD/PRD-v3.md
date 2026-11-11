# RsTask — Spec v3 (multi-agent work and RSI)

**Supersedes:** PRD-v2 §Sprints ordering (the iroh transport moves behind the agent
layer). Everything else in v1 and v2 stands.

v3 makes the workspace fit for N agents working one task graph, and for *RSI* — read
here as recursive self-improvement: agents planning, executing and reviewing changes
to their own system through rsTask. The signed, immutable event log is the right base
for an auditable loop; the loop is only as trustworthy as the review gate, so the gate
comes first.

---

## Audit (2026-11-11, against `cffa84b`)

Confirmed by probing the CLI on two replicas unless marked *(read)*.

| # | Finding | Status |
|---|---------|--------|
| A1 | Replicas diverge after reject → re-claim (origin `work`, importer `review`): the fold kept a sticky `submitted` flag. Concurrent approve vs reject also diverged — the status guards depended on arrival order. | fixed, sprint 03 |
| A2 | Self-approval not blocked (walkthrough F2): `approve()` checked neither actor kind nor the submitter. | fixed, sprint 03 |
| A3 | A human could not force-release another actor's claim: `release_claim` only looked up the caller's own claim. | fixed, sprint 03 |
| A4 | Claims cannot be kept alive: `renew_claim` has no CLI command and writes no event; `actor heartbeat` updates a column nothing reads. *(read)* | fixed, sprint 04 (the heartbeat column stays unused) |
| A5 | Merge checks signatures, not authority: any known actor's event is applied (release another's claim by naming its id, approve anything). *(read)* | sprint 07 |
| A6 | Identity is honour-system: `--actor` is a bare id, secret keys are plaintext in the shared DB, any process can run `actor new --kind human`. *(read)* | sprint 07 |
| A7 | Smaller: `update_task` has no claim/existence check and no CLI command; the active-claim index is not `UNIQUE`; claim expiry is judged by the wall clock at merge time; concurrent `task.updated` is last-arrival-wins. *(read)* | `update_task` checks: sprint 05; expiry: sprint 04; rest backlog |
| A8 | Found in sprint 05: write transactions that read before writing were refused with `database is locked` whenever another writer committed in between (reviewers approving while workers claim). | fixed, sprint 05 (`BEGIN IMMEDIATE`) |

What already holds: 20 processes racing for one claim on a shared DB gave exactly one
winner and 19 clean `ClaimConflict` errors.

## Gate rules

| # | Rule | Enforced |
|---|------|----------|
| G1 | Task status is **derived by one fold** over the task's workflow events in `(lamport, event_id)` order. An event that does not fit the status reached so far (a claim on a non-`queue` task, a verdict on a non-`review` task) stays in the log but is ineffective. The deciding event's `at` stamps `updated_at`. | merge (sprint 03) |
| G2 | **An agent cannot approve its own submission** — the approver must differ from the actor of the task's latest `task.submitted`. Humans may approve their own work; an agent may review another agent's. | local write (sprint 03); merge (sprint 07) |
| G3 | **Force-release is human-only** and releases whoever holds the task. A plain release frees only the caller's own claim. | local write (sprint 03); merge (sprint 07) |

## Multi-agent requirements

| # | Requirement |
|---|-------------|
| M1 | `task.renewed` as a signed event; `rstask task renew`; sweep stale claims on read so the board never shows dead `work`. A claim is live in `[claimed_at, expires_at)`, judged against event times, not the merging replica's clock; a renewal the holder made before the claim lapsed always counts, wherever it sorts in the log. |
| M2 | Results on submit: `task.submitted` carries `summary`, `refs` (commit/PR/path) and `evidence`; `task.changes_requested` carries a `reason`. Only the holder of a live claim submits. |
| M3 | MCP server (`rmcp`, stdio) over `Store`, one process per agent with the actor fixed at startup — no tool takes an actor. Tools: `tasks_list`, `task_get` (with the holder, the latest submission and the latest rejection reason), `board_state`, `task_create`, `task_claim`, `task_renew`, `task_release`, `task_update`, `task_complete`, `task_approve`, `task_reject`, plus **`claim_next`** — atomically take the highest-priority eligible task, so agents do not list-then-race. |
| M4 | Dependencies and subtasks: `blocked_by` and `parent` links; a task is eligible only when its blockers are `done`. |
| M5 | Roles (planner / worker / reviewer) on actors, used for routing and for who may review. |

## RSI preconditions

Agents must not review each other unattended until these hold.

| # | Precondition |
|---|--------------|
| R1 | Keys leave the workspace DB: each agent holds only its own key; humans' keys are out of agents' reach. |
| R2 | Authority is enforced in the fold (G2, G3, claim ownership on release), not only on the local write path. |
| R3 | Actor provisioning is signed by a human root key instead of trust-on-first-use. |
| R4 | Evidence-bound approval: `task.approved` references the hash of the submitted evidence (M2). |
| R5 | A protected task class — changes to the gate, the keys or the fold rules — always needs human sign-off. |

## Sprints

| Sprint | Theme | Status |
|-------:|-------|--------|
| 01 | Event DAG + offline reconciliation | done |
| 02 | iroh transport | planned, **deferred** behind 07 — one shared SQLite file already serves many agents on one host |
| 03 | Gate correctness: A1–A3 / G1–G3 | done |
| 04 | Claim liveness and handoff: M1, M2 | done |
| 05 | MCP server with `claim_next`: M3 | done |
| 06 | Dependencies and roles: M4, M5 | planned |
| 07 | Keys and authority: R1–R5, A5, A6 | planned |

## Deferred (unchanged from v2)

Encrypted keystore beyond R1, blob transfer, relay hosting / daemon mode, WASM plugins.

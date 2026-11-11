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
| A5 | Merge checks signatures, not authority: any known actor's event is applied (release another's claim by naming its id, approve anything). *(read)* | fixed, sprint 07 |
| A6 | Identity is honour-system: `--actor` is a bare id, secret keys are plaintext in the shared DB, any process can run `actor new --kind human`. *(read)* | fixed, sprint 07 (see Trust boundary) |
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
| M4 | Dependencies and subtasks. `task.blocked` / `task.unblocked` events declare that a task waits for another; a subtask names its `parent` at creation. A task is claimable only when every blocker and every subtask is `done`; `claim_next` skips the rest. A dependency that would close a cycle (through blockers or subtasks) is refused. The dependency set is folded per task in log order, so concurrent edits converge. |
| M5 | Roles on actors — `planner` (create tasks, edit dependencies and unclaimed tasks), `worker` (claim), `reviewer` (approve / reject) — fixed at creation and carried in the actor directory. An actor created without roles holds all three. The holder of a claim may always edit and submit its own task. |

## RSI preconditions

Agents must not review each other unattended until these hold.

| # | Precondition |
|---|--------------|
| R1 | Keys leave the workspace DB. Secret keys live in a key directory, one `<actor id>.key` each (`--keys`, `RSTASK_AUTH_KEY_DIR`, `auth.key_dir`; default `keys/` beside the database); the database holds public keys only. An agent started with a directory holding only its own key cannot sign as anyone else. |
| R2 | Authority is enforced on merge, not only on the local write path. Refused before entering the log: an event from an unknown actor or with the wrong key, a Lamport clock that does not follow its parents, a kind its author's role or kind does not allow (planner / worker / reviewer; force-release and provisioning: humans). Ineffective in the fold: release, submit or edit by anyone but the holder; an agent approving its own submission (G2); an approval that names another submission (R4); an agent's approval of a protected task (R5). Not enforced: a claim on a task that a dependency blocks — a replica that had not heard of the dependency made an honest concurrent edit. |
| R3 | Actors are created by signed `actor.created` events. The first event of a workspace is its root: a self-signed human. Every other actor is provisioned by a human (default: the root). A replica has one root; a bundle built on another root is refused whole. The actor directory no longer travels beside the events. |
| R4 | Evidence-bound approval: `task.approved` names the `task.submitted` event it approves — an id that hashes the summary, refs and evidence. It counts only while that submission is the one in review; locally a reviewer may pass the id it read and is refused if the task was resubmitted since (MCP requires it). |
| R5 | Protected tasks: `task.protected` (a planner, irreversible) marks a task so that only a human's approval counts. Marking is a planning decision — nothing detects which tasks touch the gate. |

## Trust boundary (after sprint 07)

What holds: a replica accepts only events signed by actors its root's chain of humans
provisioned, each acting within its kind and roles, and derives every task's state
from those events — so no actor can approve its own work, act on another's claim or
mint an identity on *someone else's* replica.

What does not: a process that can write a replica's SQLite file can corrupt *that*
replica's derived tables (other replicas are unaffected and will not accept the
result). Key files are plaintext on disk (`0600`). There is no key rotation or
revocation; losing the root key ends provisioning. Agents that must not trust each
other need their own replica and key directory, not just their own MCP server.

## Sprints

| Sprint | Theme | Status |
|-------:|-------|--------|
| 01 | Event DAG + offline reconciliation | done |
| 02 | iroh transport | planned, **deferred** behind 07 — one shared SQLite file already serves many agents on one host |
| 03 | Gate correctness: A1–A3 / G1–G3 | done |
| 04 | Claim liveness and handoff: M1, M2 | done |
| 05 | MCP server with `claim_next`: M3 | done |
| 06 | Dependencies and roles: M4, M5 | done |
| 07 | Keys and authority: R1–R5, A5, A6 | done |

## Deferred (unchanged from v2)

Encrypted keystore beyond R1, blob transfer, relay hosting / daemon mode, WASM plugins.

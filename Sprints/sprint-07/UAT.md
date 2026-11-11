# Sprint 07 — UAT

Written before development. Evidence → `uat-evidence/` (regenerate with `./uat.sh`).
Forged events cannot be produced through the CLI or MCP — that is the point — so the
forgery scenarios (U3) are evidenced by the automated tests in
`crates/rstask-core/tests/authority.rs`, which hand-sign events with a stolen-for-the-test key.

## U1 — Keys live outside the workspace database
**Given** a fresh workspace with human `alice` (root) and agent `bot`.
**When** the database file is searched for either secret key; `bot`'s key file is
moved to a directory of its own; `rstask-mcp` runs with `--keys` pointing there, once
as `bot` and once as `alice`.
**Then** the database holds no secret; as `bot` the server works; as `alice` every
write is refused (`no signing key`) — the agent cannot act as the human. A database
created by the sprint-06 binary has its keys moved out on first open and keeps working.
Evidence: the search result, the two tool results, the migrated-workspace check.

## U2 — Actors are provisioned by a human, starting from one root
**When** the first actor of a workspace is an agent; `bot` runs `actor new` with
`--by bot`; a process holding only `bot`'s key runs `actor new`; `alice` runs it.
**Then** the first three are refused (root must be human / humans only / no signing
key); `alice`'s succeeds and the new actor's creation is an event signed by `alice`.
A bundle from a workspace with a different root is rejected whole.
Evidence: the three errors, the creation event, the import stats.

## U3 — Authority holds on merge (automated)
**Given** an agent's real key and hand-signed events fed to another replica.
**Then** each of these changes nothing: an agent approving its own submission; an
actor releasing or submitting on someone else's claim; a non-reviewer's verdict; an
agent's force-release; an agent creating an actor; an event with a rewound Lamport
clock. Concurrent edits to one task's title converge to the same value everywhere.
Evidence: `cargo test -p rstask-core --test authority`.

## U4 — An approval is bound to the submission that was reviewed
**Given** replicas A and B share a task in `review` (submission S1).
**When** on B a reviewer approves; meanwhile on A another reviewer rejects, the worker
re-claims and resubmits (S2); then sync both ways. Separately: on one replica, approve
with `--submission` naming a stale id.
**Then** after sync both replicas show `review` — the approval of S1 does not approve
S2; the stale `--submission` is refused locally; approving S2 by id lands `done`.
Evidence: status on both sides before/after, the refusal, the `task.approved` payload.

## U5 — Protected tasks need a human
**Given** a task marked `task protect`, submitted by `bot`; agent `reviewer` holds the
reviewer role.
**When** `reviewer` approves over MCP; then `alice` approves.
**Then** the agent is refused (`humans only`); the human's approval lands `done`;
protection cannot be removed.
Evidence: the tool error, final status, `task show` field.

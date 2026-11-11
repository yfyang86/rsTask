# Sprint 03 — UAT

Written before development. Evidence → `uat-evidence/` (regenerate with `./uat.sh`).

## U1 — Reject → re-claim replicates
**Given** A: human `alice`, agents `bot`, `bot2`; a task claimed and submitted by `bot`.
**When** `alice` rejects it, `bot2` re-claims it, and fresh replica B imports A's bundle.
**Then** A and B both show the task in `work` with `bot2` holding it.
Evidence: `task show` status on both sides.

## U2 — Disconnected reviewers converge
**Given** A and B share a task in `review`; `alice` is on A, human `carol` on B.
**When** `alice` approves on A and `carol` rejects on B before syncing; then sync both ways.
**Then** both replicas show the same status (`done` or `queue` — hash-determined) and
both verdict events are in the log.
Evidence: status + event kinds on both sides.

## U3 — No agent self-approval
**Given** a task claimed and submitted by agent `bot`.
**When** `bot` approves it.
**Then** refused (`not permitted to approve its own submission`), status stays `review`,
no `task.approved` event. A different agent's approval then lands it in `done`.
Evidence: CLI error, status, log kinds.

## U4 — Human force-release
**Given** agent `bot` holds a live (unexpired) claim.
**When** agent `bot2` runs `task release --force`; then `alice` does.
**Then** `bot2` is refused; `alice` succeeds: task back in `queue`, a
`claim.force_released` event signed by `alice` naming the claim, replicated to B.
Evidence: CLI error, status on both sides, the event.

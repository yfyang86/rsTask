# Sprint 01 — UAT

Written before development. Evidence → `uat-evidence/`.

## U1 — One-way export/import
**Given** workspace A with actor `alice`, one created task, a claim + release.
**When** `rstask sync export > a.bundle` in A; `rstask sync import < a.bundle` in fresh B.
**Then** B's `task list`, `task log`, `board` match A exactly (statuses, claims, event kinds).
Evidence: transcripts of both sides' `task list` + `task log` after import.

## U2 — Two-way convergence
**Given** A and B sharing a common history (U1 done both ways), then A approves the task
while B creates a second task — fully divergent heads.
**When** one export/import exchange in each direction.
**Then** both sides: identical task lists, identical heads (`sync status`), a second
exchange moves nothing.
Evidence: `sync status` + `task list` on both sides; second-exchange diff is empty.

## U3 — Concurrent claim, same winner everywhere
**Given** A and B both hold the same task in `queue`.
**When** A claims it (actor X) and B claims it (actor Y) *before* syncing; then sync both ways.
**Then** both sides show the task `work` with the same single active claim; the losing
claim event is in the log but marked ineffective; no error interrupts sync.
Evidence: `task log` (both claim events present) + `claims` view on both sides.

## U4 — Out-of-order / shuffled delivery
**Given** A with a 5+ event chain (task → claim → submit → approve).
**When** B imports the bundle with events shuffled into reverse order.
**Then** B converges anyway: pending events park and drain; final state equals A's.
Evidence: import succeeds; `task list`/`heads` equal A's; `pending_events` empty.

## U5 — Tampered bundle rejected
**Given** A's exported bundle; flip one byte in one event's payload.
**When** B imports it.
**Then** the tampered event is rejected (signature/id mismatch), reported in the merge
stats; no partial state applied for it.
Evidence: CLI stderr/report output naming the bad event.

## Commands under test
```sh
rstask sync export            # writes postcard SyncBundle to stdout
rstask sync import < FILE     # applies a bundle, prints merge stats (added/rejected/locked&pending)
rstask sync status            # heads (hex), event count, pending count
```

# Sprint 03 — Report

## Shipped
- **G1 — one status fold.** `refold_task_tx` now folds the whole workflow
  (`claimed / released / force_released / submitted / approved / changes_requested`)
  in `(lamport, event_id)` order and writes the resulting status; the verdict events
  no longer use arrival-order status guards. The deciding event's `at` stamps
  `updated_at`, so replicas agree on the timestamp too.
- **G2 — no agent self-approval.** `approve()` refuses an agent that authored the
  task's latest `task.submitted` (`Error::NotPermitted`); nothing is logged.
- **G3 — human force-release.** `release_claim(.., forced = true)` requires a human
  and releases whoever holds the task; plain release is own-claim-only as before.
- `PRD/PRD-v3.md`: audit, gate rules, multi-agent requirements, RSI preconditions,
  re-sequenced sprints.

## What the tests caught (the loop worked)
1. Writing U2 before the fix showed a second divergence the audit had not probed:
   disconnected approve vs reject left A `done` and B `queue` after a full sync.
   A flag reset would have fixed A1 only; the full fold fixes both.
2. `force_release_frees_stale_claim` had been passing the agent's own id as the
   forcing actor — the test encoded the bug. Corrected to the human.
3. A claim event ordered after a submission used to activate a phantom claim row on
   a `review` task; the fold now requires `queue`.

## Decisions recorded
- G2 is literal to PRD-v1 ("agents can't self-approve"): humans may approve their own
  work (README quick start unchanged); an agent may review another agent's work.
- G3 does not require the claim to be expired — expired claims are swept anyway; force
  exists for the hung agent with a long TTL.
- Concurrent verdicts: first in `(lamport, event_id)` wins, hash-determined on equal
  lamports; tests and UAT assert agreement, not which verdict.

## Slipped / debt
- G2/G3 are enforced on the local write path only; a hand-built signed event still
  merges (PRD-v3 A5 → sprint 07).
- Expiry is still judged by the merging replica's wall clock; submitting on an expired,
  unswept claim can still diverge (→ sprint 04, M1).
- `walkthrough/uat/run.sh` step S4p ("bob approves his own work") now records a
  refusal when re-run; its archived evidence shows the old behaviour and was left as is.
- Backlog A7 untouched.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 27/27 green (7 lib + 11 store + 9 sync).
  The six sprint-03 tests were red before the fix.
- `./uat.sh`: U1–U4 pass; transcripts in `uat-evidence/`.

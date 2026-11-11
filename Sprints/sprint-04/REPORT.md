# Sprint 04 — Report

## Shipped
- **Renewal as a signed event** — `renew_claim` / `rstask task renew` write
  `task.renewed`; the fold moves the claim's expiry on every replica.
- **Deterministic expiry** — a claim is live in `[claimed_at, expires_at)`. The fold
  expires the holder against each event's own `at`; the local clock only decides
  whether the last holder has lapsed by now. Each write takes one timestamp for its
  sweep and its event, so local state and the fold cannot disagree.
- **Sweep on read** — `get_task`, `list_tasks`, `board_state` return stale tasks to
  `queue` first; rows are stamped with the expiry instant (same value the fold derives).
- **Holder-only submit** — submit and renew need the caller's own live claim.
- **Handoff payloads** — `Submission {summary, refs, evidence}` on `task.submitted`;
  `reason` on `task.changes_requested`; CLI flags to match.
- The fold now derives `updated_at` (latest of status and content events) and
  materializes the claims table completely: the winner's row is revived if it was
  swept before its renewal arrived, and every non-holder row is released.

## What the tests caught (the loop worked)
1. The first fold applied renewals in `(lamport, event_id)` order. A renewal on A and
   a good-faith takeover on B carry the same lamport, so a hash decided whether the
   renewing holder kept the task — replicas agreed, but on a coin flip. Fixed: a
   time-ordered pre-pass computes each claim's final expiry; a renewal made before the
   claim lapsed always counts. Stable over 20 runs.
2. A replica that had swept the holder's claim before learning of its renewal kept the
   row released after the merge; the fold now revives it.
3. Boards differed in `updated_at` after the same scenario (each side stamped its own
   claim time); hence the derived timestamp.

## Decisions recorded
- Expiry is strict: at `expires_at` the claim is gone, even if nobody else wants the
  task. A late renew or submit is refused with `ClaimExpired` and logs nothing.
- In the renewal race the rival loses work it started in good faith; it learns this
  only from the board/log (walkthrough F3, still open).
- `summary`, `refs`, `evidence` are optional (the README quick start still works);
  `--reason` on reject is required at the CLI. Core stores what it is given.
- The renewal pre-pass checks the renewing actor is the claimant — the one authority
  check on merge so far; the rest waits for sprint 07.

## Slipped / debt
- `task show` does not surface the latest submission or reason; agents read
  `task log` until MCP `task_get` (sprint 05).
- `actors.last_heartbeat_at` / `actor heartbeat` remain unused.
- SQLite compares timestamps as text; a timestamp with exactly zero sub-second digits
  would sort wrongly against one with a fraction. Pre-existing, improbable, backlog.
- Backlog A7 untouched.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 33/33 green (7 lib + 15 store + 11 sync).
  The two sync tests were red before the fix; the four store tests needed the new API.
- `./uat.sh`: U1–U4 pass; transcripts in `uat-evidence/`. Sprint-03 `./uat.sh` re-run
  after the CLI change: still passes.

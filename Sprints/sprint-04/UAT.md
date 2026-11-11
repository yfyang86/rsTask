# Sprint 04 — UAT

Written before development. Evidence → `uat-evidence/` (regenerate with `./uat.sh`).

## U1 — Renewal keeps the claim, everywhere
**Given** A: agent `bot` holds a task with a 2 s TTL; replica B has synced the claim.
**When** `bot` runs `task renew` (TTL 900 s) on A; the original 2 s pass; agent `rival`
on B — who has not seen the renewal — claims the task; then sync both ways.
**Then** both replicas show `bot` holding the task in `work`; `rival`'s claim event is
in the log but ineffective; a `task.renewed` event names the claim.
Evidence: status + holder on both sides, log kinds.

## U2 — A stale claim leaves the board on read
**Given** `bot` holds a task with a 1 s TTL and never renews.
**When** 1 s passes and anyone runs `board` / `task show` (no claim attempt).
**Then** the task shows in `queue`; `bot`'s late `task renew` and `task submit` are both
refused with `claim expired`; nothing is logged for them.
Evidence: board line, the two CLI errors, log kinds.

## U3 — Only the holder submits
**Given** `bot` holds a live claim.
**When** agent `rival` runs `task submit` on it.
**Then** refused (`already claimed by bot`); status stays `work`; no event.
Evidence: CLI error, status.

## U4 — Results travel with the submission, reasons with the rejection
**Given** `bot` holds a task.
**When** `bot` submits with `--summary`, two `--ref`s and `--evidence`; `alice` rejects
with `--reason`; B imports.
**Then** B's log carries the summary, refs and evidence on `task.submitted` and the
reason on `task.changes_requested`; `task reject` without `--reason` is a usage error.
Evidence: the two payloads as read on B; the usage error.

# Sprint 04 — Claim liveness and handoff (plan)

> PRD refs: §Audit A4 · §Multi-agent M1 M2 · PRD: `PRD/PRD-v3.md` · Depends on: sprint-03

## Goal (one sentence)

An agent can hold a task for as long as it keeps renewing, loses it everywhere the
moment it stops, and hands its result — or receives the reason for rework — through
the log.

## Scope

**In:**
* M1 — `renew_claim` writes a signed `task.renewed {claim_id, expires_at}`; CLI
  `task renew`. A claim is live in `[claimed_at, expires_at)`: the fold judges expiry
  against each event's `at`; a holder's renewal made before the claim lapsed always
  counts. Reads (`get_task`, `list_tasks`, `board_state`) and every claim-touching
  write sweep stale claims first, stamping rows with the expiry instant.
* M2 — `submit_for_review(.., &Submission {summary, refs, evidence})`;
  `request_changes(.., reason)`; CLI `task submit --summary --ref… --evidence`,
  `task reject --reason` (required).
* Submit and renew require the caller's own live claim (`ClaimExpired` /
  `ClaimConflict` / `InvalidTransition` say why not).

**Out (explicitly):** surfacing the latest submission/reason in `task show` (rides
with MCP `task_get`, sprint 05); warning a replica that its claim lost a merge
(walkthrough F3); the unused `actors.last_heartbeat_at`; authority on merge beyond the
renewal-holder check (sprint 07); backlog A7.

---

## Loop

### 1. Investigate
- [x] `renew_claim`: no event, no CLI. Fold: expiry compared with the merging replica's
  `now`, so a renewed claim looked expired elsewhere.
- [x] `submit_for_review` never checked the claim: any actor could submit a held task,
  and that event (no matching `claim_id`) was ineffective on other replicas.
- [x] Sweep ran only inside `claim_task`; reads showed dead `work`.

### 2. Setup UAT → `UAT.md`
Scenarios U1–U4 written before development.

### 3. Development
- [x] Failing tests first: 4 in `tests/store_test.rs`, 2 in `tests/sync.rs`.
- [x] `sweep_expired_tx`, `require_claim_tx`, `renew_claim`, `submit_for_review`,
  `request_changes`, fold (`Fold` / `Held`), `Submission` model, CLI.

### 4. Test
- [x] `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [x] `./uat.sh` → `uat-evidence/`

### 5. Fix / investigate
- [x] First fold version failed U1 on a hash tie-break (see REPORT); renewals moved to
  a time-ordered pre-pass.

### 6. Plan next
- [x] REPORT.md; debt carried to sprint 05 / 07.

---

## Deliverables
- `store.rs`, `models.rs`, CLI `task renew|submit|reject`; tests; this sprint's docs

## Definition of Done
- [x] Gate green (fmt, clippy `-D warnings`, tests)
- [x] A renewed claim holds on the origin, on a replica that raced it, and on a fresh importer
- [x] An unrenewed claim leaves `work` on a plain read; its late renew/submit are refused, unlogged
- [x] A non-holder's submit is refused
- [x] Summary, refs, evidence and reject reason are readable from another replica's log

## Risks touched
- R1 — API break: `submit_for_review`, `request_changes`, `renew_claim` signatures;
  `task reject` now needs `--reason`. Sprint-03 `uat.sh` updated.
- R6 — Expiry trusts each event's `at` (the author's clock). Skew or a backdated
  renewal shifts who holds a task; signed, so attributable. Authority is sprint 07.
- R7 — Reads now write (the sweep). Fine for SQLite/WAL at this scale; revisit if the
  read-only server ever opens the DB read-only.

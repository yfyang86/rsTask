# Sprint 03 — Gate correctness (plan)

> PRD refs: §Audit A1 A2 A3 · §Gate rules G1 G2 G3 · PRD: `PRD/PRD-v3.md` · Depends on: sprint-01

## Goal (one sentence)

The review gate means the same thing on every replica: status converges after any
workflow history, an agent cannot approve its own submission, and a human can
force-release anyone's claim.

## Scope

**In:**
* G1 — `refold_task_tx` folds the whole workflow (`claimed / released / force_released /
  submitted / approved / changes_requested`) into the status; `apply_effect_tx` routes
  the two verdict events through it instead of arrival-order status guards.
* G2 — `approve()` refuses an agent that authored the task's latest `task.submitted`.
* G3 — `release_claim(.., forced = true)` requires a human actor and releases whoever
  holds the task; plain release stays own-claim-only.
* `Error::NotPermitted { actor, action }`.

**Out (explicitly):** enforcing G2/G3 on merge (sprint 07, PRD-v3 R2); claim renewal and
event-relative expiry (sprint 04); `update_task` checks, `UNIQUE` claim index, concurrent
`task.updated` ordering (backlog A7); any CLI surface change.

---

## Loop

### 1. Investigate
- [x] Audit probes on two CLI replicas (PRD-v3 §Audit): A1 `work` vs `review`; A2 agent
  lands its own task in `done`; A3 `release --force` by a human → `ClaimConflict`.
- [x] Root causes read in `store.rs`: sticky `submitted` flag; guarded verdict UPDATEs
  outside the fold; `release_claim` filtering on the caller's id; `approve` with no check.
- [x] The existing `force_release_frees_stale_claim` test passed the *agent's* id as
  the forcing actor, masking A3.

### 2. Setup UAT → `UAT.md`
Scenarios U1–U4 written before development.

### 3. Development
- [x] Failing tests first: 3 in `tests/sync.rs`, 2 new + 1 corrected in `tests/store_test.rs`
  (all six red before the fix).
- [x] Fold, `approve`, `release_claim`, error variant.

### 4. Test
- [x] `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [x] `./uat.sh` → `uat-evidence/`

### 5. Fix / investigate
- [x] The concurrent-verdict test (U2) exposed a second divergence beyond the audited
  one; it set the shape of the fix (full status fold, not a flag reset).

### 6. Plan next
- [x] REPORT.md; debt carried to sprint 04 / 07.

---

## Deliverables
- `crates/rstask-core/src/store.rs`, `error.rs`; tests; `PRD/PRD-v3.md`; this sprint's docs

## Definition of Done
- [x] Gate green (fmt, clippy `-D warnings`, tests)
- [x] Reject → re-claim: origin, an incremental follower and a fresh importer show identical boards
- [x] Disconnected approve and reject converge to one verdict on both replicas
- [x] Agent self-approval refused with no event logged; a different actor approves
- [x] Human force-release of an agent's live claim succeeds and replicates; an agent's is refused

## Risks touched
- R1 — Fold change alters derived status on existing replicas at their next workflow
  merge: intended; it only moves replicas toward the origin's state.
- R5 — G2/G3 hold on the local write path only; a hand-built signed event still merges
  (PRD-v3 A5). Recorded, not hidden.

# Sprint 01 — Event DAG + offline reconciliation (plan)

> PRD refs: §Decisions D2 D3 D4 D5 D8 · PRD: `PRD/PRD-v2.md` · Depends on: —

## Goal (one sentence)

Two Stores holding divergent copies of one workspace converge to identical state and
event sets by exchanging a signed, content-addressed event DAG — no network involved.

## Scope

**In:**
* `event.rs`: envelope `Signable{lamport, parents, actor_id, task_id, kind, payload, at}`
  postcard-encoded; `id = blake3(signable)`; Ed25519 signature over the signable bytes;
  verify against carried pubkey.
* Migration `0002_event_dag.sql`: `events` gains `event_id/lamport/parents/signature/
  pubkey` (NULL for v1 rows); new tables `heads`, `pending_events`, `actor_keys`.
* `create_actor` generates an Ed25519 keypair (pubkey + secret in `actor_keys`).
* Local write path: every mutation builds + signs an envelope on current heads inside
  the mutation's transaction; payload changes per PRD-v2 (`task.updated` = new values,
  releases carry `claim_id`).
* `sync.rs`: bucket fingerprints, id diff, `SyncBundle{events, actors}` postcard codec,
  `receive_bundle` (verify → park missing parents → insert → apply effect; drain
  pending), actor directory exchange.
* Claim conflicts per D8: first claim (min `(lamport, event_id)`) wins; later claims
  stay in the log but are ineffective.
* CLI: `rstask sync export > file`, `rstask sync import < file`.

**Out (explicitly):** any networking/iroh (sprint 02), relay/daemon, blob transfer,
encrypted keystore, migrating/backfilling v1 rows into the DAG.

---

## Loop

### 1. Investigate
- [x] cSharing `event.rs` / `store.rs` / `net.rs` reconcile mechanics studied (PRD-v2 D2/D3).
- [x] rsTask v1 schema + write paths mapped (events written via `log_event*`).

### 2. Setup UAT → `UAT.md`
Scenarios U1–U5 written before development (Given/When/Then + evidence).

### 3. Development
- [x] Deps: `blake3`, `postcard`, `ed25519-dalek` (+`rand_core`), `rand`.
- [x] Migration 0002 + `Event` model fields.
- [x] `event.rs` envelope + codec.
- [x] Store: keys, envelope write path, buckets/heads/pending, `receive_bundle`.
- [x] `sync.rs` diff + bundle.
- [x] CLI `sync export|import`.

### 4. Test
- [x] Automated: `cargo test -p rstask-core` — convergence, concurrent claims,
  out-of-order delivery, idempotent re-sync, signature tamper rejection.
- [x] UAT evidence captured in `uat-evidence/`.

### 5. Fix / investigate
- [x] Two real fold bugs found by tests: (1) winner's claim row never
  materialized on receiving replicas; (2) `submitted` status transition missing
  from the fold. Fixed in `refold_task_tx`.
- [x] One derived-state bug: mutation timestamp must be shared between state
  rows and the event envelope, else replicas disagree on `created_at`.
- [x] Pre-existing flakes fixed (env-var race in config tests; unguarded
  re-running of ALTER migrations — now `PRAGMA user_version` guarded).

### 6. Plan next
- [x] REPORT.md; debt carried to sprint-02 plan.

---

## Deliverables
- `crates/rstask-core/src/event.rs`, `sync.rs`, migration `0002_event_dag.sql`
- Signed-DAG write path in `store.rs`; `rstask sync export|import` in CLI
- Tests + this sprint's loop docs

## Definition of Done (on top of README "Tests")
- [x] `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace` green
- [x] Two stores, fully divergent logs, converge to identical board + heads after one exchange each way
- [x] Concurrent claims on two replicas pick the same winner on both

## Risks touched
- R1 — Claim semantics change under multi-writer (D8 rule; local CLI error path unchanged).
- R2 — v1 rows (`event_id IS NULL`) are local-only: acceptable, documented.

# Sprint 01 — Report

## Shipped
- Signed event DAG (`event.rs`): BLAKE3 content ids, Lamport clocks, Ed25519
  signatures over postcard signable bytes (PRD-v2 D3/D4).
- Migration `0002_event_dag.sql` (versioned via `PRAGMA user_version`):
  DAG columns on `events`, `heads`, `pending_events`, `actor_keys`.
  The v1 FK `events→tasks` is dropped by design: a `task.created` event causally
  precedes its task row, so the log cannot reference the state.
- Every mutation now writes a signed envelope inside its transaction; actor
  creation generates an Ed25519 keypair (interim in-DB keystore, D5).
- `sync.rs`: `SyncBundle` (events + actor directory), bucket fingerprints +
  `differing_buckets` (step-1 diff for sprint 02's wire protocol).
- Merge (`receive_bundle`): verify → learn actors (TOFU) → park on missing
  parents → drain → deterministic state effect. Concurrent claims: first claim
  by `(lamport, event_id)` wins; losers stay in the log, ineffective (D8).
- CLI: `rstask sync export|import|status`. Full UAT evidence in `uat-evidence/`
  (U1–U5 all pass; U4 via its automated transcript).

## What the tests caught (the loop worked)
1. Fold materialized the winner's claim row only where the claim was made
   locally → receiving replicas showed *zero* active claims. Fixed: refold
   upserts the winning claim row.
2. The fold never moved status to `review` on `task.submitted` → approve's
   guard silently no-oped on merge. Fixed: refold owns the submitted transition.
3. Derived state diverged by one timestamp: local `tasks.created_at` used the
   INSERT time, replicas used `event.at`. Fixed: one `at` per mutation shared
   between state SQL and the envelope.
4. Pre-existing, exposed by running the suite: config tests raced on process
   env (serialized with a mutex); reopening a DB re-ran ALTER migrations
   (now guarded by `user_version`).

## Decisions recorded (template's DECISIONS.md → here, no such file exists)
- D8 tie-break is hash-order, so the UAT asserts *agreement*, not *which* claimant.
- Merged losers' claim rows exist only where created locally; the log is the
  record, the claims table is derived state. Cosmetic asymmetry, accepted.
- `lamport` starts at 0 (genesis event). Relative order is all that matters.
- v1 event rows (`event_id IS NULL`) stay local-only forever; no backfill.

## Slipped / debt → sprint 02
- Wire protocol (iroh) — sprint-02 as planned.
- Claims table asymmetry cleanup (optional full fold rebuild) — backlog.
- Encrypted keystore (D5) — deferred in PRD-v2.
- `rstask sync status` prints raw byte-array JSON in `task log` — cosmetic; a
  hex/pretty renderer belongs to the TUI/web views.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets` zero
  warnings, `cargo test --workspace` 22/22 green (7 lib + 9 store + 6 sync).
- CLI quick start (README) re-run end to end during UAT capture.

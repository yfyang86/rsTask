# Sprint 07 — Keys and authority (plan)

> PRD refs: §RSI preconditions R1–R5 · §Audit A5 A6 · PRD: `PRD/PRD-v3.md` · Depends on: sprint-06

## Goal (one sentence)

No actor can sign as another, mint an identity, or get a change accepted that the
gate forbids — on its own replica or on anyone else's after a sync.

## Scope

**In:**
* R1 — `keys.rs` keyring: secret keys in a key directory (`--keys` /
  `RSTASK_AUTH_KEY_DIR` / `auth.key_dir`, default `keys/` beside the DB), public keys
  only in the database; `export_key` / `import_key`; pre-sprint workspaces have their
  secrets moved out on first open.
* R3 — `actor.created` events: self-signed human root as the DAG's first event, every
  other actor provisioned by a human (`provision_actor`, CLI `actor new --by`); one
  root per replica; `SyncBundle` carries events only.
* R2 — on merge: `refusal_tx` (unknown actor, wrong key, Lamport not following
  parents, kind not allowed by the author's role/kind) keeps an event out of the log;
  the fold ignores release/submit/edit by a non-holder and approvals that break G2,
  R4 or R5. Task content (`title`, `body_md`, `priority`) is folded too.
* R4 — `task.approved {submission}`; `approve(.., submission: Option<&str>)`;
  `Submitted.id`; CLI `task approve --submission`; MCP `task_approve` requires it.
* R5 — migration `0004` `tasks.protected`; `protect_task`; CLI `task protect`; MCP
  `task_protect`.

**Out (explicitly):** passphrase-encrypted key files; key rotation / revocation; role
or parent changes after creation; enforcing the blocked-task rule on merged claims;
protecting a replica's SQLite file from a local process; syncing pre-sprint-07
workspaces with new replicas; iroh transport (sprint 02).

---

## Loop

### 1. Investigate
- [x] Every write path gets its key through one function (`signing_key`) — the seam for R1.
- [x] Actors were plain rows plus a trust-on-first-use directory in the bundle: nothing
  signed said who created whom.
- [x] Merge verified signatures and actor/key match only; verdict events and content
  edits were applied by arrival order or not checked at all (A5, A7).
- [x] Lamport values were taken on trust: a rewound clock would win first-claim-wins.

### 2. Setup UAT → `UAT.md`
Scenarios U1–U5 written before development.

### 3. Development
- [x] Keyring + config; provisioning events; `refusal_tx` / `admit_tx`; fold; approve,
  protect; CLI; MCP.
- [x] Existing tests moved to the new model (actors for replica B are provisioned on A
  and their key carried over); `tests/authority.rs` added.

### 4. Test
- [x] `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [x] Mutation check: five rules removed one at a time — each makes its test fail.
- [x] `./uat.sh` → `uat-evidence/`; `OLD_BIN=<sprint-06 rstask> ./uat.sh` for the legacy check.
- [x] Sprint 03–06 `uat.sh` adapted (provisioning, `submission`) and re-run.

### 5. Fix / investigate
- [x] FK failure on first merge: a root's creation event references the actor it creates.
- [x] Migrated workspaces still held the old secret in freed pages / the WAL (see REPORT).

### 6. Plan next
- [x] REPORT.md; PRD-v3 §Trust boundary; remaining work listed.

---

## Deliverables
- `keys.rs`, migration `0004_protected.sql`, store/CLI/MCP changes, `tests/authority.rs`, docs

## Definition of Done
- [x] Gate green (fmt, clippy `-D warnings`, tests)
- [x] No secret key in the database files — fresh workspace and migrated one
- [x] An agent holding only its key cannot sign as a human or provision an actor
- [x] Forged self-approval, foreign release/submit/edit, out-of-role events, agent provisioning, rewound Lamport, foreign root: each changes nothing on the receiving replica
- [x] An approval of a superseded submission does not approve the new one, across replicas
- [x] A protected task is approved only by a human

## Risks touched
- R12 — Breaking: bundle format (events only); a pre-sprint-07 workspace keeps working
  locally but cannot sync with a new replica (it has no root event).
- R13 — The stricter fold re-derives old tasks when they are next merged: an old
  `task.approved` without `submission` no longer counts. Applies to legacy workspaces only.
- R14 — An event refused on merge leaves its descendants parked in `pending_events`
  indefinitely.

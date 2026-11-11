# Sprint 07 — Report

## Shipped
- **Keys outside the database (R1).** Secret keys are files in a key directory; the
  database holds public keys only. `rstask` and `rstask-mcp` take `--keys`
  (`RSTASK_AUTH_KEY_DIR`, `auth.key_dir`). Without an actor's key file every write as
  that actor fails with `no signing key`. Pre-sprint workspaces are migrated on first
  open and the old bytes scrubbed.
- **Signed provisioning (R3).** Actors are `actor.created` events. The first actor is
  the workspace root and must be human; every later actor is signed by a human
  (`actor new --by`, default the root). A replica has one root and refuses bundles
  built on another.
- **Authority on merge (R2).** Before an event enters the log: known author, matching
  key, Lamport one past its parents, kind allowed by the author's roles/kind. In the
  fold: only the holder releases, submits or edits a claimed task; G2, R4 and R5
  decide whether an approval counts. Task content is now folded in log order, so
  concurrent edits converge (closes that part of A7).
- **Evidence-bound approval (R4).** `task.approved` names the submission event it
  approves. `task approve --submission <id>` / MCP `task_approve` refuse a stale id;
  across replicas an approval of a superseded submission is ineffective.
- **Protected tasks (R5).** `task protect` / MCP `task_protect`: irreversible; from
  then on only a human's approval counts.

## What the tests caught (the loop worked)
1. **Secrets survived the migration.** Setting `actor_keys.secret` to NULL left the
   key readable in the database file (freed page) and the WAL. The UAT's byte search
   showed "before 1, after 1"; reproduced as a red test, fixed with `VACUUM` +
   `wal_checkpoint(TRUNCATE)` after the move. (The first version of that search used
   a tool this machine lacks and printed a reassuring `0` — caught when its stderr
   showed up in the transcript.)
2. A root's creation event references the actor it creates: the first merge failed a
   foreign key until the actor row was inserted before its event.
3. The authority tests were written after the implementation, so they were
   mutation-checked: removing any one of five rules (approval conditions, role/kind
   refusal, Lamport check, holder check, single root) fails the matching test.

## Decisions recorded
- **Any human may provision, not only the root.** The root is the anchor; humans it
  (transitively) created are trusted to add actors.
- **A fresh replica adopts the first root it is given.** Pinning a root out of band is
  not implemented.
- **Out-of-role events are refused; out-of-turn events are kept but ineffective.** The
  first depends only on the author, the second on the task's state at that point.
- **MCP requires `submission` on approve; the CLI makes it optional.** Core always
  records which submission an approval is for.
- **Claims on blocked tasks are not judged on merge** — an honest concurrent edit.
- **Pre-sprint-07 workspaces are local-only.** No backfill of root events.

## Slipped / debt
- Key files are plaintext (`0600`); no passphrase, rotation or revocation; a lost root
  key cannot be replaced.
- A process with write access to a replica's SQLite file can still corrupt that
  replica's derived state (PRD-v3 §Trust boundary). Mutually distrusting agents need
  separate replicas — which needs the transport (sprint 02).
- Descendants of a refused event stay in `pending_events`.
- Protection is opt-in per task.
- `walkthrough/uat/run.sh` predates sprints 03–07 and no longer runs as written.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 57/57 green (7 lib + 11 authority + 24 store
  + 13 sync + 2 MCP stdio).
- `./uat.sh`: U1–U5 pass; transcripts in `uat-evidence/` (`u1b` generated with a
  sprint-06 binary built from the previous commit).
- Sprint 03, 04, 05 and 06 `uat.sh` re-run under the new rules: all pass.

# Sprint 08 — Report

## Shipped
- **Enrolment without moving a secret (M6).** `rstask actor request NAME` generates
  the actor's key in the local key directory and prints a request (id, kind, name,
  roles, public key, proof signature). A human runs `rstask actor admit '<request>'`
  where their own key is; that signs the `actor.created`. After the next sync the
  requesting machine can act. Closes walkthrough F1 and LAN finding L3: no key file
  is copied between machines.
- **Periodic sync (M7).** `p2p sync --every SECS` and `p2p listen --sync-every SECS`.
  In this mode a line is printed only for a round that moved something, lost a claim
  or failed; a peer that is down is retried next round.
- **Lost claims reported (M8).** `Merge.lost_claims` lists claims this replica held
  that a merge ended: `superseded` (an earlier claim from elsewhere won) or
  `force_released`. The dialing side sees it in the sync output; the listening side
  logs a warning. Closes walkthrough F3.
- **CI.** `.github/workflows/ci.yml` runs fmt, clippy `-D warnings` and the tests on
  pull requests and on `main`, with the toolchain pinned to 1.97.0.

## What the tests caught (the loop worked)
- **CLI logs went to stdout.** The UAT showed the listener's lost-claim warning
  missing from its error stream: `tracing` was writing to stdout, the channel that
  carries JSON and bundles. Logs now go to stderr.

## Decisions recorded
- The requester picks the actor id (a UUID) so its key file can be named before the
  workspace has ever been synced to that machine.
- The proof covers every field: a request edited on the way (a worker's turned into
  a human's, a different key) is refused. It does not make the request confidential.
- `actor admit` cannot create a workspace's first actor: the root needs its own key
  on the machine that founds the workspace.
- `actor new` keeps working as before (key generated where the human is) for actors
  that live on the same machine.
- Not a loss: the holder's own release, a submission, expiry.

## Slipped / debt
- **U4 is open until the pull request exists**; the workflow has not run yet.
- An agent is not pushed a notice of a lost claim; it finds out at its next
  `task_renew` (`already claimed by …`) or `task_get`.
- Enrolment is CLI-only (no MCP tool): admission is a human act.
- Still untested: two physical machines, NAT, relays, dialing by id alone.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 66/66 green, two consecutive runs
  (9 lib + 13 authority + 3 net + 24 store + 15 sync + 2 MCP stdio).
- `./uat.sh`: U1–U3 pass. Sprint 02–07 `uat.sh` re-run: all pass.

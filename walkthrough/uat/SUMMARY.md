# Walkthrough UAT — SUMMARY

**Verdict: PASS** — rsTask ran a real software sprint end-to-end through its
own CLI: Markdown-native planning, exclusive claim, TTL crash recovery,
delivery against acceptance criteria, review gate, human approval, and
two-replica convergence. The sprint task (WT-1) is a real code change in this
repo, merged into the working tree with the full quality gate green.

Re-run from scratch: `./run.sh all`. Raw transcripts: `evidence/`.
Two earlier runs are preserved under `evidence/run1-*` / `run2-*` — the UAT
caught its own scripting bugs before catching product behavior; both are
archived as evidence of the process, not hidden.

## Scenario results

| # | Scenario | Result | Evidence |
|---|----------|--------|----------|
| WT-1 | AC1 hex rendering, AC2 valid JSON, AC3 gate | PASS (gate exit 0) | 30-delivery.txt |
| S1 | Planning: task in queue, Markdown body intact, signed creation event | PASS | 10-plan.txt |
| S2 | Exclusive claim: bob takes it; alice's attempt → `ClaimConflict` after her replica learns the claim | PASS | 20-claim.txt |
| S2b | TTL expiry: alice's 1s claim goes stale; bob's claim succeeds | PASS | 21-ttl.txt |
| S3 | Delivery: real diff + green gate + hex `task log` | PASS | 30-delivery.txt |
| S4 | Review gate: approve from `work` → `InvalidTransition`; alice approves (review→done) | PASS | 20-claim.txt, 40-review.txt |
| S4p | Self-approval probe: bob approves his own work | **RECORDED — see F2** | 41-selfapprove-probe.txt |
| S5 | Sync: identical `sync status` (same heads, 9 events, 0 pending); re-exchange moves nothing | PASS | 50-sync.txt |
| S6 | Immutability: full chain `created→claimed→submitted→approved` in Lamport order, identical on both replicas; board shows sprint done | PASS | 60-final.txt |

## Findings (the UAT's real value)

- **F1 — actor keys never leave the origin replica (by design, D5).**
  First run created both actors on replica `a`; after directory sync, replica
  `b` held verify-only copies and bob could not act on his own host
  (`NoSigningKey`). Correct modeling — recorded in `UAT.md`: each actor is
  created on their native replica. Follow-up needed: actor provisioning flow
  for "same person, second machine" (cf. cSharing device linking) or an
  explicit key-export command. Backlog.
- **F2 — self-approval is not blocked.** The PRD promises "agents can't
  self-approve"; the probe shows bob submitting and then approving his own
  work, landing in `done`. The gate enforces the *shape* (review step exists)
  but not the *reviewer*. Fix: `approve()` must reject `actor.kind == Agent`
  when the actor also authored the `task.submitted` event (or any actor-kind
  restriction per PRD). Flagged for the next sprint; arguably a P1.
- **F3 — two concurrent claims made before any sync both succeed locally**
  and resolve only at merge (D8 first-wins). Expected distributed semantics
  (proven in sprint-01 U3), but worth an explicit CLI warning when a merge
  marks your claim ineffective — today the loser learns only from the log.

## Environment

- binary: `target/debug/rstask` built from this working tree (WT-1 included)
- replicas: `/tmp/rstask-walkthrough/{a,b}.sqlite` (recreated per run)
- gate inside transcript: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` — all green

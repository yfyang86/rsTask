# RsTask walkthrough UAT — prove it runs a software sprint

**Product under test:** `rstask` CLI + core (post-sprint-01).
**Method:** dogfooding. A real sprint is executed *through rsTask itself* —
planning, exclusive claim, TTL recovery, delivery, review gate, approval, and
two-replica sync — on a genuine code task taken from sprint-01's own debt
list: **WT-1 "hex-render event ids in `rstask task log`"** (REPORT.md →
"cosmetic; a hex/pretty renderer belongs to the TUI/web views").

**Actors:** `alice` (human, planner/reviewer) · `bob` (agent, implementer) —
each created on their **native** replica (alice on `a`, bob on `b`): signing
secrets never leave the origin replica (PRD-v2 D5), so a directory-synced
actor is verify-only elsewhere. This bit us in the first walkthrough run and
is recorded as finding F1.
**Replicas:** `a.sqlite` = alice's machine, `b.sqlite` = bob's host, exchanged
with `rstask sync export/import` (sprint-01 file bundles).
**Rule:** every scenario runs through the CLI only; transcripts are captured
verbatim by `run.sh` into `evidence/`. Re-run from scratch: `./run.sh all`.

## WT-1 acceptance criteria (the sprint task itself)

- AC1: `rstask task log` renders `event_id`, `signature`, `pubkey` as hex
  strings (absent for pre-DAG rows), no byte arrays.
- AC2: output stays valid JSON; non-binary fields unchanged.
- AC3: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D
  warnings && cargo test --workspace` green.

## Scenarios

### S1 — Planning (Markdown-native)
Given fresh replicas. When alice registers both actors and files WT-1 with a
Markdown body (`--body` acceptance criteria). Then the task sits in `queue`;
`task show` returns the Markdown intact; the creation is already a signed DAG
event. → `evidence/10-plan.txt`

### S2 — Exclusive claim (atomic lock)
When bob claims WT-1 on his replica (queue→work). Then alice's replica learns
the claim via sync and her claim attempt fails with `ClaimConflict`; the board
shows WT-1 in `work` under bob. (Two claims made *before* any sync would both
succeed locally and resolve at merge by D8 — proven in sprint-01 U3.) → `evidence/20-claim.txt`

### S2b — TTL expiry returns the task to the pool (agent crash recovery)
Given alice claims a probe task with `--ttl-secs 1` and goes silent. When 2s
pass and bob claims it. Then bob's claim succeeds: the stale claim is swept,
the task returns to the pool. → `evidence/21-ttl.txt`

### S3 — Delivery against AC1–AC3
When bob implements WT-1 (real code change in this repo). Then the gate is
green and `task log` prints hex ids; evidence: the diff, the gate output, and
a sample `task log`. → `evidence/30-delivery.txt`

### S4 — Review gate
When bob submits (work→review). Then a `done` move is impossible directly
from `work` (`InvalidTransition`). **Probe:** bob approving his own work while
in `review` — record the *actual* behavior. Finally alice approves
(review→done). → `evidence/40-review.txt`, `evidence/41-selfapprove-probe.txt`

### S5 — Two-replica sync converges the sprint state
After every phase the replicas exchange bundles; at the end a full exchange
both ways makes `sync status` identical, and a re-exchange moves nothing. → `evidence/50-sync.txt`

### S6 — Immutability + final board
Then WT-1's log contains the full chain `task.created → task.claimed →
task.submitted → task.approved` in Lamport order, signed; the board shows the
sprint done. → `evidence/60-final.txt`

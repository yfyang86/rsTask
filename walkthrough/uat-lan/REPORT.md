# LAN hello-world UAT — REPORT

**Verdict: PASS** — sprint-02's **U3 "LAN sync by hand"**, previously NOT RUN
("single container, no second host"), now runs green: two replicas
reconciled over the machine's real LAN address (`192.168.8.2:7701`) with the
relay disabled and a shared workspace token. No loopback shortcut, no
third-party service.

Re-run: `./run.sh` (regenerates `evidence/10-lan-hello.txt`).
Scenario and pass criteria: `UAT.md`. Raw transcript: `evidence/`.

## What ran (2026-11-12)

1. **A** (alice, human root) filed the task "hello world over LAN" and
   served its workspace on `0.0.0.0:7701` (`RSTASK_P2P_RELAY=disabled`).
2. **B**, starting from an empty database, dialed A by its LAN address:
   `p2p sync 43424804…@192.168.8.2:7701` → exit 0, `received.added = 3`
   (alice, bob, task). B's board showed the task in `queue`.
3. **B wrote back**: bob — an agent actor provisioned by alice on A, whose
   secret key file was carried to B by hand — claimed the task on B. A
   second `p2p sync` (no argument, known peer) pushed the claim to A:
   `sent = 1`. A never dialed B; one sync is fully two-way.
4. **Convergence**: identical `sync status` heads
   (`f513e28a…a1c6d69`, single head), identical boards (task in `work` under
   bob), identical event chains `task.created → task.claimed` on both
   replicas. A re-sync moved nothing (`received.added = 0`, `sent = 0`).

| # | Pass criterion (UAT.md) | Result |
|---|-------------------------|--------|
| 1 | B's sync via LAN IP exits 0; hello-world task on B's board | PASS |
| 2 | After write-back: heads identical, boards identical | PASS |
| 3 | Re-sync moves nothing (`received.added = 0`) | PASS |
| 4 | Event chain identical on both replicas | PASS |

The two-machine path (`./run.sh host` / `./run.sh join 'ID@IP:7701'`) was
exercised end-to-end on this host (join pulled the task, `added = 2`, board
correct); the commands are exactly what two LAN machines would run.

## Findings

- **L1 — (UAT scripting, fixed) orphaned listener.** Backgrounding
  `p2p listen` through the `a()` shell function made `$!` a subshell PID;
  TERM killed the subshell and the real `rstask` process kept UDP 7701 —
  the same trap sprint-02's `uat.sh` comments on. `listen_a` now invokes
  the binary directly and takes the DB path as an argument; verified no
  listener survives a run. Not a product bug.
- **L2 — (product, positive) the printed spec is directly dialable.**
  Bound to `0.0.0.0`, `p2p listen` prints `spec` with the host's primary
  LAN address already filled in (`…@192.168.8.2:7701`), so the join command
  is copy-pasteable as-is.
- **L3 — (by design, D5) no keys over the wire.** B could only write after
  bob's key file was copied out-of-band (`a/keys/<id>.key` → `b/keys/`).
  Consistent with walkthrough F1; the "same person, second machine"
  provisioning flow remains backlog there.

## Environment

- binary: `target/debug/rstask` built from this working tree
- replicas: throwaway `/tmp/rstask-uat-lan/{a,b}` (`local`); persistent
  `./instance-a` / `./instance-b` for `host` / `join`
- network: en0 `192.168.8.2`, UDP 7701, relay `disabled`, token
  `RSTASK_P2P_TOKEN` (default `uat-lan-hello`, override for real use)

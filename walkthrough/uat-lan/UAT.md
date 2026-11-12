# LAN hello-world UAT — sprint-02 U3, run for real

**Product under test:** `rstask p2p` (iroh QUIC sync, sprint 02).
**Gap closed:** sprint-02's UAT proved sync on loopback only and left
**U3 — LAN sync by hand** as NOT RUN ("single container, no second host").
This walkthrough runs it: `walkthrough/uat-lan/evidence/u3-lan-not-run.txt`
in sprint-02 is superseded by `evidence/10-lan-hello.txt` here.

**Scenario (hello world):** replica **A** (alice, human root) files a task
"hello world over LAN" and serves its workspace on `0.0.0.0:7701`. Replica
**B** starts empty and dials A **by its LAN address** — not `127.0.0.1` —
with the relay disabled (`RSTASK_P2P_RELAY=disabled`), so only direct
addresses are used. B receives the task; then B writes back (bob, an agent
actor provisioned by alice, claims the task — his secret key file is carried
to B by hand, never over the wire) and syncs again. One `p2p sync` is
two-way, so A learns the claim without ever dialing B.

**Pass criteria**
- B's `p2p sync <node>@<A's LAN IP>:7701` exits 0 and B's board shows the
  hello-world task.
- After B's write-back sync, `sync status` heads are identical on A and B
  and both boards match.
- A re-sync moves nothing (`received.added = 0`).
- The task's event chain (`task.created → task.claimed`) is identical on
  both replicas.

## Run it

```sh
./run.sh          # local: both replicas on this machine, B dials the LAN IP
```

Two real machines (same LAN, same `RSTASK_P2P_TOKEN` on both, default
`uat-lan-hello`):

```sh
host A:  ./run.sh host                          # prints the join command
host B:  ./run.sh join 'NODE_ID@A_LAN_IP:7701'  # pulls the task, shows board
```

`host` keeps `./instance-a/` and `join` keeps `./instance-b/` beside this
file, so the pair can be re-run; `local` uses throwaway `/tmp/rstask-uat-lan`.

**Note on actors:** B must not create its own first actor — that would make
B a different workspace root and its events would be refused (walkthrough
finding F1, PRD-v2 D5). Writers on B use an actor provisioned on A whose key
file was carried across (`a/keys/<id>.key` → `b/keys/`).

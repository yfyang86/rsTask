# Sprint 02 — UAT

Written before development. Evidence → `uat-evidence/`.

## U1 — In-process two-node sync (automated)
**Given** two `Store`s with divergent DAGs, two `Node`s on one tokio runtime, relay
disabled, same token.
**When** `node_a.sync_with(&store_a, peer_b)`.
**Then** both stores converge (heads equal, board equal) — mirrors sprint-01 U2 but over
QUIC. Evidence: test output.

## U2 — Loopback CLI sync
**Given** workspace dirs `wa/` and `wb/` (each `rstask init` + actors + task), B knows
A's node spec `ID@127.0.0.1:PORT`.
**When** `rstask --db wb p2p sync <A-spec>`; then A runs `p2p sync` back to B (or a
second push from B).
**Then** both `task list` outputs identical. Evidence: transcripts.

## U3 — LAN sync by hand
**Given** two hosts on the same LAN, token shared via env, A `p2p listen`, B `peer add`.
**When** B syncs.
**Then** convergence; note NAT/relay behaviour in evidence.

## U4 — Wrong token refused
**Given** B uses token T2, A uses T1.
**When** B dials A.
**Then** handshake fails before any event/frame exchange; clean error, no panic.
Evidence: stderr on both sides.

## U5 — Unknown peer / stale addrs
**Given** B dials an id nobody serves (or a dead addr).
**Then** bounded failure (dial timeout), error reported, stores untouched.
Evidence: stderr + exit code.

## Commands under test
```sh
rstask p2p listen                  # print node id + addr (or run a one-shot accept)
rstask p2p peer add ID@HOST:PORT
rstask p2p peer list
rstask p2p sync [ID@HOST:PORT]     # default: all known peers
rstask p2p status                  # node id, relay mode, peers
```

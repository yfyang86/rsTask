# Sprint 02 — iroh transport (plan)

> PRD refs: §Decisions D1 D6 D7 · PRD: `PRD/PRD-v2.md` · Depends on: sprint-01

## Goal (one sentence)

Two `rstask` nodes on one machine (tests) and on a LAN (UAT) reconcile their event
DAGs over iroh QUIC with a token-authenticated handshake — no file bundles.

## Scope

**In:**
* Deps: `iroh 1.3` (features `metrics, fast-apple-datapath, portmapper, tls-aws-lc-rs`),
  `futures`.
* `net.rs`: node identity key (per workspace DB dir, file `node.key`), `Endpoint` with
  ALPN `rstask/1`, `RelayChoice{default|disabled|custom}` from config `p2p.relay`.
* Handshake per D7: `Hello{token_hash? no — nonces}` + mutual MAC over shared workspace
  token, both endpoint ids bound into the MAC (iroh TLS authenticates the ids).
* Protocol (postcard frames, request/response over one bi-stream):
  `GetBuckets → Buckets`, `GetIds{buckets} → Ids`, `GetEvents{ids} → Events`,
  `Push{events, actors} → PushAck`, `Actors → Actors` (directory exchange).
* Reuse sprint-01 `sync.rs` diff/apply unchanged: `net` is just another bundle carrier.
* Config: `[p2p] enabled, relay, token` (+ env `RSTASK_P2P_TOKEN`, `RSTASK_P2P_RELAY`).
* CLI: `rstask p2p peer add SPEC|list`, `rstask p2p sync [PEER]`, `rstask p2p status`
  (node id, relay mode, peers). Store peers in DB table `peers(id, addrs, last_seen)`.

**Out (explicitly):** relay server hosting, daemon/always-on mode, hole-punching beyond
what iroh gives for free, NAT-specific UAT (LAN UAT only), registered-user guards,
rekey/rotation, attachment transfer.

---

## Loop

### 1. Investigate
- [ ] cSharing `net.rs` session/handshake/sync_with + `session.rs` probes.
- [ ] iroh 1.3 API surface vs cSharing pin (Endpoint/Router/ProtocolHandler).

### 2. Setup UAT → `UAT.md`
U1 in-process two-node sync (relay disabled); U2 loopback CLI sync; U3 LAN by hand;
U4 wrong-token refusal; U5 unknown-peer handling.

### 3. Development — per Deliverables below.
### 4. Test
- [ ] `cargo test -p rstask-core` net tests with in-process endpoints, relay disabled.
- [ ] `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
### 5. Fix / investigate — triage, re-loop.
### 6. Plan next — REPORT.md; carry debt (daemon? attachments?) to backlog.

## Deliverables
- `crates/rstask-core/src/net.rs`, migration `0003_peers.sql`
- Config extensions, `RSTASK_P2P_*` env
- CLI `rstask p2p` subcommands

## Definition of Done
- [ ] Two in-process nodes converge over QUIC with relay disabled (test).
- [ ] Two CLI workspaces on one host sync via `rstask p2p sync` (UAT transcript).
- [ ] Wrong token → refused before any event exchange.

## Risks touched
- R3 — iroh build cost (~hundreds of crates): keep cSharing's feature pin + dev opt-levels.
- R4 — Token leakage = full workspace write access: token never logged, env-preferred.

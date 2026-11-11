# RsTask — Spec v2 (P2P sync)

**Supersedes:** `PRD-v1.md` §Stack (`p2panda` → removed), §Deferred (p2p claim-conflict
strategy → designed below). Everything else in v1 stands.

v2 designs the multi-node layer: how replicas of the RsTask workspace connect and
converge. The design reuses two proven mechanisms from the sibling cSharing codebase
(`github.com:yfyang86/cSharing`) — nothing else.

---

## The two problems

1. **Connectivity** — two nodes behind NAT must reach each other without a central
   server.
2. **Replication + conflicts** — the immutable event log must converge across nodes,
   and "claim = atomic lock" must stay meaningful when two writers claim concurrently.

## Decisions

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| D1 | Transport | **iroh 1.3** (QUIC, NAT hole-punching, relay fallback), features `metrics, fast-apple-datapath, portmapper, tls-aws-lc-rs` | Replaces v1's `p2panda`. Proven in cSharing; p2panda's node stack (aquadoggo, materialization) fights "one SQLite DB, one core". Feature flags mirror cSharing's pin, minus `unstable-net-report` (unused). |
| D2 | Sync algorithm | Port cSharing's bucket reconcile: per-bucket BLAKE3 fingerprints over Lamport ranges → id diff → batched exchange | 3 cheap steps, git-like, converges in any connection order. (`cSharing/src/store.rs` `buckets()`, `net.rs` `reconcile()`.) |
| D3 | Event identity | Event = **BLAKE3 content-addressed DAG node**: `id = blake3(postcard(Signable{lamport, parents, actor, task, kind, payload, at}))`, parents = current heads, Lamport clock | rsTask's v1 log (`seq AUTOINCREMENT`) is single-node. Lamport + heads + pending queue come from cSharing's `event.rs`/`store.rs`. |
| D4 | Signatures | **Ed25519** (ed25519-dalek 2) per actor, over the postcard `Signable` bytes | v1 stack already lists `ed25519-dalek`. |
| D5 | Key storage | Actor keypairs in the workspace DB (`actor_keys` table). No passphrase keystore yet. | PRD threat model = trusted local workspace. Separate encrypted keystore (cSharing `keystore.bin`) is explicitly out of scope. |
| D6 | Framing | **postcard** length-prefixed frames over iroh QUIC bi-streams, ALPN `rstask/1` | Direct port of cSharing's session layer. |
| D7 | Peer auth | Shared **workspace token**: `MAC(token, "client"/"server" ‖ endpoint_ids ‖ nonces)` handshake, both ids bound (iroh TLS authenticates them) | Simplified from cSharing's passphrase-derived key. Offline-friendly; token lives in config/env. |
| D8 | Claim conflicts | **First claim wins**: among claim events for one task, the one with min `(lamport, event_id)` takes the lock; later claim events are kept in the log but *ineffective* | Un-deferred from v1. Deterministic ⇒ every replica folds to the same state. Local `claim_task` keeps its synchronous `ClaimConflict` error path. |

**Explicitly not taken from cSharing:** SQLCipher, Argon2id room keys, XChaCha20 epoch
sealing, ML-KEM/ML-DSA post-quantum hybrid, zero-knowledge ring signatures, room/epoch
membership & rekey machinery, login guard, daemon, device linking, blob chunk transfer
(attachments stay local until a later sprint says otherwise), its TUI, its task tracker.

## Architecture (v2 delta)

```
CLI ─┐
TUI ─┼─► Core ─► SQLite (state, events, locks, heads, pending)
Web ─┤           Markdown + binaries (content)
MCP ─┘
         ▲                    ▲
    Local REST (Axum)   sync.rs │ net.rs (iroh QUIC)
                              P2P (optional, off by default)
```

- `rstask-core/src/event.rs` — envelope: id/lamport/parents/sign/verify, postcard codec.
- `rstask-core/src/sync.rs` — buckets, id diff, `SyncBundle` encode/decode, apply/merge
  (verify → park missing parents → insert → state effect), actor directory exchange.
- `rstask-core/src/net.rs` *(sprint 02)* — iroh endpoint, token handshake, protocol
  `GetBuckets / GetIds / GetEvents / Push / Actors`.

## Event log (multi-node)

New columns on `events` (`event_id`, `lamport`, `parents`, `signature`, `pubkey`;
v1 rows stay local-only, `event_id IS NULL`), plus tables `heads`, `pending_events`,
`actor_keys(actor_id, pubkey, secret)`.

- Local write: build envelope on current heads, sign, insert + update heads **in the same
  transaction** as the state mutation (v1 invariant kept).
- Merge apply effects (deterministic, arrival-order independent):
  `task.created` (insert if unknown) · `task.updated` (apply provided fields only) ·
  `task.claimed` (first-wins, D8) · `claim.released` / `claim.force_released` /
  `task.submitted` (release named claim) · `task.approved` / `task.changes_requested`
  (workflow gate, guarded: no-op if status already moved).
- Causal completeness: an event is applied only when all parents are present; others
  park in `pending_events` and drain on arrival.

Payload changes from v1 (needed for replay): `task.updated` carries the **new** field
values (was: old values); release/submit events carry `claim_id`.

## Config (additions)

```toml
[p2p]
enabled = false             # off by default
transport = "quic"          # iroh QUIC (D1); reserved
relay = "default"           # default | disabled | https://relay…  (sprint 02)
token = ""                  # workspace token; env RSTASK_P2P_TOKEN (sprint 02)
```

## Sprints

| Sprint | Theme | Status |
|-------:|-------|--------|
| 01 | Event DAG + offline reconciliation (in-process / file sync: `rstask sync export\|import`), claim-conflict rule | this loop |
| 02 | iroh transport: handshake, protocol, `rstask p2p` commands, relay config | planned |

## Deferred (updated from v1)

- Encrypted keystore for actor keys (D5 records the interim).
- P2P attachment (blob) transfer.
- Relay server hosting / daemon mode / registered-users guard.
- Dynamic plugin loading (WASM) — unchanged.
- Agent self-registration — unchanged.

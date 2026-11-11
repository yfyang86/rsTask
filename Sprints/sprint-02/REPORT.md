# Sprint 02 — Report

Run after sprint 07. Source of the design: the session layer of cSharing's `net.rs`
(`github.com/yfyang86/cSharing`, read at `4a4701c`), with the PRD-v2 "not taken" list
left out.

## Shipped
- **`rstask-core/src/net.rs`** (feature `net`): an iroh 1.3 endpoint with ALPN
  `rstask/1`, `RelayChoice {Default | Disabled | Custom}`, `PeerSpec` (`ID@host:port,…`),
  length-prefixed postcard frames, and a `Node` that serves one workspace and can
  `sync_with` a peer.
- **Handshake (D7).** `Hello → HelloAck{mac_s} → Auth{mac_c} → AuthOk`; both MACs are
  keyed BLAKE3 over role, both endpoint ids and both nonces, with the key derived from
  the workspace token. A dialer with the wrong token stops at the server's MAC; a
  client that skips that check is refused by the server's check of its MAC. A node
  without a token does not start.
- **Reconcile (D2).** Bucket fingerprints → ids of differing buckets → `GetEvents` /
  `Push` in batches of 48, parents first. One `sync_with` converges both sides.
  Received events go through the same `receive_events` path as file bundles, so the
  sprint-07 authority rules apply unchanged; `PushAck` reports how many the peer refused.
- **Peers.** Migration `0005`, `add_peer` / `list_peers` / `touch_peer`; a successful
  sync remembers the peer and its addresses.
- **CLI.** `rstask p2p listen | peer add | peer list | sync [SPEC] | status`; config
  `p2p.relay`, `p2p.token`, `p2p.bind` with `RSTASK_P2P_RELAY|TOKEN|BIND`. The node's
  identity is `node.key` beside the database.

## Taken from cSharing / left out
- **Ported:** relay choice, peer spec, frame codec and limits, MAC construction
  (length-prefixed keyed BLAKE3, constant-time compare), endpoint binding, session
  open/close, the three-step reconcile, the accept handler's linger before teardown.
- **Left out (PRD-v2):** rooms and locators, passphrase-derived keys, post-quantum key
  exchange (iroh's default TLS provider is used), zero-knowledge logins, registry and
  credentials, login guard and bans, relay-only mode, blob transfer, the daemon, its
  relay server (`iroh-relay` is not a dependency).

## Deviations from the plan
- **No `Actors → Actors` message.** Since sprint 07 actors are `actor.created` events
  in the DAG; a directory exchange would reintroduce trust-on-first-use.
- **`net.rs` is behind a cargo feature** that only the CLI enables: `rstask-mcp`,
  `rstask-server` and `rstask-tui` do not build iroh (checked with `cargo tree`).
- **`node.key` sits beside the database**, not in the key directory: it identifies a
  replica, signs no events, and must not travel with an actor's key.
- `p2p.enabled` is left reserved; running a `p2p` command is the opt-in.
- `futures` was not needed (peers are synced one after another).

## What the tests caught (the loop worked)
1. **`ids_in_buckets` returned nothing** — a sprint-01 helper that nothing had ever
   called. Its SQL mixed `?1` with plain `?`, and sqlx then binds the plain ones from
   the first argument again. The first two-node sync "succeeded" having moved zero
   events. Fixed; a regression test now covers steps 2 and 3 without the network, and
   no other query mixes the two styles.
2. **The listener could not be stopped from a script.** A process started in the
   background ignores Ctrl-C, so the first UAT run hung at exit; a second attempt
   signalled the wrong process (a subshell). `p2p listen` now also stops on SIGTERM
   and the script starts the listener directly.

## Decisions recorded
- iroh pinned to `=1.3.0`, the version cSharing's lockfile uses and the API this port
  was read against (1.4.1 is the newest release).
- The token is a membership gate, not an authority: holding it lets a peer exchange
  events, nothing more. It is never printed (`p2p status` says `set` / `unset`).
- Events the peer refuses are re-offered on every sync (their buckets keep differing).
  Harmless, but wasted work; a per-peer "refused" memo is backlog.

## Slipped / debt
- **U3 (LAN, two hosts) was not run** — this environment is one container. Everything
  else ran on loopback with the relay disabled, so NAT traversal, relay fallback and
  dialing by id alone (`relay = default`) are untested here.
- No daemon / always-on mode, no automatic periodic sync, no notification when a sync
  changes a task an agent holds (walkthrough F3).
- No rate limiting or lockout on failed handshakes (cSharing's guard was not ported).
- The token sits in config or the environment in plain text; no rotation.
- `p2p sync` syncs peers sequentially.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 63/63 green, two consecutive runs
  (9 lib + 11 authority + 3 net + 24 store + 14 sync + 2 MCP stdio).
- `./uat.sh`: U1, U2, U4, U5 pass; U3 recorded as not run. Sprint 03–07 `uat.sh`
  re-run after the CLI change: all pass.

# Sprint 08 — Replicas in practice (plan)

> PRD refs: §Multi-agent M6 M7 M8 · PRD: `PRD/PRD-v3.md` · Depends on: sprint-02, sprint-07
> Input: `walkthrough/uat-lan/REPORT.md` (L3), walkthrough F1 and F3, sprint-02 and
> sprint-07 debt lists, and the missing CI noted when the first pull request was merged.

## Goal (one sentence)

Running rsTask on two machines needs no secret carried between them, no one typing
`sync`, and no silent surprise when a merge takes a claim away — and every pull
request is checked.

## Scope

**In:**
* M6 — `Store::request_actor` (key generated and kept locally, request with public
  fields and a proof signature) and `Store::admit_actor` (a human signs the
  `actor.created`); CLI `actor request`, `actor admit`.
* M7 — CLI `p2p sync --every SECS`, `p2p listen --sync-every SECS`; stops on Ctrl-C / SIGTERM.
* M8 — `Merge.lost_claims` (`superseded` / `force_released`), printed by `p2p sync` and
  `sync import`, logged by the listener.
* `.github/workflows/ci.yml`: fmt, clippy `-D warnings`, tests, toolchain pinned.

**Out (explicitly):** a persistent notice an agent can poll for a lost claim (it
learns from `task_renew` / `task_get`); MCP tools for enrolment; a service unit or
daemon wrapper; syncing peers in parallel; key rotation; NAT / relay testing.

---

## Loop

### 1. Investigate
- [x] PR #2 (`walkthrough/uat-lan`): LAN sync passes; L3 repeats F1 — the writer on B
  needed a key file copied by hand.
- [x] `provision_actor` always generated the key on the provisioning replica.
- [x] The fold already knows which claims lose; nothing surfaced it.

### 2. Setup UAT → `UAT.md`
Scenarios U1–U4 written before development.

### 3. Development
- [x] Tests first: 2 in `tests/authority.rs`, 1 in `tests/sync.rs`.
- [x] Core, then CLI, then the workflow.

### 4. Test
- [x] `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [x] `./uat.sh` → `uat-evidence/` (U1–U3); sprint 02–07 `uat.sh` re-run.
- [ ] U4: needs this sprint's pull request to exist.

### 5. Fix / investigate
- [x] CLI logs went to stdout (see REPORT).

### 6. Plan next
- [x] REPORT.md.

---

## Deliverables
- Core `request_actor` / `admit_actor`, `Merge.lost_claims`; CLI commands; CI workflow; docs

## Definition of Done
- [x] Gate green
- [x] An actor requested on B and admitted on A acts on B; its key file exists only on B
- [x] An altered request is refused; a request cannot found a workspace
- [x] With `p2p sync --every 1`, a task filed on A appears on B and a claim on B appears on A, unprompted
- [x] The losing replica of a concurrent claim, and the holder of a force-released claim, are told
- [ ] CI runs on the pull request

## Risks touched
- R15 — The request is not encrypted or addressed: anyone who sees it can present it,
  but only a human with a key can admit it, and admitting gives the requester exactly
  the kind and roles the human saw.
- R16 — CI builds iroh on every run (cached); the toolchain pin must be moved by hand.

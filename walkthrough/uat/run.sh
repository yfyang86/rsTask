#!/usr/bin/env bash
# Walkthrough UAT driver — executes scenarios S1–S6 of UAT.md through the
# rstask CLI and captures verbatim transcripts into evidence/.
# Re-runnable from scratch: ./run.sh all   (assumes the WT-1 code change is
# present in the working tree; `deliver` records it as evidence).
set -u
cd "$(dirname "$0")"
EV=$PWD/evidence
REPO=$(cd ../.. && pwd)
BIN=$REPO/target/debug/rstask
WS=/tmp/rstask-walkthrough
IDS=$WS/ids.env

mkdir -p "$EV"

r() { # r: log the conceptual command, run it verbatim, tolerate failure
    echo "\$ rstask $*" >>"$LOG"
    if "$BIN" --db "$DB" "$@" >>"$LOG" 2>&1; then
        echo "(exit 0)" >>"$LOG"
    else
        echo "(exit $?, expected for negative probes)" >>"$LOG"
    fi
    echo >>"$LOG"
}

plan() {
    rm -rf "$WS" && mkdir -p "$WS"
    LOG=$EV/10-plan.txt; : >"$LOG"
    DB=$WS/a.sqlite
    echo "# S1 — planning: init replica a (alice's machine)" >>"$LOG"
    r init

    echo "# register actors on their NATIVE replicas (secrets never leave the"
    echo "# origin replica — a directory-synced actor is verify-only elsewhere,"
    echo "# so alice is created on a and bob on b)" >>"$LOG"
    "$BIN" --db "$DB" actor new alice --kind human >"$WS/alice.json"
    ALICE=$(jq -r .id "$WS/alice.json")
    echo "\$ rstask actor new alice --kind human   (on a)" >>"$LOG"; cat "$WS/alice.json" >>"$LOG"; echo >>"$LOG"
    DB=$WS/b.sqlite
    echo "\$ rstask init   (on b)" >>"$LOG"
    "$BIN" --db "$DB" init >>"$LOG" 2>&1
    "$BIN" --db "$DB" actor new bob --kind agent >"$WS/bob.json"
    BOB=$(jq -r .id "$WS/bob.json")
    echo "\$ rstask actor new bob --kind agent   (on b)" >>"$LOG"; cat "$WS/bob.json" >>"$LOG"; echo >>"$LOG"

    cat >"$WS/wt1-body.md" <<'EOF'
## WT-1 — hex-render event ids in `rstask task log`

Sprint-01 debt: `task log` prints `event_id`/`signature`/`pubkey` as raw byte
arrays. Render them as hex; keep everything else unchanged.

### Acceptance criteria
- AC1: hex strings, not byte arrays (absent on pre-DAG rows).
- AC2: output stays valid JSON; other fields unchanged.
- AC3: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
EOF
    echo "# file WT-1 with the Markdown spec as the task body (on a, by alice)" >>"$LOG"
    DB=$WS/a.sqlite
    "$BIN" --db "$DB" task new "WT-1: hex-render event ids in task log" \
        --body "$(cat "$WS/wt1-body.md")" --actor "$ALICE" >"$WS/wt1.json"
    WT1=$(jq -r .id "$WS/wt1.json")
    echo "\$ rstask task new \"WT-1: ...\" --body @wt1-body.md --actor alice   (on a)" >>"$LOG"
    cat "$WS/wt1.json" >>"$LOG"; echo >>"$LOG"

    echo "# the creation is already a signed DAG event" >>"$LOG"
    r task show "$WT1"
    r task log "$WT1"

    echo "# first exchange: actor directories cross; WT-1 reaches replica b" >>"$LOG"
    DB=$WS/a.sqlite
    "$BIN" --db "$DB" sync export >"$WS/a1.bundle" 2>>"$LOG"
    echo "\$ rstask sync export   (on a) > a1.bundle" >>"$LOG"
    DB=$WS/b.sqlite
    r sync import <"$WS/a1.bundle"
    "$BIN" --db "$DB" sync export >"$WS/b1.bundle"
    DB=$WS/a.sqlite
    echo "\$ rstask sync export   (on b) > b1.bundle; rstask sync import < b1.bundle   (on a)" >>"$LOG"
    r sync import <"$WS/b1.bundle"

    printf 'ALICE=%s\nBOB=%s\nWT1=%s\n' "$ALICE" "$BOB" "$WT1" >"$IDS"
}

claim() {
    . "$IDS"
    LOG=$EV/20-claim.txt; : >"$LOG"

    echo "# S2 — exclusive claim: bob takes WT-1 on replica b" >>"$LOG"
    DB=$WS/b.sqlite
    r task claim "$WT1" --actor "$BOB"
    r board

    echo "# exchange b->a so alice's replica learns the claim; her claim attempt"
    echo "# then hits the atomic lock (ClaimConflict)" >>"$LOG"
    "$BIN" --db "$DB" sync export >"$WS/b1c.bundle"
    DB=$WS/a.sqlite
    echo "\$ rstask sync export   (on b) > b1c.bundle; rstask sync import < b1c.bundle   (on a)" >>"$LOG"
    r sync import <"$WS/b1c.bundle"
    r task claim "$WT1" --actor "$ALICE"

    echo "# S4 gate probe (part 1): done is unreachable from work" >>"$LOG"
    r task approve "$WT1" --actor "$ALICE"
}

ttl() {
    . "$IDS"
    LOG=$EV/21-ttl.txt; : >"$LOG"
    DB=$WS/a.sqlite

    echo "# S2b — TTL expiry: alice claims a probe task with a 1s TTL and goes silent" >>"$LOG"
    "$BIN" --db "$DB" task new "WT-2: ttl probe" --actor "$ALICE" >"$WS/wt2.json"
    WT2=$(jq -r .id "$WS/wt2.json")
    printf 'WT2=%s\n' "$WT2" >>"$IDS"
    echo "\$ rstask task new \"WT-2: ttl probe\" --actor alice   (on a)" >>"$LOG"
    cat "$WS/wt2.json" >>"$LOG"; echo >>"$LOG"
    r task claim "$WT2" --actor "$ALICE" --ttl-secs 1
    echo "# ... alice's process dies; 2s later:" >>"$LOG"
    sleep 2
    "$BIN" --db "$DB" sync export >"$WS/a2.bundle"
    DB=$WS/b.sqlite
    echo "\$ rstask sync export   (on a) > a2.bundle; rstask sync import < a2.bundle   (on b)" >>"$LOG"
    r sync import <"$WS/a2.bundle"
    echo "# bob claims the stale task on replica b — the sweep must free it" >>"$LOG"
    r task claim "$WT2" --actor "$BOB"
    "$BIN" --db "$DB" sync export >"$WS/b2.bundle"
    DB=$WS/a.sqlite
    echo "\$ rstask sync export   (on b) > b2.bundle; rstask sync import < b2.bundle   (on a)" >>"$LOG"
    r sync import <"$WS/b2.bundle"
}

deliver() {
    . "$IDS"
    LOG=$EV/30-delivery.txt; : >"$LOG"

    echo "# S3 — delivery: the WT-1 code change in this repo" >>"$LOG"
    echo "\$ git diff --stat" >>"$LOG"
    git -C "$REPO" diff --stat >>"$LOG" 2>&1
    echo >>"$LOG"
    echo "\$ git diff -- crates/rstask-cli/src/main.rs" >>"$LOG"
    git -C "$REPO" diff -- crates/rstask-cli/src/main.rs >>"$LOG" 2>&1
    echo >>"$LOG"

    echo "# AC3 gate" >>"$LOG"
    echo "\$ cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace" >>"$LOG"
    {
        cd "$REPO" &&
            cargo fmt --check &&
            cargo clippy --workspace --all-targets -- -D warnings &&
            cargo test --workspace
    } >>"$LOG" 2>&1
    echo "(gate exit $?)" >>"$LOG"; echo >>"$LOG"

    echo "# AC1/AC2: post-fix rendering of the signed creation event" >>"$LOG"
    DB=$WS/a.sqlite
    r task log "$WT1"

    echo "# bob submits WT-1 for review on replica b" >>"$LOG"
    DB=$WS/b.sqlite
    r task submit "$WT1" --actor "$BOB"
    "$BIN" --db "$DB" sync export >"$WS/b3.bundle"
    DB=$WS/a.sqlite
    echo "\$ rstask sync export   (on b) > b3.bundle; rstask sync import < b3.bundle   (on a)" >>"$LOG"
    r sync import <"$WS/b3.bundle"
}

review() {
    . "$IDS"
    LOG=$EV/40-review.txt; : >"$LOG"
    DB=$WS/a.sqlite

    echo "# S4 — review: alice approves WT-1 (review -> done)" >>"$LOG"
    r task approve "$WT1" --actor "$ALICE"
    r task show "$WT1"

    LOG=$EV/41-selfapprove-probe.txt; : >"$LOG"
    echo "# S4 probe: can bob approve his OWN work? (PRD: agents can't self-approve)" >>"$LOG"
    echo "# WT-2 is held by bob (claimed after alice's TTL expiry); he submits, then approves." >>"$LOG"
    DB=$WS/b.sqlite
    r task submit "$WT2" --actor "$BOB"
    r task show "$WT2"
    echo "# -> the attempt itself:" >>"$LOG"
    r task approve "$WT2" --actor "$BOB"
    r task show "$WT2"
}

sync_final() {
    . "$IDS"
    LOG=$EV/50-sync.txt; : >"$LOG"

    echo "# S5 — full convergence: exchange both ways, then re-exchange (must move nothing)" >>"$LOG"
    DB=$WS/a.sqlite
    "$BIN" --db "$DB" sync export >"$WS/a4.bundle"
    DB=$WS/b.sqlite
    echo "\$ rstask sync export   (on a) > a4.bundle; rstask sync import < a4.bundle   (on b)" >>"$LOG"
    r sync import <"$WS/a4.bundle"
    "$BIN" --db "$DB" sync export >"$WS/b4.bundle"
    DB=$WS/a.sqlite
    echo "\$ rstask sync export   (on b) > b4.bundle; rstask sync import < b4.bundle   (on a)" >>"$LOG"
    r sync import <"$WS/b4.bundle"

    echo "# sync status must be identical on both replicas" >>"$LOG"
    r sync status
    DB=$WS/b.sqlite
    r sync status

    echo "# re-exchange moves nothing" >>"$LOG"
    "$BIN" --db "$DB" sync export >"$WS/b5.bundle"
    DB=$WS/a.sqlite
    r sync import <"$WS/b5.bundle"

    LOG=$EV/60-final.txt; : >"$LOG"
    echo "# S6 — final board and the immutable chain" >>"$LOG"
    DB=$WS/a.sqlite
    r board
    echo "# full event chain of WT-1 (kinds in Lamport order):" >>"$LOG"
    "$BIN" --db "$DB" task log "$WT1" | jq '[.[] | {kind, lamport, actor: .actor_id[0:8]}]' >>"$LOG" 2>&1
    echo >>"$LOG"
    DB=$WS/b.sqlite
    echo "# same chain on replica b:" >>"$LOG"
    "$BIN" --db "$DB" task log "$WT1" | jq '[.[] | {kind, lamport, actor: .actor_id[0:8]}]' >>"$LOG" 2>&1
}

case "${1:-}" in
plan) plan ;;
claim) claim ;;
ttl) ttl ;;
deliver) deliver ;;
review) review ;;
sync) sync_final ;;
all) plan && claim && ttl && deliver && review && sync_final ;;
*) echo "usage: ./run.sh {plan|claim|ttl|deliver|review|sync|all}" >&2; exit 2 ;;
esac

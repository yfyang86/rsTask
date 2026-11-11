#!/usr/bin/env bash
# Sprint 02 UAT — regenerates uat-evidence/. Run from anywhere: ./uat.sh
# Loopback only, relay disabled: no third-party service is contacted.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
cargo build -q -p rstask-cli --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/debug/rstask; W=$(mktemp -d); EV=$HERE/uat-evidence
export RUST_BACKTRACE=0 RSTASK_P2P_RELAY=disabled
a() { RSTASK_P2P_TOKEN=workspace-token "$BIN" --db "$W/a/w.sqlite" "$@"; }
b() { RSTASK_P2P_TOKEN=workspace-token "$BIN" --db "$W/b/w.sqlite" "$@"; }
c() { RSTASK_P2P_TOKEN=some-other-token "$BIN" --db "$W/c/w.sqlite" "$@"; }
id() { jq -r .id; }
SED=()
name() { SED+=(-e "s/$1/$2/g"); }   # show LABEL instead of ID in evidence
names() { sed "${SED[@]}" -E -e 's/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}/<id>/g' \
  -e 's/127\.0\.0\.1:[0-9]+/127.0.0.1:<port>/g' -e 's/"last_seen":"[^"]+"/"last_seen":"<time>"/'; }
board() { "$1" task list | awk 'NR>1{print $2, $4, $5, $6}' | sort; }

ALICE=$(a actor new alice --kind human | id); name "$ALICE" alice
BOT=$(a actor new bot --kind agent | id); name "$BOT" bot
T=$(a task new "shared task" --actor "$ALICE" | id)

# A serves its workspace for the whole run.
# Started directly, not through a(): $! must be the listener itself, not a subshell.
RSTASK_P2P_TOKEN=workspace-token "$BIN" --db "$W/a/w.sqlite" p2p listen --bind 127.0.0.1:0 \
  >"$W/listen.out" 2>"$W/listen.err" &
LISTENER=$!
trap 'kill -TERM $LISTENER 2>/dev/null; wait $LISTENER 2>/dev/null' EXIT
until [ -s "$W/listen.out" ]; do sleep 0.1; done
SPEC=$(jq -r .spec "$W/listen.out"); NODE_A=$(jq -r .node "$W/listen.out"); name "$NODE_A" NODE_A

{ echo "# U1 — in-process two-node sync (automated)"
  cargo test -p rstask-core --features net --manifest-path "$ROOT/Cargo.toml" --test net 2>&1 \
    | grep -E "^test |test result" | sort | sed -E 's/; finished in .*//'
} >"$EV/u1-in-process.txt" 2>&1

{ echo "# U2 — loopback CLI sync"
  echo "\$ A: p2p listen --bind 127.0.0.1:0"; cat "$W/listen.out"
  b init >/dev/null
  echo "\$ B (empty): p2p sync <A's spec>"; b p2p sync "$SPEC" | jq -c '{received: .ok.received.added, sent: .ok.sent}'
  mkdir -p "$W/b/keys"; cp "$W/a/keys/$BOT.key" "$W/b/keys/"
  a task claim "$T" --actor "$ALICE" >/dev/null; a task submit "$T" --actor "$ALICE" --summary done >/dev/null
  for i in 1 2 3; do b task new "from b $i" --actor "$BOT" >/dev/null; done
  echo "diverged: A has $(board a | wc -l) tasks, B has $(board b | wc -l)"
  echo "\$ B: p2p sync            (no argument: every known peer)"; b p2p sync | jq -c '{received: .ok.received.added, sent: .ok.sent, refused_by_peer: .ok.refused_by_peer}'
  echo "A board:"; board a | sed 's/^/  /'
  echo "B board:"; board b | sed 's/^/  /'
  echo "boards identical: $([ "$(board a)" = "$(board b)" ] && echo yes || echo NO)   heads identical: $([ "$(a sync status | jq -c .heads)" = "$(b sync status | jq -c .heads)" ] && echo yes || echo NO)"
  echo "\$ B: p2p sync again"; b p2p sync | jq -c '{received: .ok.received.added, sent: .ok.sent}'
  NODE_B=$(b p2p status | jq -r .node); name "$NODE_B" NODE_B
  echo "\$ B: p2p status"; b p2p status | jq -c .
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u2-loopback-cli.txt"

{ echo "# U3 — LAN sync by hand"
  echo "NOT RUN: this environment is a single container with no second host."
  echo "To run: host A  \`RSTASK_P2P_TOKEN=… rstask p2p listen --bind 0.0.0.0:7701\`,"
  echo "        host B  \`RSTASK_P2P_TOKEN=… rstask p2p sync <ID>@<A's LAN address>:7701\`."
} >"$EV/u3-lan-not-run.txt"

{ echo "# U4 — wrong token refused"
  CAROL=$(c actor new carol --kind human | id)
  c task new "another workspace" --actor "$CAROL" >/dev/null
  before_a=$(a sync status | jq -c '{events}'); before_c=$(c sync status | jq -c '{events}')
  echo "\$ C (a different token): p2p sync <A's spec>"; c p2p sync "$SPEC" 2>&1 | grep -v '^$' | head -3; echo "exit=${PIPESTATUS[0]}"
  echo "A before/after: $before_a / $(a sync status | jq -c '{events}')   C before/after: $before_c / $(c sync status | jq -c '{events}')"
  echo "\$ no token at all: p2p sync"; RSTASK_P2P_TOKEN= "$BIN" --db "$W/b/w.sqlite" p2p sync "$SPEC" 2>&1 | head -1
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u4-wrong-token.txt"

{ echo "# U5 — unknown peer"
  NOBODY=$(c p2p status | jq -r .node); name "$NOBODY" NODE_NOBODY
  before=$(b sync status | jq -c '{events}')
  start=$(date +%s)
  echo "\$ B: p2p sync <an id nobody serves>@127.0.0.1:9"; b p2p sync "$NOBODY@127.0.0.1:9" 2>&1 | grep -v '^$' | head -3; echo "exit=${PIPESTATUS[0]}"
  echo "gave up within 15s: $([ $(( $(date +%s) - start )) -le 15 ] && echo yes || echo NO)   B before/after: $before / $(b sync status | jq -c '{events}')"
  echo "\$ B: p2p sync not-an-id"; b p2p sync not-an-id 2>&1 | head -1
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u5-unknown-peer.txt"

tail -n +1 "$EV"/u*.txt

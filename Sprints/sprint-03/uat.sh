#!/usr/bin/env bash
# Sprint 03 UAT — regenerates uat-evidence/. Run from anywhere: ./uat.sh
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
cargo build -q -p rstask-cli --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/debug/rstask; W=$(mktemp -d); EV=$HERE/uat-evidence
export RUST_BACKTRACE=0
a() { "$BIN" --db "$W/a.sqlite" "$@"; }
b() { "$BIN" --db "$W/b.sqlite" "$@"; }
id() { jq -r .id; }
status() { "$1" task show "$2" | jq -r .status; }
kinds() { "$1" task log "$2" | jq -c '[.[].kind]'; }
sync_ab() { a sync export | b sync import >/dev/null; }
sync_ba() { b sync export | a sync import >/dev/null; }

ALICE=$(a actor new alice --kind human | id)
BOT=$(a actor new bot --kind agent | id)
BOT2=$(a actor new bot2 --kind agent | id)
b init >/dev/null
names() { sed -e "s/$ALICE/alice/g" -e "s/$BOT2/bot2/g" -e "s/$BOT/bot/g" -e "s/${CAROL:-@}/carol/g"; }

{ echo "# U1 — reject -> re-claim replicates"
  T=$(a task new "u1 rework" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" >/dev/null; a task submit "$T" --actor "$BOT" >/dev/null
  a task reject "$T" --actor "$ALICE" --reason rework >/dev/null; a task claim "$T" --actor "$BOT2" >/dev/null
  sync_ab
  echo "A status: $(status a "$T")"; echo "B status: $(status b "$T")"
  echo "B log:    $(kinds b "$T")"
} 2>&1 | names >"$EV/u1-reject-reclaim.txt"

{ echo "# U2 — disconnected reviewers converge"
  T=$(a task new "u2 contested" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" >/dev/null; a task submit "$T" --actor "$BOT" >/dev/null
  CAROL=$(a actor new carol --kind human | id); sync_ab
  a task approve "$T" --actor "$ALICE" >/dev/null; b task reject "$T" --actor "$CAROL" --reason rework >/dev/null
  echo "before sync  A: $(status a "$T")  B: $(status b "$T")"
  sync_ab; sync_ba
  echo "after sync   A: $(status a "$T")  B: $(status b "$T")"
  echo "A log: $(kinds a "$T")"; echo "B log: $(kinds b "$T")"
} 2>&1 | names >"$EV/u2-concurrent-verdicts.txt"

{ echo "# U3 — no agent self-approval"
  T=$(a task new "u3 self" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" >/dev/null; a task submit "$T" --actor "$BOT" >/dev/null
  echo "\$ task approve --actor bot"; a task approve "$T" --actor "$BOT"; echo "exit=$?"
  echo "status: $(status a "$T")   log: $(kinds a "$T")"
  echo "\$ task approve --actor bot2"; a task approve "$T" --actor "$BOT2" | jq -r .status
} 2>&1 | names >"$EV/u3-self-approval.txt"

{ echo "# U4 — human force-release"
  T=$(a task new "u4 hung" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" >/dev/null; sync_ab
  echo "\$ task release --force --actor bot2"; a task release "$T" --actor "$BOT2" --force; echo "exit=$?"
  echo "\$ task release --force --actor alice"; a task release "$T" --actor "$ALICE" --force; echo "exit=$?"
  sync_ab
  echo "A status: $(status a "$T")"; echo "B status: $(status b "$T")"
  b task log "$T" | jq -c '.[] | select(.kind=="claim.force_released") | {kind, actor_id, payload}'
} 2>&1 | names >"$EV/u4-force-release.txt"

tail -n +1 "$EV"/u*.txt

#!/usr/bin/env bash
# Sprint 04 UAT — regenerates uat-evidence/. Run from anywhere: ./uat.sh
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
payload() { "$1" task log "$2" | jq -c --arg k "$3" '.[] | select(.kind==$k) | .payload | fromjson'; }
sync_ab() { a sync export | b sync import >/dev/null; }
sync_ba() { b sync export | a sync import >/dev/null; }

ALICE=$(a actor new alice --kind human | id)
BOT=$(a actor new bot --kind agent | id)
b init >/dev/null; sync_ab
RIVAL=$(a actor new rival --kind agent | id); sync_ab
names() { sed -e "s/$ALICE/alice/g" -e "s/$BOT/bot/g" -e "s/$RIVAL/rival/g" \
  -E -e 's/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}/<id>/g'; }

{ echo "# U1 — renewal keeps the claim, everywhere"
  T=$(a task new "u1 long job" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" --ttl-secs 2 >/dev/null; sync_ab
  echo "\$ A: task renew --actor bot --ttl-secs 900"; a task renew "$T" --actor "$BOT" --ttl-secs 900 | jq -c '{actor_id, released_at}'
  sleep 2.2
  echo "\$ B (has not seen the renewal): task claim --actor rival"; b task claim "$T" --actor "$RIVAL" | jq -c '{actor_id}'
  sync_ab; sync_ba
  echo "A: status=$(status a "$T")  holder renews again -> $(a task renew "$T" --actor "$BOT" | jq -c '{actor_id}')"
  echo "B: status=$(status b "$T")  rival submits -> $(b task submit "$T" --actor "$RIVAL" 2>&1 | head -1)"
  echo "A log: $(kinds a "$T")"; echo "B log: $(kinds b "$T")"
} 2>&1 | names >"$EV/u1-renewal.txt"

{ echo "# U2 — a stale claim leaves the board on read"
  T=$(a task new "u2 abandoned" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" --ttl-secs 1 >/dev/null
  echo "status right after claim: $(status a "$T")"; sleep 1.2
  echo "status after 1.2s, read only: $(status a "$T")"
  echo "\$ task renew --actor bot"; a task renew "$T" --actor "$BOT"; echo "exit=$?"
  echo "\$ task submit --actor bot"; a task submit "$T" --actor "$BOT"; echo "exit=$?"
  echo "log: $(kinds a "$T")"
} 2>&1 | names >"$EV/u2-stale-on-read.txt"

{ echo "# U3 — only the holder submits"
  T=$(a task new "u3 held" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" >/dev/null
  RIVAL_A=$(a actor new rival-a --kind agent | id)
  echo "\$ task submit --actor rival-a"; a task submit "$T" --actor "$RIVAL_A"; echo "exit=$?"
  echo "status: $(status a "$T")   log: $(kinds a "$T")"
} 2>&1 | names >"$EV/u3-holder-submits.txt"

{ echo "# U4 — results travel with the submission, reasons with the rejection"
  T=$(a task new "u4 handoff" --actor "$ALICE" | id)
  a task claim "$T" --actor "$BOT" >/dev/null
  a task submit "$T" --actor "$BOT" --summary "fold now derives status" \
    --ref c265568 --ref crates/rstask-core/src/store.rs --evidence "cargo test: 33/33 green" >/dev/null
  echo "\$ task reject --actor alice   (no --reason)"; a task reject "$T" --actor "$ALICE" 2>&1 | head -2; echo "exit=${PIPESTATUS[0]}"
  a task reject "$T" --actor "$ALICE" --reason "add a test for the tie-break" >/dev/null
  sync_ab
  echo "B reads task.submitted:         $(payload b "$T" task.submitted | jq -c 'del(.claim_id)')"
  echo "B reads task.changes_requested: $(payload b "$T" task.changes_requested)"
  echo "B status: $(status b "$T")"
} 2>&1 | names >"$EV/u4-handoff.txt"

tail -n +1 "$EV"/u*.txt

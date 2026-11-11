#!/usr/bin/env bash
# Sprint 07 UAT — regenerates uat-evidence/. Run from anywhere: ./uat.sh
# Optional: OLD_BIN=/path/to/a/sprint-06/rstask ./uat.sh also regenerates the
# legacy-workspace check (u1b); without it that file is left as recorded.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
cargo build -q -p rstask-cli -p rstask-mcp --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/debug/rstask; MCP=$ROOT/target/debug/rstask-mcp
W=$(mktemp -d); EV=$HERE/uat-evidence
export RUST_BACKTRACE=0
# Replica A and B live in separate directories, so each has its own key directory.
a() { "$BIN" --db "$W/a/w.sqlite" "$@"; }
b() { "$BIN" --db "$W/b/w.sqlite" "$@"; }
c() { "$BIN" --db "$W/c/w.sqlite" "$@"; }
id() { jq -r .id; }
status() { "$1" task show "$2" | jq -r .status; }
sync_ab() { a sync export | b sync import >/dev/null; }
sync_ba() { b sync export | a sync import >/dev/null; }
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"uat","version":"0"}}}'
READY='{"jsonrpc":"2.0","method":"notifications/initialized"}'
# mcp KEYDIR ACTOR TOOL [ARGS] -> the tool's JSON, or "ERROR: <text>" (replica A)
mcp() {
  printf '%s\n' "$INIT" "$READY" "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$3\",\"arguments\":${4:-{\}}}}" \
    | "$MCP" --db "$W/a/w.sqlite" --keys "$1" --actor "$2" | jq -r 'select(.id==2) | .result | if .isError then "ERROR: " + .content[0].text else .content[0].text end'
}
SED=()
name() { SED+=(-e "s/$1/$2/g"); }   # show LABEL instead of ID in evidence
names() { sed "${SED[@]}" -E -e 's/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}/<id>/g' -e "s#$W#<tmp>#g"; }
# hexfind FILE... HEX -> how many of the files contain these bytes
hexfind() { local hex=${*: -1} n=0 f; for f in "${@:1:$#-1}"; do [ -f "$f" ] && od -An -v -tx1 "$f" | tr -d ' \n' | grep -q "$hex" && n=$((n+1)); done; echo "$n"; }

ALICE=$(a actor new alice --kind human | id); name "$ALICE" alice
BOT=$(a actor new bot --kind agent | id); name "$BOT" bot

{ echo "# U1 — keys live outside the workspace database"
  echo "key directory: $(ls "$W/a/keys" | names | tr '\n' ' ')"
  for who in ALICE BOT; do
    secret=$(cat "$W/a/keys/${!who}.key")
    echo "database files holding ${who,,}'s secret (raw bytes or hex text): $(hexfind "$W"/a/w.sqlite* "$secret") + $(grep -l -a "$secret" "$W"/a/w.sqlite* 2>/dev/null | wc -l)"
  done
  mkdir "$W/bot-keys"; mv "$W/a/keys/$BOT.key" "$W/bot-keys/"
  echo "bot's key moved to its own directory; the server is started with --keys <that directory>"
  echo "as bot:   task_create -> $(mcp "$W/bot-keys" "$BOT" task_create '{"title":"made by bot"}' | jq -r '"created \"" + .title + "\""')"
  echo "as alice: task_create -> $(mcp "$W/bot-keys" "$ALICE" task_create '{"title":"forged"}')"
  echo "as alice: task_approve -> $(mcp "$W/bot-keys" "$ALICE" task_approve '{"id":"x","submission":"y"}')"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u1-keys.txt"

if [ -n "${OLD_BIN:-}" ]; then
  { echo "# U1b — a workspace created by the sprint-06 binary"
    old() { "$OLD_BIN" --db "$W/old/w.sqlite" "$@"; }; new() { "$BIN" --db "$W/old/w.sqlite" "$@"; }
    OA=$(old actor new alice --kind human | id); name "$OA" alice
    OT=$(old task new "made before sprint 07" --actor "$OA" | id)
    mkdir "$W/old-copy"; cp "$W"/old/w.sqlite* "$W/old-copy/"
    echo "\$ new binary: task list"; new task list
    secret=$(cat "$W/old/keys/$OA.key")
    echo "key directory after first open: $(ls "$W/old/keys" | tr '\n' ' ')"
    echo "database files holding alice's secret: before $(hexfind "$W"/old-copy/w.sqlite* "$secret"), after $(hexfind "$W"/old/w.sqlite* "$secret")"
    echo "\$ new binary: task claim (alice still signs)"; new task claim "$OT" --actor "$OA" | jq -c '{actor_id}'
    echo "\$ new binary: actor new bot (provisioned by the oldest human)"; new actor new bot --kind agent | jq -c '{name, kind}'
  } >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u1b-legacy-workspace.txt"
fi

{ echo "# U2 — actors are provisioned by a human, starting from one root"
  echo "\$ (fresh workspace) actor new bot --kind agent"; c actor new bot --kind agent 2>&1 | head -1
  echo "\$ actor new minion --by bot            (bot's key directory)"; a --keys "$W/bot-keys" actor new minion --kind agent --by "$BOT" 2>&1 | head -1
  echo "\$ actor new minion                     (bot's key directory)"; a --keys "$W/bot-keys" actor new minion --kind agent 2>&1 | head -1
  CAROL=$(a actor new carol --kind human | id); name "$CAROL" carol
  echo "\$ actor new carol --kind human         (alice's key directory)"
  a task log "$CAROL" | jq -c '.[] | {kind, signed_by: .actor_id, payload: (.payload | fromjson | del(.pubkey))}'
  MALLORY=$(c actor new mallory --kind human | id); name "$MALLORY" mallory
  c task new "from another workspace" --actor "$MALLORY" >/dev/null
  echo "\$ import a bundle from a workspace rooted at mallory"
  c sync export | a sync import | jq -c '{added, rejected, reasons: [.rejected_ids[] | sub("^[0-9a-f]+: "; "")]}'
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u2-provisioning.txt"

{ echo "# U3 — authority holds on merge (automated)"
  cargo test -p rstask-core --manifest-path "$ROOT/Cargo.toml" --test authority 2>&1 | grep -E "^test |test result" | sort | sed -E 's/; finished in .*//'
} >"$EV/u3-authority-on-merge.txt" 2>&1

WORKER=$(a actor new worker --kind agent --role worker | id); name "$WORKER" worker
REV=$(a actor new rev --kind agent --role reviewer --role planner | id); name "$REV" rev

{ echo "# U4 — an approval is bound to the submission that was reviewed"
  T=$(a task new "u4 feature" --actor "$ALICE" | id)
  a task claim "$T" --actor "$WORKER" >/dev/null
  a task submit "$T" --actor "$WORKER" --summary "first try" >/dev/null
  b init >/dev/null; sync_ab; mkdir -p "$W/b/keys"; cp "$W/a/keys/$REV.key" "$W/b/keys/"
  S1=$(b task show "$T" | jq -r .submission.id); name "$S1" S1
  echo "B: rev reads the task: submission $(b task show "$T" | jq -c '.submission | {id, summary}')"
  a task reject "$T" --actor "$ALICE" --reason "not yet" >/dev/null
  a task claim "$T" --actor "$WORKER" >/dev/null
  a task submit "$T" --actor "$WORKER" --summary "second try" >/dev/null
  S2=$(a task show "$T" | jq -r .submission.id); name "$S2" S2
  echo "A: alice rejects; worker resubmits: submission $(a task show "$T" | jq -c '.submission | {id, summary}')"
  for i in 1 2 3 4; do b task new "busy $i" --actor "$REV" >/dev/null; done
  b task approve "$T" --actor "$REV" --submission "$S1" >/dev/null
  echo "B: rev, busy elsewhere meanwhile, approves S1"
  echo "before sync  A: $(status a "$T")  B: $(status b "$T")"
  sync_ab; sync_ba
  echo "after sync   A: $(status a "$T")  B: $(status b "$T")"
  echo "\$ A: task approve --submission S1"; a task approve "$T" --actor "$ALICE" --submission "$S1" 2>&1 | head -1
  echo "\$ A: task approve --submission S2 -> $(a task approve "$T" --actor "$ALICE" --submission "$S2" | jq -r .status)"
  echo "approvals in the log: $(a task log "$T" | jq -c '[.[] | select(.kind=="task.approved") | {by: .actor_id, submission: (.payload | fromjson | .submission)}]')"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u4-bound-approval.txt"

{ echo "# U5 — protected tasks need a human"
  P=$(a task new "u5 change the gate" --actor "$ALICE" | id)
  echo "\$ task protect -> $(a task protect "$P" --actor "$ALICE" | jq -c '{title, protected}')"
  a task claim "$P" --actor "$WORKER" >/dev/null; a task submit "$P" --actor "$WORKER" --summary "gate changed" >/dev/null
  S=$(a task show "$P" | jq -r .submission.id)
  echo "rev (agent, reviewer role) task_approve -> $(mcp "$W/a/keys" "$REV" task_approve "{\"id\":\"$P\",\"submission\":\"$S\"}")"
  echo "status: $(status a "$P")"
  echo "alice task approve -> $(a task approve "$P" --actor "$ALICE" --submission "$S" | jq -r .status)"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u5-protected.txt"

tail -n +1 "$EV"/u*.txt

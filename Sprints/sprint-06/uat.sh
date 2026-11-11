#!/usr/bin/env bash
# Sprint 06 UAT — regenerates uat-evidence/. Run from anywhere: ./uat.sh
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
cargo build -q -p rstask-cli -p rstask-mcp --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/debug/rstask; MCP=$ROOT/target/debug/rstask-mcp
W=$(mktemp -d); EV=$HERE/uat-evidence
export RUST_BACKTRACE=0
DB=$W/a.sqlite
r() { "$BIN" --db "$DB" "$@"; }
b() { "$BIN" --db "$W/b.sqlite" "$@"; }
id() { jq -r .id; }
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"uat","version":"0"}}}'
READY='{"jsonrpc":"2.0","method":"notifications/initialized"}'
# mcp ACTOR TOOL [ARGS] -> the tool's JSON, or "ERROR: <text>"
mcp() {
  printf '%s\n' "$INIT" "$READY" "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$2\",\"arguments\":${3:-{\}}}}" \
    | "$MCP" --db "$DB" --actor "$1" | jq -r 'select(.id==2) | .result | if .isError then "ERROR: " + .content[0].text else .content[0].text end'
}
# next ACTOR -> title of the task claim_next hands out (or "nothing claimable")
next() { r_json=$(mcp "$1" claim_next); echo "$r_json" | jq -r 'if . == null then "nothing claimable" else .title end' 2>/dev/null || echo "$r_json"; }
# finish TASK WORKER -> submit and have alice approve
finish() { r task submit "$1" --actor "$2" >/dev/null; r task approve "$1" --actor "$ALICE" >/dev/null; }

ALICE=$(r actor new alice --kind human | id)
BOT=$(r actor new bot --kind agent | id)
SED=()
name() { SED+=(-e "s/$1/$2/g"); }   # name ID LABEL: show LABEL instead of ID in evidence
name "$ALICE" alice; name "$BOT" bot
names() { sed "${SED[@]}" -E -e 's/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}/<id>/g'; }

{ echo "# U1 — a blocked task waits for its blocker"
  DEPLOY=$(r task new deploy --actor "$ALICE" | id); name "$DEPLOY" deploy
  BUILD=$(r task new build --actor "$ALICE" | id); name "$BUILD" build
  mcp "$ALICE" task_update "{\"id\":\"$DEPLOY\",\"priority\":9}" >/dev/null
  r task block "$DEPLOY" --by "$BUILD" --actor "$ALICE"
  echo "claim_next -> $(next "$BOT")"
  echo "\$ A: task claim deploy"; r task claim "$DEPLOY" --actor "$BOT" 2>&1 | head -1
  BOB=$(r actor new bob --kind agent | id); name "$BOB" bob
  b init >/dev/null; r sync export | b sync import >/dev/null
  echo "\$ B (imported the bundle): task claim deploy"; b task claim "$DEPLOY" --actor "$BOB" 2>&1 | head -1
  echo "task show deploy -> $(r task show "$DEPLOY" | jq -c '{status, priority, blocked_by, waiting_on}')"
  finish "$BUILD" "$BOT"
  echo "build is done; claim_next -> $(next "$BOT")"
  echo "task show deploy -> $(r task show "$DEPLOY" | jq -c '{status, blocked_by, waiting_on}')"
  finish "$DEPLOY" "$BOT"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u1-blocked-by.txt"

{ echo "# U2 — a parent waits for its subtasks"
  EPIC=$(r task new epic --actor "$ALICE" | id); name "$EPIC" epic
  P1=$(r task new "part 1" --parent "$EPIC" --actor "$ALICE" | id); name "$P1" part-1
  P2=$(r task new "part 2" --parent "$EPIC" --actor "$ALICE" | id); name "$P2" part-2
  echo "task show epic -> $(r task show "$EPIC" | jq -c '{status, children, waiting_on}')"
  echo "claim_next -> $(next "$BOT")"; finish "$P1" "$BOT"
  echo "claim_next -> $(next "$BOT")"; finish "$P2" "$BOT"
  echo "task show epic -> $(r task show "$EPIC" | jq -c '{status, children, waiting_on}')"
  echo "claim_next -> $(next "$BOT")"; finish "$EPIC" "$BOT"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u2-subtasks.txt"

{ echo "# U3 — dependencies must make sense"
  A=$(r task new a --actor "$ALICE" | id); name "$A" a
  B=$(r task new b --actor "$ALICE" | id); name "$B" b
  C=$(r task new c --actor "$ALICE" | id); name "$C" c
  echo "\$ task block a --by a";       r task block "$A" --by "$A" --actor "$ALICE" 2>&1 | head -1
  echo "\$ task block a --by nosuch";  r task block "$A" --by nosuch --actor "$ALICE" 2>&1 | head -1
  r task block "$A" --by "$B" --actor "$ALICE"; r task block "$B" --by "$C" --actor "$ALICE"
  echo "\$ task block c --by a   (a waits on b, b waits on c)"; r task block "$C" --by "$A" --actor "$ALICE" 2>&1 | head -1
  echo "c log: $(r task log "$C" | jq -c '[.[].kind]')"
  echo "\$ task claim a";              r task claim "$A" --actor "$BOT" 2>&1 | head -1
  r task unblock "$A" --by "$B" --actor "$ALICE"
  echo "\$ task unblock a --by b; task claim a -> $(r task claim "$A" --actor "$BOT" | jq -r .actor_id)"
  echo "a log: $(r task log "$A" | jq -c '[.[].kind]')"
  r task release "$A" --actor "$BOT"
  for t in "$C" "$B" "$A"; do r task claim "$t" --actor "$BOT" >/dev/null; finish "$t" "$BOT"; done
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u3-sane-dependencies.txt"

{ echo "# U4 — roles route the work"
  PLANNER=$(r actor new planner --kind agent --role planner | id); name "$PLANNER" planner
  WORKER=$(r actor new worker --kind agent --role worker | id); name "$WORKER" worker
  REVIEWER=$(r actor new reviewer --kind agent --role reviewer | id); name "$REVIEWER" reviewer
  printf '%s\n' "$INIT" | "$MCP" --db "$DB" --actor "$WORKER" | jq -r '.result.instructions' | cut -d. -f1-2
  short() { sed -E -e 's/^ERROR: actor [^ ]+ is not permitted to .*\(needs role ([a-z]+)\)$/refused: needs role \1/' ; }
  for who in PLANNER WORKER REVIEWER; do
    echo "${who,,} task_create  -> $(mcp "${!who}" task_create '{"title":"u4 job"}' | jq -r '"created " + .title' 2>/dev/null || mcp "${!who}" task_create '{"title":"u4 job"}' | short)"
  done
  for who in PLANNER WORKER REVIEWER; do
    echo "${who,,} claim_next   -> $(mcp "${!who}" claim_next | jq -r '"claimed " + .title' 2>/dev/null || mcp "${!who}" claim_next | short)"
  done
  T=$(mcp "$WORKER" tasks_list '{"status":"work"}' | jq -r '.[0].id')
  mcp "$WORKER" task_complete "{\"id\":\"$T\",\"summary\":\"done\"}" >/dev/null
  SUB=$(r task show "$T" | jq -r .submission.id)
  for who in PLANNER WORKER REVIEWER; do
    echo "${who,,} task_approve -> $(mcp "${!who}" task_approve "{\"id\":\"$T\",\"submission\":\"$SUB\"}" | jq -r '"approved, status " + .status' 2>/dev/null || mcp "${!who}" task_approve "{\"id\":\"$T\",\"submission\":\"$SUB\"}" | short)"
  done
  echo "actor created without --role: $(r actor new jack --kind agent | jq -c .roles)"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u4-roles.txt"

tail -n +1 "$EV"/u*.txt

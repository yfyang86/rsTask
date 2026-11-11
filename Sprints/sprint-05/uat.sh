#!/usr/bin/env bash
# Sprint 05 UAT — regenerates uat-evidence/. Run from anywhere: ./uat.sh
# Each `mcp` call starts a fresh `rstask-mcp` for one actor and speaks MCP over stdio.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
cargo build -q -p rstask-cli -p rstask-mcp --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/debug/rstask; MCP=$ROOT/target/debug/rstask-mcp
W=$(mktemp -d); DB=$W/w.sqlite; EV=$HERE/uat-evidence
export RUST_BACKTRACE=0
r() { "$BIN" --db "$DB" "$@"; }
id() { jq -r .id; }
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"uat","version":"0"}}}'
READY='{"jsonrpc":"2.0","method":"notifications/initialized"}'
# rpc ACTOR METHOD [PARAMS] -> the JSON-RPC result
rpc() {
  printf '%s\n' "$INIT" "$READY" "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"$2\",\"params\":${3:-{\}}}" \
    | "$MCP" --db "$DB" --actor "$1" | jq -c 'select(.id==2) | .result'
}
# mcp ACTOR TOOL [ARGS] -> the tool's JSON, or "ERROR: <text>"
mcp() {
  rpc "$1" tools/call "{\"name\":\"$2\",\"arguments\":${3:-{\}}}" \
    | jq -r 'if .isError then "ERROR: " + .content[0].text else .content[0].text end'
}
kinds() { r task log "$1" | jq -c --arg b "$BOT" --arg v "$REVIEWER" --arg a "$ALICE" \
  '[.[] | .kind + " by " + ({($b):"bot",($v):"reviewer",($a):"alice"}[.actor_id] // "?")]'; }

ALICE=$(r actor new alice --kind human | id)
BOT=$(r actor new bot --kind agent | id)
REVIEWER=$(r actor new reviewer --kind agent | id)
names() { sed -e "s/$ALICE/alice/g" -e "s/$BOT/bot/g" -e "s/$REVIEWER/reviewer/g" \
  -E -e 's/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}/<id>/g'; }

{ echo "# U1 — an agent finds its tools and its identity"
  printf '%s\n' "$INIT" | "$MCP" --db "$DB" --actor "$BOT" | jq -r '.result | "server: \(.serverInfo.name)\ninstructions: \(.instructions)"'
  echo "tools: $(rpc "$BOT" tools/list | jq -c '[.tools[].name] | sort')"
  echo "tools taking an actor argument: $(rpc "$BOT" tools/list | jq -c '[.tools[] | select(.inputSchema.properties.actor) | .name]')"
  echo "\$ rstask-mcp --actor nobody"; "$MCP" --db "$DB" --actor nobody </dev/null; echo "exit=$?"
} 2>&1 | names >"$EV/u1-tools-and-identity.txt"

{ echo "# U2 — claim_next hands out distinct tasks under a race"
  for t in "p0 oldest" "p5 urgent" "p1 soon"; do
    T=$(r task new "$t" --actor "$ALICE" | id)
    mcp "$BOT" task_update "{\"id\":\"$T\",\"priority\":${t:1:1}}" >/dev/null
  done
  echo "lone agent first: $(mcp "$BOT" claim_next | jq -r .title)"
  mcp "$BOT" task_release "{\"id\":\"$(mcp "$BOT" tasks_list '{"status":"work"}' | jq -r '.[0].id')\"}" >/dev/null
  AGENTS=(); for i in 1 2 3 4 5 6 7 8; do AGENTS+=("$(r actor new "racer$i" --kind agent | id)"); done
  for i in "${!AGENTS[@]}"; do
    ( echo "racer$((i+1)): $(mcp "${AGENTS[$i]}" claim_next | jq -r 'if . == null then "null" else .title end')" >"$W/race.$i" ) &
  done; wait
  cat "$W"/race.* | sort
  echo "distinct tasks handed out: $(cat "$W"/race.* | grep -v null | cut -d' ' -f2- | sort -u | wc -l)"
  echo "left in queue: $(mcp "$BOT" tasks_list '{"status":"queue"}' | jq length)"
} 2>&1 | names >"$EV/u2-claim-next-race.txt"

{ echo "# U3 — a full worker -> reviewer loop over MCP"
  T=$(r task new "u3 ship it" --body "do the thing" --actor "$ALICE" | id)
  echo "bot claim_next -> $(mcp "$BOT" claim_next | jq -c '{title, status, holder: .claim.actor_id}')"
  echo "bot task_renew -> $(mcp "$BOT" task_renew "{\"id\":\"$T\",\"ttl_secs\":1800}" | jq -c '{actor_id, release_reason}')"
  mcp "$BOT" task_update "{\"id\":\"$T\",\"body_md\":\"do the thing\\n\\nnotes: done in store.rs\"}" >/dev/null
  mcp "$BOT" task_complete "{\"id\":\"$T\",\"summary\":\"did the thing\",\"refs\":[\"abc123\"],\"evidence\":\"38/38 green\"}" >/dev/null
  echo "reviewer task_get -> $(mcp "$REVIEWER" task_get "{\"id\":\"$T\"}" | jq -c '{status, submission: (.submission | del(.at))}')"
  mcp "$REVIEWER" task_reject "{\"id\":\"$T\",\"reason\":\"add a test\"}" >/dev/null
  echo "bot claim_next again -> $(mcp "$BOT" claim_next | jq -c '{title, rejection: (.rejection | del(.at))}')"
  mcp "$BOT" task_complete "{\"id\":\"$T\",\"summary\":\"added the test\"}" >/dev/null
  echo "reviewer task_approve -> $(mcp "$REVIEWER" task_approve "{\"id\":\"$T\"}" | jq -r .status)"
  echo "log: $(kinds "$T")"
} 2>&1 | names >"$EV/u3-worker-reviewer-loop.txt"

{ echo "# U4 — the gate holds over MCP"
  T=$(r task new "u4 held" --actor "$ALICE" | id)
  mcp "$BOT" task_claim "{\"id\":\"$T\"}" >/dev/null
  echo "reviewer task_update   -> $(mcp "$REVIEWER" task_update "{\"id\":\"$T\",\"title\":\"hijack\"}")"
  echo "reviewer task_complete -> $(mcp "$REVIEWER" task_complete "{\"id\":\"$T\",\"summary\":\"mine\"}")"
  echo "reviewer task_release  -> $(mcp "$REVIEWER" task_release "{\"id\":\"$T\"}")"
  mcp "$BOT" task_complete "{\"id\":\"$T\",\"summary\":\"done\"}" >/dev/null
  echo "bot task_approve (own) -> $(mcp "$BOT" task_approve "{\"id\":\"$T\"}")"
  echo "log: $(kinds "$T")"
} 2>&1 | names >"$EV/u4-gate-over-mcp.txt"

tail -n +1 "$EV"/u*.txt

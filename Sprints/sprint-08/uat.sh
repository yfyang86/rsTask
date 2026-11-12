#!/usr/bin/env bash
# Sprint 08 UAT — regenerates uat-evidence/ (U1–U3; U4 is the PR's check runs).
# Loopback, relay disabled. A and B have separate databases and key directories.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
cargo build -q -p rstask-cli --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/debug/rstask; W=$(mktemp -d); EV=$HERE/uat-evidence
export RUST_BACKTRACE=0 RSTASK_P2P_RELAY=disabled RSTASK_P2P_TOKEN=workspace-token NO_COLOR=1
a() { "$BIN" --db "$W/a/w.sqlite" "$@"; }
b() { "$BIN" --db "$W/b/w.sqlite" "$@"; }
c() { "$BIN" --db "$W/c/w.sqlite" "$@"; }
id() { jq -r .id; }
status() { "$1" task show "$2" | jq -r .status; }
holder() { "$1" task show "$2" | jq -r '.claim.actor_id // "nobody"'; }
SED=()
name() { SED+=(-e "s/$1/$2/g"); }   # show LABEL instead of ID in evidence
names() { sed "${SED[@]}" -E -e 's/[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}/<id>/g' \
  -e 's/127\.0\.0\.1:[0-9]+/127.0.0.1:<port>/g' -e 's/\x1b\[[0-9;]*m//g' \
  -e 's/^[0-9]{4}-[0-9-]+T[0-9:.]+Z +//' -e 's/"(pubkey|proof)":"[0-9a-f]{12}[0-9a-f]+"/"\1":"<hex>"/g'; }
# wait_for SECONDS COMMAND… -> seconds it took for COMMAND to succeed, or "never"
wait_for() { local limit=$1 start=$SECONDS; shift
  until "$@" >/dev/null 2>&1; do [ $((SECONDS - start)) -ge "$limit" ] && { echo never; return; }; sleep 0.2; done
  echo "$((SECONDS - start))s"; }

ALICE=$(a actor new alice --kind human | id); name "$ALICE" alice
X=$(a actor new x --kind agent | id); name "$X" x
# Started directly, not through a(): $! must be the process itself.
"$BIN" --db "$W/a/w.sqlite" p2p listen --bind 127.0.0.1:0 >"$W/listen.out" 2>"$W/listen.err" &
LISTENER=$!; SYNCER=
trap 'kill -TERM $LISTENER $SYNCER 2>/dev/null; wait 2>/dev/null' EXIT
until [ -s "$W/listen.out" ]; do sleep 0.1; done
SPEC=$(jq -r .spec "$W/listen.out")

{ echo "# U1 — an actor joins from another machine without a secret key moving"
  T=$(a task new "u1 first job" --actor "$ALICE" | id)
  echo "\$ B (empty replica): actor request bob --kind agent --role worker"
  b actor request bob --kind agent --role worker >"$W/request.json"; cat "$W/request.json"
  BOB=$(jq -r .id "$W/request.json"); name "$BOB" bob
  echo "\$ A: actor admit '<request>'"; a actor admit "$(cat "$W/request.json")" | jq -c '{name, kind, roles}'
  echo "bob's creation event: $(a task log "$BOB" | jq -c '.[0] | {kind, signed_by: .actor_id}')"
  echo "key files — A: $(ls "$W/a/keys" | tr '\n' ' ')  B: $(ls "$W/b/keys" | tr '\n' ' ')"
  echo "\$ B: p2p sync <A>; task claim --actor bob"; b p2p sync "$SPEC" | jq -c '{received: .ok.received.added}'
  b task claim "$T" --actor "$BOB" | jq -c '{actor_id}'; b p2p sync | jq -c '{sent: .ok.sent, refused_by_peer: .ok.refused_by_peer}'
  echo "A sees the task held by: $(holder a "$T")"
  echo "\$ A: actor admit <the request with its name changed to root>"
  a actor admit "$(jq -c '.id = "other-id" | .name = "root"' "$W/request.json")" 2>&1 | head -1
  echo "\$ (workspace with no actors): actor admit '<request>'"; c actor admit "$(cat "$W/request.json")" 2>&1 | head -1
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u1-enrolment.txt"

{ echo "# U2 — replicas keep each other current without anyone typing sync"
  "$BIN" --db "$W/b/w.sqlite" p2p sync --every 1 >"$W/syncer.out" 2>"$W/syncer.err" &
  SYNCER=$!
  echo "\$ B: p2p sync --every 1 &        (A is still listening)"
  T=$(a task new "u2 filed on A" --actor "$ALICE" | id)
  echo "alice files a task on A  -> on B's board after $(wait_for 10 b task show "$T")"
  b task claim "$T" --actor "$BOB" >/dev/null
  in_work() { [ "$(status a "$T")" = work ]; }
  echo "bob claims it on B       -> A shows it in work after $(wait_for 10 in_work)"
  echo "B's sync loop printed (only rounds that moved something):"; jq -c '{received: .ok.received.added, sent: .ok.sent}' "$W/syncer.out" | sed 's/^/  /'
  kill -TERM "$SYNCER"; wait "$SYNCER"; echo "sync loop stopped on SIGTERM: exit=$?"; SYNCER=
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u2-periodic-sync.txt"

{ echo "# U3 — a replica is told when a merge takes a claim away"
  T=$(a task new "u3 contested" --actor "$ALICE" | id); b p2p sync >/dev/null
  a task claim "$T" --actor "$X" >/dev/null; b task claim "$T" --actor "$BOB" >/dev/null
  echo "disconnected: x claims on A, bob claims on B; then B syncs"
  seen=$(grep -c "claim lost" "$W/listen.err")
  LOST_B=$(b p2p sync | jq -c '[.ok.received.lost_claims[] | {actor_id, reason}]')
  LOST_A=$(( $(grep -c "claim lost" "$W/listen.err") - seen ))
  WINNER=$(holder a "$T")
  echo "both replicas agree the holder is: $WINNER / $(holder b "$T")"
  echo "B's sync output, lost_claims: $LOST_B"
  echo "A's listener log, lost claims: $LOST_A $(grep "claim lost" "$W/listen.err" | tail -n "$LOST_A" | sed -E 's/^.*net: /— /' | head -1)"
  if [ "$WINNER" = "$X" ]; then echo "\$ B: task renew --actor bob"; b task renew "$T" --actor "$BOB" 2>&1 | head -1
  else echo "\$ A: task renew --actor x"; a task renew "$T" --actor "$X" 2>&1 | head -1; fi

  T2=$(a task new "u3 hung" --actor "$ALICE" | id); b p2p sync >/dev/null
  b task claim "$T2" --actor "$BOB" >/dev/null; b p2p sync >/dev/null
  a task release "$T2" --actor "$ALICE" --force
  echo "bob holds another task; alice force-releases it on A; B syncs"
  echo "B's sync output, lost_claims: $(b p2p sync | jq -c '[.ok.received.lost_claims[] | {actor_id, reason}]')"
} >"$W/raw" 2>&1; names <"$W/raw" >"$EV/u3-lost-claims.txt"

kill -TERM "$LISTENER"; wait "$LISTENER"; echo "listener stopped on SIGTERM: exit=$?" >>"$EV/u2-periodic-sync.txt"; LISTENER=
tail -n +1 "$EV"/u*.txt

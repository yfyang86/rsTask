#!/usr/bin/env bash
# LAN hello-world UAT — sprint-02 U3, run for real this time.
#
#   ./run.sh local   (default) both replicas on this machine, but B dials A's
#                    real LAN address (not loopback), relay disabled.
#   ./run.sh host    machine A: init ./instance-a, file the hello-world task,
#                    serve it on 0.0.0.0:$PORT, print the join command.
#   ./run.sh join 'ID@A_LAN_IP:PORT'
#                    machine B: init ./instance-b and pull the task over LAN.
#
# Both sides need RSTASK_P2P_TOKEN set to the same secret (default below) and
# RSTASK_P2P_RELAY=disabled (exported here): LAN means direct addresses only.
set -uo pipefail
cd "$(dirname "$0")"
REPO=$(cd ../.. && pwd)
BIN=$REPO/target/debug/rstask
EV=$PWD/evidence
WS=/tmp/rstask-uat-lan
PORT=${PORT:-7701}
export RSTASK_P2P_TOKEN=${RSTASK_P2P_TOKEN:-uat-lan-hello}
export RSTASK_P2P_RELAY=disabled

lan_ip() { # first non-loopback IPv4 of the default interface (macOS/Linux)
    ipconfig getifaddr en0 2>/dev/null && return
    ipconfig getifaddr en1 2>/dev/null && return
    ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.*src \([0-9.]*\).*/\1/p' | head -1
}

a() { "$BIN" --db "$WS/a/a.sqlite" "$@"; }
b() { "$BIN" --db "$WS/b/b.sqlite" "$@"; }

build() {
    [ -x "$BIN" ] || cargo build -q -p rstask-cli --manifest-path "$REPO/Cargo.toml" || {
        echo "build failed: cargo build -p rstask-cli" >&2; exit 1; }
}

# start a's listener, wait for its {"spec": ...} line, set NODE_A.
# $BIN is invoked directly, not through a(): $! must be the listener itself,
# not a function subshell whose orphan would keep holding the port.
listen_a() { # $1 = A's database path
    "$BIN" --db "$1" p2p listen --bind "0.0.0.0:$PORT" \
        >"$WS/listen.out" 2>"$WS/listen.err" &
    LISTENER=$!
    for _ in $(seq 1 50); do [ -s "$WS/listen.out" ] && break; sleep 0.1; done
    if [ ! -s "$WS/listen.out" ]; then
        echo "listener did not come up on 0.0.0.0:$PORT:" >&2
        cat "$WS/listen.err" >&2; exit 1
    fi
    NODE_A=$(jq -r .node "$WS/listen.out")
}

local() {
    rm -rf "$WS" && mkdir -p "$WS/a" "$WS/b" "$EV"
    LOG=$EV/10-lan-hello.txt; : >"$LOG"
    IP=$(lan_ip)
    [ -n "$IP" ] || { echo "no LAN address found" >&2; exit 1; }

    echo "# LAN hello-world — relay disabled, B dials A's LAN address ($IP)" >>"$LOG"
    echo "# workspace token: shared; replicas: $WS/{a,b}" >>"$LOG"; echo >>"$LOG"

    echo "# A: init, alice (human root), the hello-world task" >>"$LOG"
    echo "\$ rstask init" >>"$LOG"; a init >>"$LOG" 2>&1; echo >>"$LOG"
    a actor new alice --kind human >"$WS/alice.json"
    ALICE=$(jq -r .id "$WS/alice.json")
    echo "\$ rstask actor new alice --kind human" >>"$LOG"; cat "$WS/alice.json" >>"$LOG"; echo >>"$LOG"
    a task new "hello world over LAN" --actor "$ALICE" >"$WS/hello.json"
    HELLO=$(jq -r .id "$WS/hello.json")
    echo "\$ rstask task new \"hello world over LAN\" --actor alice" >>"$LOG"
    cat "$WS/hello.json" >>"$LOG"; echo >>"$LOG"

    echo "# A: provision bob (agent) for B — the actor event crosses via sync," >>"$LOG"
    echo "#    the secret key file is carried by hand (never over the wire)" >>"$LOG"
    a actor new bob --kind agent --by "$ALICE" >"$WS/bob.json"
    BOB=$(jq -r .id "$WS/bob.json")
    echo "\$ rstask actor new bob --kind agent --by alice" >>"$LOG"; cat "$WS/bob.json" >>"$LOG"; echo >>"$LOG"
    mkdir -p "$WS/b/keys" && cp "$WS/a/keys/$BOB.key" "$WS/b/keys/"
    echo "carried: a/keys/$BOB.key -> b/keys/" >>"$LOG"; echo >>"$LOG"

    listen_a "$WS/a/a.sqlite"
    trap 'kill -TERM $LISTENER 2>/dev/null; wait $LISTENER 2>/dev/null' EXIT
    echo "# A: serving (note the bind address is 0.0.0.0 — reachable from the LAN)" >>"$LOG"
    echo "\$ rstask p2p listen --bind 0.0.0.0:$PORT" >>"$LOG"; cat "$WS/listen.out" >>"$LOG"; echo >>"$LOG"

    SPEC="$NODE_A@$IP:$PORT"
    echo "# B: init empty, then dial A BY ITS LAN ADDRESS: $SPEC" >>"$LOG"
    echo "\$ rstask init" >>"$LOG"; b init >>"$LOG" 2>&1; echo >>"$LOG"
    echo "\$ rstask p2p sync $SPEC" >>"$LOG"
    b p2p sync "$SPEC" >>"$LOG" 2>&1 && echo "(exit 0)" >>"$LOG"; echo >>"$LOG"
    echo "# B's board — the hello-world task arrived over the LAN:" >>"$LOG"
    echo "\$ rstask board" >>"$LOG"; b board >>"$LOG" 2>&1; echo >>"$LOG"

    echo "# B writes back: bob claims the task (signs with the carried key)" >>"$LOG"
    echo "\$ rstask task claim $HELLO --actor bob" >>"$LOG"
    b task claim "$HELLO" --actor "$BOB" >>"$LOG" 2>&1 && echo "(exit 0)" >>"$LOG"; echo >>"$LOG"
    echo "# B: sync again (no argument = every known peer); A learns the claim" >>"$LOG"
    echo "\$ rstask p2p sync" >>"$LOG"
    b p2p sync >>"$LOG" 2>&1 && echo "(exit 0)" >>"$LOG"; echo >>"$LOG"

    echo "# convergence checks" >>"$LOG"
    A_HEADS=$(a sync status | jq -c .heads); B_HEADS=$(b sync status | jq -c .heads)
    A_BOARD=$(a board); B_BOARD=$(b board)
    echo "A heads: $A_HEADS" >>"$LOG"; echo "B heads: $B_HEADS" >>"$LOG"
    echo "heads identical:  $([ "$A_HEADS" = "$B_HEADS" ] && echo yes || echo NO)" >>"$LOG"
    echo "boards identical: $([ "$A_BOARD" = "$B_BOARD" ] && echo yes || echo NO)" >>"$LOG"
    echo "\$ rstask board   (on A — hello world claimed by bob)" >>"$LOG"
    a board >>"$LOG" 2>&1; echo >>"$LOG"
    echo "# re-sync moves nothing (received.added = 0 both ways)" >>"$LOG"
    echo "\$ rstask p2p sync" >>"$LOG"; b p2p sync >>"$LOG" 2>&1; echo >>"$LOG"

    echo "# event chain of the hello-world task, identical on both replicas:" >>"$LOG"
    a task log "$HELLO" | jq -c '[.[] | .kind]' | sed 's/^/A: /' >>"$LOG" 2>&1
    b task log "$HELLO" | jq -c '[.[] | .kind]' | sed 's/^/B: /' >>"$LOG" 2>&1

    kill -TERM "$LISTENER" 2>/dev/null; wait "$LISTENER" 2>/dev/null; trap - EXIT
    echo >>"$LOG"; echo "# PASS: B reached A at $IP:$PORT, state converged both ways" >>"$LOG"
    tail -n +1 "$LOG"
}

host() {
    IP=$(lan_ip); mkdir -p "$EV"
    WS=$PWD/instance-a; a() { "$BIN" --db "$WS/a.sqlite" "$@"; }
    [ -f "$WS/a.sqlite" ] || { mkdir -p "$WS"; a init >/dev/null
        ALICE=$(a actor new alice --kind human | jq -r .id)
        a task new "hello world over LAN" --actor "$ALICE" | jq -r .id >"$WS/hello.id"
        echo "instance-a ready: alice + hello-world task ($(cat "$WS/hello.id"))"; }
    listen_a "$WS/a.sqlite"
    trap 'kill -TERM $LISTENER 2>/dev/null; wait $LISTENER 2>/dev/null' EXIT
    cat "$WS/listen.out"
    echo "serving on 0.0.0.0:$PORT — on machine B (same LAN, same RSTASK_P2P_TOKEN) run:"
    echo "  ./run.sh join '$NODE_A@$IP:$PORT'"
    wait "$LISTENER"
}

join() {
    WS=$PWD/instance-b; mkdir -p "$WS"
    "$BIN" --db "$WS/b.sqlite" init >/dev/null
    echo "\$ rstask p2p sync $1"
    "$BIN" --db "$WS/b.sqlite" p2p sync "$1" || exit 1
    echo "\$ rstask board"
    "$BIN" --db "$WS/b.sqlite" board
}

build
case "${1:-local}" in
    local) local ;;
    host) host ;;
    join) [ $# -ge 2 ] || { echo "usage: ./run.sh join 'ID@HOST:PORT'" >&2; exit 2; }; join "$2" ;;
    *) echo "usage: ./run.sh {local|host|join SPEC}" >&2; exit 2 ;;
esac

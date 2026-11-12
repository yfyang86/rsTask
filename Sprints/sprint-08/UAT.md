# Sprint 08 — UAT

Written before development. Evidence → `uat-evidence/` (regenerate with `./uat.sh`).
Loopback, relay disabled; replicas A and B have separate databases and key directories.

## U1 — An actor joins from another machine without a secret key moving
**Given** A holds the workspace (root `alice`); B is an empty replica.
**When** B runs `actor request bob --kind agent --role worker` and hands the printed
request to A; A runs `actor admit '<request>'`; B syncs and claims a task as `bob`.
**Then** the claim succeeds and reaches A; `bob`'s key file exists in B's key
directory and nowhere on A; `bob`'s creation event is signed by `alice`. A request
whose name was edited after it was made is refused; `actor admit` cannot create the
first actor of a workspace.
Evidence: the request (public fields), key directory listings, the event, the refusals.

## U2 — Replicas keep each other current without anyone typing `sync`
**Given** A runs `p2p listen`; B runs `p2p sync --every 1` in the background.
**When** `alice` files a task on A; a moment later `bob` claims it on B.
**Then** B's board shows the task, and A's board shows it in `work`, each within a few
seconds and with no manual sync; both processes stop cleanly on SIGTERM.
Evidence: boards with timings, exit codes.

## U3 — A replica is told when a merge takes a claim away
**Given** agents `x` (on A) and `y` (on B) claim the same task while disconnected.
**When** B syncs with A.
**Then** exactly one of them lost. If `y` lost, B's `p2p sync` output lists the claim
under `lost_claims`; if `x` lost, A's listener logs it. On either replica the loser's
`task renew` is refused naming the winner. A human's force-release arriving by sync is
reported the same way; a holder's own release is not.
Evidence: sync output / listener log, the refusal.

## U4 — Pull requests are checked
**Given** `.github/workflows/ci.yml`.
**When** this sprint's pull request is opened.
**Then** GitHub runs fmt, clippy (`-D warnings`) and the test suite on it.
Evidence: the check runs on the pull request.

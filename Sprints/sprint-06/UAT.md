# Sprint 06 — UAT

Written before development. Evidence → `uat-evidence/` (regenerate with `./uat.sh`).

## U1 — A blocked task waits for its blocker
**Given** tasks `deploy` (priority 9) and `build`; `deploy` is blocked by `build`.
**When** a worker calls `claim_next`, then tries `task claim deploy`; `build` goes
through review to `done`; the worker calls `claim_next` again.
**Then** the first `claim_next` hands out `build` despite its lower priority; the direct
claim is refused naming the blocker; after `build` is `done`, `claim_next` hands out
`deploy`. A second replica that imported the bundle refuses the same claim.
Evidence: the two `claim_next` titles, the refusals on A and B, `task show deploy`.

## U2 — A parent waits for its subtasks
**Given** an `epic` and two subtasks created with `--parent epic`.
**When** a worker calls `claim_next` three times, finishing each task in between.
**Then** the subtasks come first although the epic is older; `task show epic` lists its
children and what it is waiting on; the epic is handed out only once both are `done`.
Evidence: order of titles, `task show epic` before and after.

## U3 — Dependencies must make sense
**When** a task is blocked by itself; by an unknown id; or by a task that already
(transitively) waits on it.
**Then** each is refused (`cycle` / `not found`) and nothing is logged; removing an edge
with `task unblock` makes the task claimable again.
Evidence: the three errors, log kinds.

## U4 — Roles route the work
**Given** agents `planner` (role planner), `worker` (worker), `reviewer` (reviewer),
each on its own MCP server.
**When** each tries all three jobs: `task_create`, `claim_next`, `task_approve`.
**Then** each succeeds only at its own: the planner plans, the worker claims and
completes, the reviewer approves; every other attempt is a tool error naming the
missing role; the server instructions state the agent's roles. An actor created
without `--role` can still do everything (README quick start unchanged).
Evidence: the 3×3 outcome table, the instructions line.

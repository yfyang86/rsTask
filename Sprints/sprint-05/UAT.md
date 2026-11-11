# Sprint 05 — UAT

Written before development. Evidence → `uat-evidence/` (regenerate with `./uat.sh`).
Each agent runs its own `rstask-mcp --actor <id>` over stdio; requests below are MCP
`tools/call`s.

## U1 — An agent finds its tools and its identity
**Given** a workspace with agent `bot`.
**When** a client initializes `rstask-mcp --actor bot` and lists tools.
**Then** the server instructions name the actor; the tool list is `board_state`,
`claim_next`, `task_approve`, `task_claim`, `task_complete`, `task_create`, `task_get`,
`task_reject`, `task_release`, `task_renew`, `task_update`, `tasks_list`; no tool takes
an actor argument. Starting with an unknown actor fails before serving.
Evidence: instructions, tool names, the startup error.

## U2 — `claim_next` hands out distinct tasks under a race
**Given** 3 queued tasks with priorities 5, 1, 0 and 8 agents, one server each.
**When** all 8 call `claim_next` at the same moment.
**Then** exactly 3 get a task — all different — and 5 get `null`; a lone agent calling
first always gets the priority-5 task.
Evidence: per-agent result, board.

## U3 — A full worker → reviewer loop over MCP
**Given** agents `bot` and `reviewer`, one queued task.
**When** `bot`: `claim_next` → `task_renew` → `task_update` (body) → `task_complete`
(summary, refs, evidence); `reviewer`: `task_get` → `task_reject` (reason); `bot`:
`claim_next` again → `task_get` → `task_complete`; `reviewer`: `task_approve`.
**Then** `reviewer`'s `task_get` shows the submission; `bot`'s second `task_get` shows
the rejection reason; the task ends `done`; the CLI `task log` shows every step signed
by the right actor.
Evidence: the two `task_get` results, final status, log kinds + actors.

## U4 — The gate holds over MCP
**Given** `bot` holds a task; `rival` is another agent.
**When** `rival` calls `task_update`, `task_complete`, `task_release` on it; `bot`
completes and then calls `task_approve` on its own submission.
**Then** every one of these returns a tool error (`isError: true`) with core's message;
the log gains only `bot`'s submission.
Evidence: the four error texts, log kinds.

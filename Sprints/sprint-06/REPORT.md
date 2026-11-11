# Sprint 06 — Report

## Shipped
- **Dependencies** — `rstask task block A --by B` / `task unblock`, MCP `task_block` /
  `task_unblock`. A blocked task cannot be claimed and `claim_next` passes over it,
  whatever its priority, until every blocker is `done`.
- **Subtasks** — `task new --parent P`, MCP `task_create.parent`. A parent is not
  claimable until all its subtasks are `done`; then it is handed out like any task
  (someone integrates and submits it for review).
- **Cycle refusal** — a dependency that would make a task wait on itself, through any
  chain of blockers and subtasks, is refused and logs nothing.
- **`task show` / `task_get`** list `blocked_by`, `children` and `waiting_on`.
- **Roles** — `actor new --role planner|worker|reviewer` (repeatable). Planners create
  tasks and edit dependencies and unclaimed tasks; workers claim; reviewers approve
  and reject. The refusal names the missing role. No `--role` = all three, so existing
  actors, the README quick start and earlier UAT scripts are unaffected.
- Both replicate: links through events (the dependency set is folded per task in log
  order, so concurrent edits converge), roles through the actor directory.

## What the tests caught (the loop worked)
- Nothing in the product this time: the six new tests passed on the first full
  implementation, and the migration check on a sprint-05 database was clean.
- The UAT transcripts first showed `<id>` everywhere instead of task names — a script
  bug (scenario ran in a pipeline subshell), fixed in `uat.sh`.

## Decisions recorded
- **Subtasks block their parent.** `parent` is not just a label: it is the cheapest
  rule that lets a planner decompose a task and have the pieces done first. The
  parent is not closed automatically.
- **Roles are restrictive only when given.** Simpler than a permission matrix and
  backward compatible; humans and agents follow the same rule.
- **The claim holder may always edit and submit its own task**, planner or not.
- **Roles and parents are fixed at creation.** Changing them needs an authority model
  (who may promote an agent?) — that is sprint 07's subject.
- Dependencies on a claimed task are still editable by a planner; the holder decides
  whether to release.

## Slipped / debt → sprint 07
- Roles and the blocked rule are enforced on the local write path only. A replica
  that has not yet heard of a dependency can claim the task; the merge accepts it.
- Two replicas can each add half of a cycle; after merge both tasks wait forever
  until a planner removes an edge. Detectable in the fold; not done.
- No "why can't I claim anything" view: `claim_next` returns `null` whether the queue
  is empty or everything is waiting. `task_get` shows `waiting_on` per task.
- Sprint-05 UAT evidence predates the two new tools; left as the record of that sprint.

## Verification
- `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings`
  zero warnings, `cargo test --workspace` 46/46 green (7 lib + 24 store + 13 sync +
  2 MCP stdio).
- `./uat.sh`: U1–U4 pass; transcripts in `uat-evidence/`. Sprint 03, 04 and 05
  `uat.sh` re-run: still pass.

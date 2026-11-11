# Sprint 06 — Dependencies and roles (plan)

> PRD refs: §Multi-agent M4 M5 · PRD: `PRD/PRD-v3.md` · Depends on: sprint-05

## Goal (one sentence)

A planner can lay out work as a graph — this before that, this made of those — and
the workspace hands each task to a worker only when it is ready, and each job only to
an actor whose role allows it.

## Scope

**In:**
* M4 — migration `0003`: `tasks.parent_id`, `task_deps`. `add_dependency` /
  `remove_dependency` (`task.blocked` / `task.unblocked`), `create_subtask`
  (`parent` on `task.created`). A task is claimable only when every blocker and every
  subtask is `done`: `claim_task` refuses (`Blocked`), `claim_next` skips. Cycles
  through blockers or subtasks are refused (`DependencyCycle`). Dependencies fold per
  task in log order on merge. `task_detail` gains `blocked_by`, `children`, `waiting_on`.
* M5 — `actors.roles` (`planner`, `worker`, `reviewer`), fixed at creation, carried in
  the sync actor directory; none given = all. Enforced in core: planner to create
  tasks, edit dependencies and edit unclaimed tasks; worker to claim; reviewer to
  approve / reject.
* CLI: `actor new --role…`, `task new --parent`, `task block|unblock --by`.
  MCP: `task_create.parent`, `task_block`, `task_unblock`; instructions state the roles.

**Out (explicitly):** changing roles or re-parenting after creation; per-task routing
(skills/capabilities beyond the three roles); enforcing roles or the blocked rule on
merged events (sprint 07, R2); auto-closing a parent when its subtasks finish; hiding
MCP tools the actor's role cannot use.

---

## Loop

### 1. Investigate
- [x] `claim_next` eligibility is one `WHERE`; `claim_tx` is the single claim path.
- [x] The actor directory travels as `ActorRow` in the bundle, not as events — roles
  ride there (and inherit its trust-on-first-use).
- [x] `task_modules` (v1) is unrelated and stays unused.

### 2. Setup UAT → `UAT.md`
Scenarios U1–U4 written before development.

### 3. Development
- [x] Tests first: 4 in `tests/store_test.rs`, 2 in `tests/sync.rs`.
- [x] Migration, models (`Role`, `Roles`, `Task.parent_id`, `TaskDetail`), store, CLI, MCP.

### 4. Test
- [x] `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [x] `./uat.sh` → `uat-evidence/`
- [x] A database created by the sprint-05 binary opens, migrates and keeps working.

### 5. Fix / investigate
- [x] No product defects this loop; one UAT-script bug (ids not replaced by names in
  the transcripts because the scenario ran in a pipeline subshell).

### 6. Plan next
- [x] REPORT.md; debt carried to sprint 07.

---

## Deliverables
- Migration `0003_links_roles.sql`; core, CLI and MCP changes; tests; this sprint's docs

## Definition of Done
- [x] Gate green (fmt, clippy `-D warnings`, tests)
- [x] A higher-priority blocked task is handed out only after its blocker is `done`; the same refusal on a second replica
- [x] A parent is handed out only after its subtasks
- [x] Self, unknown and cyclic dependencies are refused with nothing logged
- [x] Planner / worker / reviewer agents each succeed only at their own job, over MCP
- [x] An actor created without roles can do everything; a pre-sprint database still works

## Risks touched
- R10 — Sync bundle format changed (`ActorRow.roles`): a bundle exported by an older
  binary no longer decodes. Bundles are transient; both sides must run the same build.
- R11 — Roles are honour-system until sprint 07: any process can create an actor with
  any role, and a merged event is not checked against its author's roles.

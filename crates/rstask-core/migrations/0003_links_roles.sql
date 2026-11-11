-- Sprint 06: dependencies, subtasks and roles (PRD-v3 M4, M5).

-- A subtask names its parent; a parent waits for its subtasks.
ALTER TABLE tasks ADD COLUMN parent_id TEXT;

-- `task_id` waits until `blocked_by` is done. Derived from the task's
-- `task.blocked` / `task.unblocked` events (no FK: the log is the source).
CREATE TABLE task_deps (
    task_id     TEXT NOT NULL,
    blocked_by  TEXT NOT NULL,
    PRIMARY KEY (task_id, blocked_by)
);

-- Comma-separated subset of planner,worker,reviewer. Actors that predate
-- roles, or are created without any, hold all three.
ALTER TABLE actors ADD COLUMN roles TEXT NOT NULL DEFAULT 'planner,worker,reviewer';

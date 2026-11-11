-- RsTask initial schema
-- SQLite is the source of truth: tasks, actors, claims, events, module state.

PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS actors (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL CHECK (kind IN ('human', 'agent')),
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    last_heartbeat_at TEXT
);

CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    body_md     TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'queue'
                CHECK (status IN ('queue', 'work', 'review', 'done')),
    priority    INTEGER NOT NULL DEFAULT 0,
    created_by  TEXT NOT NULL REFERENCES actors(id),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- Claims are atomic locks with TTL. A task may have at most one active claim;
-- expired claims are swept lazily on every claim attempt.
CREATE TABLE IF NOT EXISTS claims (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES tasks(id),
    actor_id        TEXT NOT NULL REFERENCES actors(id),
    claimed_at      TEXT NOT NULL,
    expires_at      TEXT NOT NULL,
    released_at     TEXT,
    release_reason  TEXT CHECK (release_reason IN ('done', 'released', 'expired', 'forced'))
);

CREATE INDEX IF NOT EXISTS idx_claims_task_active
    ON claims(task_id) WHERE released_at IS NULL;

-- Immutable event log: every claim/update/complete is appended, never mutated.
CREATE TABLE IF NOT EXISTS events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL REFERENCES tasks(id),
    actor_id    TEXT REFERENCES actors(id),
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',
    at          TEXT NOT NULL
);

-- Modules are orthogonal attachments: a task can appear here in any
-- combination (sprint, kanban, okr, waterfall, brainstorm, pipeline).
CREATE TABLE IF NOT EXISTS task_modules (
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    module      TEXT NOT NULL
                CHECK (module IN ('sprint', 'kanban', 'okr', 'waterfall', 'brainstorm', 'pipeline')),
    state       TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (task_id, module)
);

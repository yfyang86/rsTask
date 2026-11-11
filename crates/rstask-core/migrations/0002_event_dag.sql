-- Sprint 01: multi-node event DAG (PRD-v2 D3, D4, D5).

-- The DAG is the source of truth; task rows are derived by the fold (PRD-v2).
-- A created-event causally precedes its task row, so the v1 FK events→tasks
-- cannot hold on merge: rebuild the table without it (SQLite cannot drop a
-- constraint in place).
CREATE TABLE events_dag (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    actor_id    TEXT REFERENCES actors(id),
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',
    at          TEXT NOT NULL,
    event_id    BLOB,               -- NULL on v1 rows: local-only, never synced
    lamport     INTEGER,
    parents     TEXT NOT NULL DEFAULT '[]',
    signature   BLOB,
    pubkey      BLOB
);
INSERT INTO events_dag (seq, task_id, actor_id, kind, payload, at)
    SELECT seq, task_id, actor_id, kind, payload, at FROM events;
DROP TABLE events;
ALTER TABLE events_dag RENAME TO events;

CREATE INDEX IF NOT EXISTS idx_events_event_id
    ON events(event_id) WHERE event_id IS NOT NULL;

-- DAG heads: known events no other known event names as a parent.
CREATE TABLE IF NOT EXISTS heads (
    event_id BLOB PRIMARY KEY
);

-- Events received before their parents; drained on every insert.
CREATE TABLE IF NOT EXISTS pending_events (
    event_id    BLOB PRIMARY KEY,
    body        BLOB NOT NULL,          -- postcard-encoded StoredEvent
    parents     TEXT NOT NULL,          -- JSON array of hex event ids
    received_at TEXT NOT NULL
);

-- Actor signing keys. Interim plaintext keystore inside the workspace DB (PRD-v2 D5):
-- `secret` is NULL for actors learned from peers (verify-only).
CREATE TABLE IF NOT EXISTS actor_keys (
    actor_id TEXT PRIMARY KEY REFERENCES actors(id),
    pubkey   BLOB NOT NULL,
    secret   BLOB
);

-- Sprint 02: known sync peers (PRD-v2 D1). `id` is the peer's iroh endpoint id,
-- `addrs` a comma-separated list of direct socket addresses (may be empty).
CREATE TABLE peers (
    id         TEXT PRIMARY KEY,
    addrs      TEXT NOT NULL DEFAULT '',
    last_seen  TEXT
);

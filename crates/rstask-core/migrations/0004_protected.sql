-- Sprint 07: protected tasks need a human approval (PRD-v3 R5).
ALTER TABLE tasks ADD COLUMN protected INTEGER NOT NULL DEFAULT 0;

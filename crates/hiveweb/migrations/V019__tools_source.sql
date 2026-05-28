-- Add `source` column to tools table (workspace | builtin), mirroring skills.source
-- builtin Tool = system pre-defined, immutable; workspace = user-created.
-- builtin Tool can only wrap builtin Function (kind=1).

ALTER TABLE tools
    ADD COLUMN source VARCHAR(16) NOT NULL DEFAULT 'workspace' COMMENT 'workspace | builtin'
    AFTER kind;

-- Backfill: existing rows are user-created → workspace
UPDATE tools SET source = 'workspace' WHERE source = '';

-- Stage 2: Thesis Memory schema extension
--
-- Adds `symbol` for per-symbol snapshot lookup and `schema_version` for
-- forward compatibility with later enrichment plans.
--
-- SQLite allows ADD COLUMN for nullable columns and for NOT NULL columns
-- that carry a DEFAULT expression.

ALTER TABLE phase_snapshots ADD COLUMN symbol TEXT;
ALTER TABLE phase_snapshots ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;

-- Index to support efficient per-symbol phase-5 thesis lookups by recency.
CREATE INDEX IF NOT EXISTS idx_phase_snapshots_symbol_phase
    ON phase_snapshots(symbol, phase_number, created_at);

CREATE TABLE IF NOT EXISTS phase_snapshots (
    execution_id    TEXT    NOT NULL,
    phase_number    INTEGER NOT NULL,
    phase_name      TEXT    NOT NULL,
    trading_state_json TEXT NOT NULL,
    token_usage_json   TEXT,
    created_at      TEXT    NOT NULL,
    UNIQUE(execution_id, phase_number)
);

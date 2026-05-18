CREATE TABLE IF NOT EXISTS transcript_cache (
    symbol       TEXT NOT NULL,
    quarter      TEXT NOT NULL CHECK (quarter GLOB '[0-9][0-9][0-9][0-9]Q[1-4]'),
    payload_json TEXT NOT NULL,
    cached_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (symbol, quarter)
);

-- Image generation bookkeeping: pacing for the owner's ChatGPT session (and the API fallback).
CREATE TABLE image_generations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    backend     TEXT NOT NULL,          -- 'chatgpt' | 'openai_api'
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    outcome     TEXT NOT NULL DEFAULT 'running',  -- ok | limit | blocked | not_signed_in | refused | timeout | error
    note        TEXT
);
CREATE INDEX image_generations_recent ON image_generations(backend, started_at);

-- A backend is paused until `until_at` (ChatGPT said "limit reached", a bot check, repeated errors).
CREATE TABLE image_cooldowns (
    backend   TEXT PRIMARY KEY,
    until_at  INTEGER NOT NULL,
    reason    TEXT NOT NULL
);

-- Juggernaught OS schema v1 (SQLite, WAL).
-- Columns ending in `_enc` hold AES-256-GCM envelopes (jugg-crypto::seal) with
-- AAD = "<table>:<column>:<id>". Columns ending in `_bidx` are HMAC blind indexes.
-- Keys never live in this database.

CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value_json  TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- ---------------------------------------------------------------- identity
CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    email_enc     BLOB NOT NULL,
    email_bidx    TEXT NOT NULL UNIQUE,
    display_name  TEXT,
    role          TEXT NOT NULL CHECK (role IN ('owner','editor','viewer')),
    created_at    INTEGER NOT NULL,
    disabled_at   INTEGER
);

-- Magic links: only the SHA-256 of the token is stored. Single use, 15 min.
CREATE TABLE auth_magic_links (
    token_hash  TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at  INTEGER NOT NULL,
    used_at     INTEGER,
    ip_bidx     TEXT
);

CREATE TABLE auth_totp (
    user_id       TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret_enc    BLOB NOT NULL,
    confirmed_at  INTEGER,
    last_step     INTEGER NOT NULL DEFAULT 0   -- replay protection
);

CREATE TABLE auth_passkeys (
    id               TEXT PRIMARY KEY,
    user_id          TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_enc   BLOB NOT NULL,           -- serialized webauthn Passkey
    label            TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    last_used_at     INTEGER
);

-- Recovery codes: Argon2id hashes, single use.
CREATE TABLE auth_recovery_codes (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash  TEXT NOT NULL,
    used_at    INTEGER
);

CREATE TABLE sessions (
    token_hash    TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    method        TEXT NOT NULL CHECK (method IN ('magic_link','passkey','totp','recovery')),
    created_at    INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    device_label  TEXT,
    ip_bidx       TEXT,
    revoked_at    INTEGER
);
CREATE INDEX sessions_user ON sessions(user_id);

-- ---------------------------------------------------------------- brand
CREATE TABLE brands (
    id             TEXT PRIMARY KEY,
    version        INTEGER NOT NULL,
    standards_json TEXT NOT NULL,      -- jugg-brand::BrandStandards (public by nature)
    is_active      INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL,
    UNIQUE (id, version)
);

-- ---------------------------------------------------------------- assets
CREATE TABLE assets (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    title           TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('draft','rejected','review','final')),
    storage_path    TEXT NOT NULL,     -- relative to data_dir/assets
    mime            TEXT NOT NULL,
    bytes           INTEGER NOT NULL,
    sha256          TEXT NOT NULL,
    encrypted       INTEGER NOT NULL DEFAULT 0,  -- private assets use FileCipher
    brand_version   INTEGER,
    gate_report_json TEXT,             -- brand/standards gate output
    parent_id       TEXT REFERENCES assets(id), -- variants (sizes, formats, vectorized logo)
    job_id          TEXT,
    created_at      INTEGER NOT NULL
);
CREATE INDEX assets_kind ON assets(kind, created_at DESC);

-- ---------------------------------------------------------------- conversations
CREATE TABLE conversations (
    id          TEXT PRIMARY KEY,
    title_enc   BLOB NOT NULL,       -- chat titles quote the owner's words: encrypted
    user_id     TEXT REFERENCES users(id),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE messages (
    id               TEXT PRIMARY KEY,
    conversation_id  TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role             TEXT NOT NULL CHECK (role IN ('user','assistant','tool','system')),
    content_enc      BLOB NOT NULL,
    tool_calls_json  TEXT,
    provider         TEXT,
    created_at       INTEGER NOT NULL
);
CREATE INDEX messages_conv ON messages(conversation_id, created_at);

-- ---------------------------------------------------------------- AI providers
CREATE TABLE providers (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN ('claude_code_cli','codex_cli','openrouter','anthropic_api','openai_api')),
    label        TEXT NOT NULL,
    config_json  TEXT NOT NULL,     -- model, base url, flags (no secrets)
    secret_enc   BLOB,              -- API key if any
    priority     INTEGER NOT NULL DEFAULT 100,
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL
);

CREATE TABLE usage (
    id             TEXT PRIMARY KEY,
    provider_id    TEXT NOT NULL,
    model          TEXT NOT NULL,
    input_tokens   INTEGER NOT NULL DEFAULT 0,
    output_tokens  INTEGER NOT NULL DEFAULT 0,
    cost_micros    INTEGER NOT NULL DEFAULT 0,
    conversation_id TEXT,
    created_at     INTEGER NOT NULL
);

-- ---------------------------------------------------------------- jobs
-- SQLite has no SKIP LOCKED: workers claim with a single
-- UPDATE ... WHERE id = (SELECT ... LIMIT 1) RETURNING inside BEGIN IMMEDIATE.
CREATE TABLE jobs (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('queued','running','awaiting_approval','succeeded','failed','cancelled')),
    priority      INTEGER NOT NULL DEFAULT 50,
    payload_json  TEXT NOT NULL,
    result_json   TEXT,
    progress      REAL NOT NULL DEFAULT 0,
    error         TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0,
    max_attempts  INTEGER NOT NULL DEFAULT 3,
    worker        TEXT,
    heartbeat_at  INTEGER,
    run_after     INTEGER NOT NULL,
    conversation_id TEXT,
    created_at    INTEGER NOT NULL,
    finished_at   INTEGER
);
CREATE INDEX jobs_ready ON jobs(status, priority DESC, run_after);

CREATE TABLE tool_runs (
    id           TEXT PRIMARY KEY,
    job_id       TEXT REFERENCES jobs(id),
    tool         TEXT NOT NULL,
    tool_version TEXT NOT NULL,
    args_json    TEXT NOT NULL,
    exit_code    INTEGER,
    duration_ms  INTEGER,
    log_tail     TEXT,
    created_at   INTEGER NOT NULL
);

-- Publish / destructive actions wait here for the owner (shown inline in chat).
CREATE TABLE approvals (
    id            TEXT PRIMARY KEY,
    job_id        TEXT REFERENCES jobs(id),
    action        TEXT NOT NULL,
    risk          TEXT NOT NULL,
    summary       TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','expired')),
    requested_at  INTEGER NOT NULL,
    decided_at    INTEGER,
    decided_by    TEXT REFERENCES users(id)
);

-- ---------------------------------------------------------------- website
CREATE TABLE site_pages (
    id           TEXT PRIMARY KEY,
    slug         TEXT NOT NULL UNIQUE,
    title        TEXT NOT NULL,
    blocks_json  TEXT NOT NULL,     -- jugg-web::Page
    status       TEXT NOT NULL CHECK (status IN ('draft','published','archived')),
    updated_at   INTEGER NOT NULL
);

CREATE TABLE site_deploys (
    id               TEXT PRIMARY KEY,
    status           TEXT NOT NULL CHECK (status IN ('building','failed','live','superseded','rolled_back')),
    report_json      TEXT NOT NULL,   -- validator report incl. accepted overrides
    content_sha256   TEXT NOT NULL,
    created_by       TEXT REFERENCES users(id),
    created_at       INTEGER NOT NULL,
    live_at          INTEGER
);

-- Owner chose to bypass a standards rule. Always warned, always logged.
CREATE TABLE standards_overrides (
    id          TEXT PRIMARY KEY,
    rule        TEXT NOT NULL,
    target      TEXT NOT NULL,      -- page slug / asset id / '*'
    reason      TEXT NOT NULL,
    user_id     TEXT NOT NULL REFERENCES users(id),
    created_at  INTEGER NOT NULL,
    revoked_at  INTEGER
);

-- Website form submissions (contact forms etc.) are customer PII: encrypted.
CREATE TABLE form_submissions (
    id          TEXT PRIMARY KEY,
    form        TEXT NOT NULL,
    data_enc    BLOB NOT NULL,
    created_at  INTEGER NOT NULL
);

-- ---------------------------------------------------------------- audit
-- Hash-chained; every N entries a checkpoint is signed with ML-DSA-65.
CREATE TABLE audit_log (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    prev_hash   TEXT NOT NULL,
    entry_hash  TEXT NOT NULL,
    actor       TEXT NOT NULL,      -- user id, 'agent:<conv>', 'steward', 'updater'
    action      TEXT NOT NULL,
    detail_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE audit_checkpoints (
    seq         INTEGER PRIMARY KEY REFERENCES audit_log(seq),
    entry_hash  TEXT NOT NULL,
    signature   BLOB NOT NULL,      -- ML-DSA-65 over "jugg-audit-v1|seq|entry_hash"
    created_at  INTEGER NOT NULL
);

-- ---------------------------------------------------------------- updates
CREATE TABLE updates (
    id            TEXT PRIMARY KEY,
    component     TEXT NOT NULL,
    from_version  TEXT,
    to_version    TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('available','staged','canary','promoted','rolled_back','failed','deferred')),
    log           TEXT,
    created_at    INTEGER NOT NULL,
    finished_at   INTEGER
);

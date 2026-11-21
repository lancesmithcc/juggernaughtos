-- Steward has its own database so a compromised app process cannot
-- silently erase security history (steward runs as a separate user).
CREATE TABLE events (
    id          TEXT PRIMARY KEY,
    source      TEXT NOT NULL,      -- http, sshd, canary, fim, falco, suricata, crowdsec, health
    kind        TEXT NOT NULL,
    severity    TEXT NOT NULL CHECK (severity IN ('info','low','medium','high','critical')),
    confidence  REAL NOT NULL,
    subject     TEXT,               -- ip / path / service
    detail_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX events_subject ON events(subject, created_at);

CREATE TABLE incidents (
    id           TEXT PRIMARY KEY,
    subject      TEXT NOT NULL,
    score        REAL NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('open','contained','resolved','false_positive')),
    event_ids    TEXT NOT NULL,     -- JSON array
    actions_json TEXT NOT NULL,     -- what the steward did
    triage_md    TEXT,              -- AI-written plain-language summary
    created_at   INTEGER NOT NULL,
    resolved_at  INTEGER
);

CREATE TABLE bans (
    subject     TEXT PRIMARY KEY,
    reason      TEXT NOT NULL,
    expires_at  INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE fim_baseline (
    path        TEXT PRIMARY KEY,
    sha256      TEXT NOT NULL,
    mode        INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE checks (
    name        TEXT PRIMARY KEY,   -- tls_expiry, disk, backup_age, cve_scan, site_up ...
    status      TEXT NOT NULL CHECK (status IN ('ok','warn','fail')),
    detail      TEXT NOT NULL,
    checked_at  INTEGER NOT NULL
);

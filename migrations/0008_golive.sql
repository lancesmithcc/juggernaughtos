-- Go live: one row tracking how this box gets onto the owner's domain.
CREATE TABLE golive (
    id                TEXT PRIMARY KEY CHECK (id = 'golive'),
    domain            TEXT,
    -- RDAP: 1 registered, 0 available to buy, NULL not checked / unknown
    registered        INTEGER,
    registrar         TEXT,
    cf_token_enc      BLOB,
    cf_account_id     TEXT,
    cf_zone_id        TEXT,
    -- Cloudflare zone status: pending | active | …
    zone_status       TEXT,
    nameservers       TEXT,
    -- What the domain's nameservers are right now (public DNS), JSON array
    current_ns        TEXT,
    -- vps | home (next phases)
    target            TEXT,
    last_error        TEXT,
    checked_at        INTEGER,
    updated_at        INTEGER NOT NULL
);

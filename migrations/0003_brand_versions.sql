-- Brands are versioned: every change (wizard, logo adoption, edits) adds a row.
-- 0001 made `id` alone the primary key, which allowed only one version.
CREATE TABLE brands_v2 (
    id             TEXT NOT NULL,
    version        INTEGER NOT NULL,
    standards_json TEXT NOT NULL,
    is_active      INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (id, version)
);
INSERT INTO brands_v2 (id, version, standards_json, is_active, created_at)
    SELECT id, version, standards_json, is_active, created_at FROM brands;
DROP TABLE brands;
ALTER TABLE brands_v2 RENAME TO brands;
CREATE INDEX brands_active ON brands(is_active, version DESC);

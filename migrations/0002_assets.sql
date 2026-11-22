-- Phase 3: asset previews and provenance.
ALTER TABLE assets ADD COLUMN preview_path TEXT;
ALTER TABLE assets ADD COLUMN conversation_id TEXT;
ALTER TABLE assets ADD COLUMN decided_at INTEGER;
CREATE INDEX assets_sha ON assets(sha256);
CREATE INDEX assets_status ON assets(status, created_at DESC);

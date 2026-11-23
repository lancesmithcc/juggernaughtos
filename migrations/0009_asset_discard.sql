-- Discarded assets are hidden from the library (soft delete, undoable).
ALTER TABLE assets ADD COLUMN discarded_at INTEGER;

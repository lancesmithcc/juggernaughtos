-- Why the last automatic attempt for an image request failed (shown to the owner).
ALTER TABLE image_handoffs ADD COLUMN note TEXT;

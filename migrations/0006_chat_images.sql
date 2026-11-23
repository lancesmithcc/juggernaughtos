CREATE TABLE chat_images (
    id          TEXT PRIMARY KEY,
    message_id  TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    image_enc   BLOB NOT NULL,
    name_enc    BLOB NOT NULL,
    UNIQUE (message_id, position)
);

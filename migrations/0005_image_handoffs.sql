-- Images the owner makes in their own ChatGPT: the agent prepares the prompt, the owner
-- opens it in ChatGPT and drops the finished image back into the chat.
CREATE TABLE image_handoffs (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    prompt          TEXT NOT NULL,      -- full prompt, brand rules included
    kind            TEXT NOT NULL,      -- image | texture | icon_set | logo
    shape           TEXT NOT NULL,      -- square | landscape | portrait
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'waiting',  -- waiting | done
    asset_id        TEXT,
    created_at      INTEGER NOT NULL,
    done_at         INTEGER
);
CREATE INDEX image_handoffs_conversation ON image_handoffs(conversation_id, created_at);

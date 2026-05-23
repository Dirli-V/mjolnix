CREATE TABLE repo_stores (
    repo_id BIGINT PRIMARY KEY REFERENCES repos (id) ON DELETE CASCADE,
    store_root TEXT NOT NULL,
    store_uri TEXT NOT NULL,
    substituter_url TEXT NOT NULL,
    cache_public_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

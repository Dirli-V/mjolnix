CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE ssh_keys (
    fingerprint TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE repos (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users (id),
    namespace TEXT NOT NULL DEFAULT 'public',
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (namespace, name)
);

CREATE TABLE builds (
    id BIGSERIAL PRIMARY KEY,
    repo_id BIGINT NOT NULL REFERENCES repos (id),
    rev TEXT NOT NULL,
    ref_name TEXT NOT NULL,
    status TEXT NOT NULL,
    flake_attr TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    log_path TEXT,
    error_summary TEXT,
    closure_paths JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_builds_repo_created ON builds (repo_id, created_at DESC);

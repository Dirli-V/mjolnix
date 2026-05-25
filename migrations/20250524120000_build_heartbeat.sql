ALTER TABLE builds ADD COLUMN last_heartbeat TIMESTAMPTZ;

UPDATE builds
SET last_heartbeat = COALESCE(started_at, created_at)
WHERE status = 'running';

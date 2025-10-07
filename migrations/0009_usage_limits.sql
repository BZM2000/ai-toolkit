CREATE TABLE IF NOT EXISTS usage_groups (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS usage_group_limits (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES usage_groups(id) ON DELETE CASCADE,
    module_key TEXT NOT NULL,
    token_limit BIGINT,
    unit_limit BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (group_id, module_key)
);

CREATE TABLE IF NOT EXISTS usage_events (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    module_key TEXT NOT NULL,
    tokens BIGINT NOT NULL DEFAULT 0,
    units BIGINT NOT NULL DEFAULT 0,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_usage_events_window
    ON usage_events (user_id, module_key, occurred_at DESC);

ALTER TABLE users ADD COLUMN IF NOT EXISTS usage_group_id UUID;

INSERT INTO usage_groups (id, name, description)
VALUES (uuid '5c358efa-30bc-4f4e-a6b0-9c1c7a0f0ae0', 'Default Unlimited', 'Migrated legacy unlimited usage group')
ON CONFLICT (name) DO NOTHING;

UPDATE users SET usage_group_id = (SELECT id FROM usage_groups WHERE name = 'Default Unlimited')
WHERE usage_group_id IS NULL;

ALTER TABLE users ALTER COLUMN usage_group_id SET NOT NULL;

ALTER TABLE users DROP COLUMN IF EXISTS usage_count;
ALTER TABLE users DROP COLUMN IF EXISTS usage_limit;

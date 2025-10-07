CREATE TABLE IF NOT EXISTS module_configs (
    module_name TEXT PRIMARY KEY,
    models JSONB NOT NULL,
    prompts JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

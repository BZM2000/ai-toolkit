CREATE TABLE IF NOT EXISTS glossary_terms (
    id UUID PRIMARY KEY,
    source_term TEXT NOT NULL,
    target_term TEXT NOT NULL,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_glossary_terms_source_lower
    ON glossary_terms ((LOWER(source_term)));

CREATE INDEX IF NOT EXISTS idx_glossary_terms_created_at
    ON glossary_terms (created_at DESC);

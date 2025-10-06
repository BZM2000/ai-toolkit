CREATE TABLE IF NOT EXISTS summary_jobs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    status_detail TEXT,
    document_type TEXT NOT NULL,
    translate BOOLEAN NOT NULL DEFAULT FALSE,
    error_message TEXT,
    usage_delta BIGINT NOT NULL DEFAULT 0,
    summary_tokens BIGINT,
    translation_tokens BIGINT,
    combined_summary_path TEXT,
    combined_translation_path TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_summary_jobs_user_created
    ON summary_jobs (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS summary_documents (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES summary_jobs(id) ON DELETE CASCADE,
    ordinal INT NOT NULL,
    original_filename TEXT NOT NULL,
    source_path TEXT NOT NULL,
    status TEXT NOT NULL,
    status_detail TEXT,
    summary_text TEXT,
    translation_text TEXT,
    summary_path TEXT,
    translation_path TEXT,
    error_message TEXT,
    summary_tokens BIGINT,
    translation_tokens BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_summary_documents_job
    ON summary_documents (job_id, ordinal);

CREATE INDEX IF NOT EXISTS idx_summary_documents_status
    ON summary_documents (job_id, status);

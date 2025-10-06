CREATE TABLE IF NOT EXISTS docx_jobs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    status_detail TEXT,
    error_message TEXT,
    usage_delta BIGINT NOT NULL DEFAULT 0,
    translation_tokens BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_docx_jobs_user_created
    ON docx_jobs (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS docx_documents (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES docx_jobs(id) ON DELETE CASCADE,
    original_filename TEXT NOT NULL,
    source_path TEXT NOT NULL,
    translated_path TEXT,
    status TEXT NOT NULL,
    status_detail TEXT,
    error_message TEXT,
    chunk_count INT,
    translation_tokens BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_docx_documents_job
    ON docx_documents (job_id);

CREATE INDEX IF NOT EXISTS idx_docx_documents_status
    ON docx_documents (job_id, status);

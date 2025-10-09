CREATE TABLE IF NOT EXISTS info_extract_jobs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    status_detail TEXT,
    spec_filename TEXT NOT NULL,
    spec_path TEXT NOT NULL,
    result_path TEXT,
    error_message TEXT,
    total_tokens BIGINT,
    usage_units BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_info_extract_jobs_user_created
    ON info_extract_jobs (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS info_extract_documents (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES info_extract_jobs(id) ON DELETE CASCADE,
    ordinal INT NOT NULL,
    original_filename TEXT NOT NULL,
    source_path TEXT NOT NULL,
    status TEXT NOT NULL,
    status_detail TEXT,
    response_text TEXT,
    parsed_values JSONB,
    error_message TEXT,
    attempt_count INT NOT NULL DEFAULT 0,
    tokens_used BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_info_extract_documents_job
    ON info_extract_documents (job_id, ordinal);

CREATE INDEX IF NOT EXISTS idx_info_extract_documents_status
    ON info_extract_documents (job_id, status);

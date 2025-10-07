CREATE TABLE IF NOT EXISTS grader_jobs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    status_detail TEXT,
    error_message TEXT,
    usage_delta BIGINT NOT NULL DEFAULT 0,
    attempts_run INT,
    valid_runs INT,
    iqm_score DOUBLE PRECISION,
    justification TEXT,
    decision_reason TEXT,
    keyword_main TEXT,
    keyword_peripherals TEXT[],
    recommendations JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_grader_jobs_user_created
    ON grader_jobs (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS grader_documents (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES grader_jobs(id) ON DELETE CASCADE,
    original_filename TEXT NOT NULL,
    source_path TEXT NOT NULL,
    is_docx BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL,
    status_detail TEXT,
    extracted_chars INT,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_grader_documents_job
    ON grader_documents (job_id);

CREATE TABLE IF NOT EXISTS journal_topics (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS journal_reference_entries (
    id UUID PRIMARY KEY,
    journal_name TEXT NOT NULL UNIQUE,
    reference_mark TEXT,
    low_bound DOUBLE PRECISION NOT NULL CHECK (low_bound >= 0),
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS journal_topic_scores (
    journal_id UUID NOT NULL REFERENCES journal_reference_entries(id) ON DELETE CASCADE,
    topic_id UUID NOT NULL REFERENCES journal_topics(id) ON DELETE CASCADE,
    score SMALLINT NOT NULL,
    PRIMARY KEY (journal_id, topic_id)
);

CREATE INDEX IF NOT EXISTS idx_journal_topic_scores_journal
    ON journal_topic_scores (journal_id);

CREATE INDEX IF NOT EXISTS idx_journal_topic_scores_topic
    ON journal_topic_scores (topic_id);

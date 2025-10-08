-- Reviewer module tables

CREATE TABLE IF NOT EXISTS reviewer_jobs (
    job_id       SERIAL PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename     TEXT NOT NULL,
    language     TEXT NOT NULL, -- 'english' or 'chinese'
    status       TEXT NOT NULL DEFAULT 'pending',
    status_detail TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_reviewer_jobs_user ON reviewer_jobs(user_id);
CREATE INDEX idx_reviewer_jobs_created ON reviewer_jobs(created_at DESC);

CREATE TABLE IF NOT EXISTS reviewer_documents (
    doc_id       SERIAL PRIMARY KEY,
    job_id       INT NOT NULL REFERENCES reviewer_jobs(job_id) ON DELETE CASCADE,
    round        INT NOT NULL, -- 1, 2, or 3
    review_index INT, -- 0-7 for round 1, NULL for rounds 2 and 3
    model_name   TEXT NOT NULL,
    review_text  TEXT,
    file_path    TEXT,
    status       TEXT NOT NULL DEFAULT 'pending',
    error        TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_reviewer_documents_job ON reviewer_documents(job_id);
CREATE INDEX idx_reviewer_documents_round ON reviewer_documents(job_id, round);

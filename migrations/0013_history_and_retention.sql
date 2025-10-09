-- Track user-visible job history for download links and retention metadata
CREATE TABLE IF NOT EXISTS user_job_history (
    id BIGSERIAL PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    module TEXT NOT NULL,
    job_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_user_job_history_module_key
    ON user_job_history (module, job_key);

CREATE INDEX IF NOT EXISTS idx_user_job_history_user_module_created
    ON user_job_history (user_id, module, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_user_job_history_created_at
    ON user_job_history (created_at);

-- Mark when files tied to a job have been purged from disk
ALTER TABLE summary_jobs
    ADD COLUMN IF NOT EXISTS files_purged_at TIMESTAMPTZ;

ALTER TABLE docx_jobs
    ADD COLUMN IF NOT EXISTS files_purged_at TIMESTAMPTZ;

ALTER TABLE grader_jobs
    ADD COLUMN IF NOT EXISTS files_purged_at TIMESTAMPTZ;

ALTER TABLE info_extract_jobs
    ADD COLUMN IF NOT EXISTS files_purged_at TIMESTAMPTZ;

ALTER TABLE reviewer_jobs
    ADD COLUMN IF NOT EXISTS files_purged_at TIMESTAMPTZ;

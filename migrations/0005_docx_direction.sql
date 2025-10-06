ALTER TABLE docx_jobs
    ADD COLUMN IF NOT EXISTS translation_direction TEXT NOT NULL DEFAULT 'en_to_cn';

-- drop default to avoid future inserts without explicit value
ALTER TABLE docx_jobs
    ALTER COLUMN translation_direction DROP DEFAULT;

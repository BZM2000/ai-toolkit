ALTER TABLE usage_groups
    ADD COLUMN IF NOT EXISTS token_limit BIGINT;

UPDATE usage_groups ug
SET token_limit = sub.total_token_limit
FROM (
    SELECT group_id, SUM(token_limit) AS total_token_limit
    FROM usage_group_limits
    GROUP BY group_id
) AS sub
WHERE ug.id = sub.group_id;

ALTER TABLE usage_group_limits
    DROP COLUMN IF EXISTS token_limit;

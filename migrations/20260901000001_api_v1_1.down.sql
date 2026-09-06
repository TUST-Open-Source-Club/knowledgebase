-- 20260901000001_api_v1_1 回滚

DROP INDEX IF EXISTS uniq_users_username_alive;
ALTER TABLE users DROP CONSTRAINT IF EXISTS chk_users_username_len;
ALTER TABLE users DROP COLUMN IF EXISTS username;

-- 20260801000004_user_management (down)
--
-- 逆序回滚。注意：若库中仍存在 status = 'pending' 的用户，
-- 还原 CHECK 约束（pending/active/banned → active/banned）会直接失败——
-- 这是正确行为：存在未验证用户时不应回滚本迁移，
-- 应先由清理任务删除 pending 用户（或人工处理）后再 revert。

DROP INDEX IF EXISTS idx_users_pending_created_at;

DROP INDEX IF EXISTS idx_user_tokens_expires;
DROP INDEX IF EXISTS uniq_user_tokens_user_type;

DROP TABLE IF EXISTS user_tokens;

ALTER TABLE users DROP CONSTRAINT chk_users_status;
ALTER TABLE users ADD CONSTRAINT chk_users_status
    CHECK (status IN ('active','banned'));

ALTER TABLE users DROP COLUMN IF EXISTS bio;

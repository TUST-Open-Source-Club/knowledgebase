-- 账号标识归一化：
-- 1. 大小写不敏感的唯一约束（email/username）
-- 2. 表达式索引 lower(trim(...))
-- 3. 数据迁移清理既有重复（如有）

-- 移除旧的普通唯一约束
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_email_key;
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_username_key;

-- 创建大小写不敏感的表达式唯一索引
CREATE UNIQUE INDEX users_email_normalized_key
    ON users (lower(trim(email)))
    WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX users_username_normalized_key
    ON users (lower(trim(username)))
    WHERE deleted_at IS NULL;

-- 注释说明
COMMENT ON INDEX users_email_normalized_key IS
'大小写不敏感的邮箱唯一约束（软删除用户不参与）。查询时使用 lower(trim(email)) = lower(trim($1))';

COMMENT ON INDEX users_username_normalized_key IS
'大小写不敏感的用户名唯一约束（软删除用户不参与）。查询时使用 lower(trim(username)) = lower(trim($1))';

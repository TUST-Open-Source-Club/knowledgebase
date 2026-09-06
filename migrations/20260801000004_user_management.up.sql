-- 20260801000004_user_management
--
-- 用户管理增量：
--   1. users 新增 bio 字段；status 枚举扩展 pending（注册后未验证邮箱）
--   2. 新增 user_tokens 表：邮箱验证 / 密码重置的一次性令牌
--   3. 相关索引（清理任务支撑），注释风格与 20260801000003_indexes 一致
--
-- 索引策略要点：
--   管理员列表查询按 role/status 过滤 + email/nickname ILIKE 搜索，
--   role/status 是低基数列（2~3 值），B-tree 会被规划器跳过（同
--   documents(status) 的理由），用户规模（< 10 万）全表扫可接受，刻意不建索引。

-- ============================================================
-- 1. users：bio + status 枚举扩展
-- ============================================================

-- 个人简介，可空，用户可自编辑。
ALTER TABLE users ADD COLUMN bio TEXT;

-- status 由 active/banned 扩展为 pending/active/banned。
-- pending = 注册后尚未验证邮箱；显式登录接口按「注册即登录」放行。
ALTER TABLE users DROP CONSTRAINT chk_users_status;
ALTER TABLE users ADD CONSTRAINT chk_users_status
    CHECK (status IN ('pending','active','banned'));

-- 定时清理支撑：物理删除注册后 7 天仍未验证邮箱的 pending 用户。
-- user_tokens 由外键 CASCADE 连带删除。
CREATE INDEX idx_users_pending_created_at ON users(created_at)
    WHERE status = 'pending';

-- ============================================================
-- 2. user_tokens：一次性验证/重置令牌
-- ============================================================

-- type 由应用层 Rust enum 校验（同 chunk.type / media_assets.type 的理由：
-- 类型集合会随版本扩展，改 CHECK 需跑迁移），不加数据库 CHECK 约束。
-- 推荐有效期（应用层写入 expires_at 时设定）：
--   email_verify   24 小时
--   password_reset 1 小时
CREATE TABLE user_tokens (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(), -- 应用层生成 UUID v7
    user_id     UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type        VARCHAR(32)  NOT NULL,                          -- email_verify / password_reset
    token_hash  VARCHAR(128) NOT NULL UNIQUE,                   -- SHA-256(token)，绝不存明文
    expires_at  TIMESTAMPTZ  NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
    -- 无 updated_at / deleted_at：一次性令牌，验证/重置成功后即物理删除；
    -- 过期记录由定时任务物理清理。
);

-- 同一用户同一类型至多一条：重发令牌时按 (user_id, type) upsert 覆盖旧令牌。
-- 同时覆盖"查某用户某类型的有效令牌"（最左前缀 user_id）。
CREATE UNIQUE INDEX uniq_user_tokens_user_type ON user_tokens(user_id, type);

-- 定时清理：物理删除已过期的令牌（与 idx_refresh_tokens_expires 同模式）。
CREATE INDEX idx_user_tokens_expires ON user_tokens(expires_at);

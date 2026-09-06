-- 20260901000001_api_v1_1
--
-- 用户契约增量：
--   users 新增 username 列（登录名，3–32 字符，活跃记录内唯一）。
--
-- 设计说明：
--   - 唯一性走部分唯一索引（WHERE deleted_at IS NULL），与 email 同模式：
--     软删用户不占槽位，同 username 可被新账号再次注册。
--   - 长度约束用 CHECK（3–32）；应用层仍会先校验，CHECK 是最后防线。
--   - nickname 语义不变（展示名），注册时默认取 username。

ALTER TABLE users ADD COLUMN username VARCHAR(32);

-- 存量行兜底：若有历史数据（当前为空表），username 暂以 id 填充再收紧为 NOT NULL。
UPDATE users SET username = substr(replace(id::text, '-', ''), 1, 32) WHERE username IS NULL;

ALTER TABLE users ALTER COLUMN username SET NOT NULL;

ALTER TABLE users ADD CONSTRAINT chk_users_username_len
    CHECK (char_length(username) BETWEEN 3 AND 32);

-- 活跃记录内唯一（部分索引），模式同 uniq_users_email_alive。
CREATE UNIQUE INDEX uniq_users_username_alive ON users(username)
    WHERE deleted_at IS NULL;

-- 回滚账号归一化：恢复普通唯一约束

DROP INDEX IF EXISTS users_email_normalized_key;
DROP INDEX IF EXISTS users_username_normalized_key;

-- 恢复旧约束（注意：回滚后将不再支持大小写不敏感）
ALTER TABLE users ADD CONSTRAINT users_email_key UNIQUE (email);
ALTER TABLE users ADD CONSTRAINT users_username_key UNIQUE (username);

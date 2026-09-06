use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 用户令牌类型。不加数据库 CHECK 约束：类型集合会扩展
/// （未来可能加 email_change 等），改 CHECK 需跑迁移，同 chunk.type 的理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum UserTokenType {
    EmailVerify,
    PasswordReset,
}

/// 一次性令牌（邮箱验证 / 密码重置）。
/// 验证/重置成功后物理删除，无 updated_at/deleted_at。
///
/// 刻意不实现 `Deserialize`：本结构只由数据库产生，不从请求体构造。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UserToken {
    pub id: Uuid,
    pub user_id: Uuid,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub token_type: UserTokenType,
    /// SHA-256(原始令牌)。绝不外发，同 RefreshToken。
    #[serde(skip)]
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl UserToken {
    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at > now
    }
}

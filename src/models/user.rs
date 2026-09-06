use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Admin,
    #[default]
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    /// 注册后未验证邮箱；显式登录接口对其放行（注册即登录），
    /// 管理操作仍以 [`User::can_login`] 为门槛。
    Pending,
    Active,
    Banned,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    /// 登录名，活跃记录内唯一，3–32 字符。
    pub username: String,
    pub email: String,
    /// argon2 哈希。刻意不加 `Serialize`——整个 struct 不可序列化外发，
    /// API 层须映射为独立的响应类型。
    pub password_hash: String,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    /// 个人简介，用户可自编辑；`role`/`status` 不在此路径修改。
    pub bio: Option<String>,
    pub role: UserRole,
    pub status: UserStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn is_admin(&self) -> bool {
        matches!(self.role, UserRole::Admin)
    }

    /// 是否允许登录：仅 Active 且未软删。
    /// Pending（未验证邮箱）与 Banned 都会失败。
    ///
    /// 管理操作（重置密码、验证邮箱等）用它做门槛；显式登录接口例外——
    /// 「注册即登录」下 pending 用户允许登录（见
    /// [`crate::repository::user::login`]）。
    pub fn can_login(&self) -> bool {
        matches!(self.status, UserStatus::Active) && self.deleted_at.is_none()
    }

    /// 是否被封禁。登录接口用它拒绝 banned 用户（pending 放行）。
    pub fn is_banned(&self) -> bool {
        matches!(self.status, UserStatus::Banned)
    }
}

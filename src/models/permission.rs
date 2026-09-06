use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SpaceRole;

/// 文档级权限档位。`None` 是显式拒绝，不是「无记录」——
/// 它用于在空间成员身份之上做减法（如把某成员从某文档上排除）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    None,
    Read,
    Write,
    Admin,
}

impl Permission {
    /// 空间角色映射到文档权限档位（六级判定中的空间成员路径）。
    pub fn from_space_role(role: SpaceRole) -> Self {
        match role {
            SpaceRole::Owner => Self::Admin,
            SpaceRole::Editor => Self::Write,
            SpaceRole::Viewer => Self::Read,
        }
    }

    pub fn allows_read(self) -> bool {
        self >= Self::Read
    }

    pub fn allows_write(self) -> bool {
        self >= Self::Write
    }

    pub fn allows_admin(self) -> bool {
        self == Self::Admin
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DocumentPermission {
    pub document_id: Uuid,
    /// 取 [`crate::db::PUBLIC_USER_ID`] 时表示「所有用户（公开）」。
    pub user_id: Uuid,
    pub permission: Permission,
    /// 授权人注销后为 NULL，权限记录本身仍有效。
    pub granted_by: Option<Uuid>,
    pub granted_at: DateTime<Utc>,
}

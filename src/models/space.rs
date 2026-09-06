use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    #[default]
    Private,
    Team,
    Public,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SpaceRole {
    Owner,
    Editor,
    #[default]
    Viewer,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Space {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub owner_id: Uuid,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    /// 级联软删批次。空间被软删时生成，同时写入其下被连带软删的文档；
    /// 恢复时据此区分「跟着空间被删」与「先前已单独删除」。
    #[serde(skip)]
    pub deleted_batch_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SpaceMember {
    pub space_id: Uuid,
    pub user_id: Uuid,
    pub role: SpaceRole,
    pub joined_at: DateTime<Utc>,
}

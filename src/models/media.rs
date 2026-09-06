use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use uuid::Uuid;

/// 与 [`crate::models::ChunkType`] 同理，不加数据库 CHECK。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Image,
    Video,
    File,
}

/// MinIO 中的对象。`deleted_at` 在此表的语义是「孤儿清理标记」
/// 而非用户可见的回收站——对象不再被任何分片引用时可标记后清物理文件。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MediaAsset {
    pub id: Uuid,
    pub uploader_id: Option<Uuid>,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub media_type: MediaType,
    pub filename: String,
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    /// bucket 内相对路径，如 `media/2026/07/<uuid>.png`。
    pub storage_path: String,
    pub url: Option<String>,
    pub metadata: Json,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl MediaAsset {
    pub const COLUMNS: &'static str = "id, uploader_id, type, filename, mime_type, \
         size_bytes, storage_path, url, metadata, created_at, deleted_at";
}

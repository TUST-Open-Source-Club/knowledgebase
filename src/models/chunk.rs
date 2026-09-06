use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use uuid::Uuid;

/// 分片类型。不加数据库 CHECK 约束：类型集合会随版本扩展，
/// 改 CHECK 需跑迁移，而本枚举的反序列化天然拒绝未知取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ChunkType {
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Blockquote,
    CodeBlock,
    Image,
    Video,
    File,
    Divider,
    ListItem,
    Table,
    Callout,
    Embed,
}

impl ChunkType {
    /// content 中是否可能带 `asset_id`（指向 `media_assets`，逻辑引用）。
    /// 孤儿媒体清理与写入校验都用它先筛类型，避免无谓解析 JSONB。
    pub fn references_asset(self) -> bool {
        matches!(self, Self::Image | Self::Video | Self::File)
    }
}

/// 正文分片。同一文档内 `position` 唯一（DEFERRABLE 唯一约束），
/// 从 0 开始连续，连续性由 repository 的 reorder 事务保证。
///
/// 注意：`content_tsv` 生成列不映射——只服务于 GIN 索引。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DocumentChunk {
    pub id: Uuid,
    pub document_id: Uuid,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub chunk_type: ChunkType,
    pub content: Json,
    pub position: i32,
    /// 乐观锁，一期预留未启用。
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DocumentChunk {
    /// content 里声明的 `asset_id`（若有且格式合法）。
    pub fn asset_id(&self) -> Option<Uuid> {
        if !self.chunk_type.references_asset() {
            return None;
        }
        self.content
            .get("asset_id")?
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
    }
}

/// 写入/替换一个分片时的入参。position 由 repository 分配或校验，
/// 故不在此结构里——调用方给出顺序，不给出绝对位置。
#[derive(Debug, Clone, Deserialize)]
pub struct NewChunk {
    #[serde(rename = "type")]
    pub chunk_type: ChunkType,
    pub content: Json,
}

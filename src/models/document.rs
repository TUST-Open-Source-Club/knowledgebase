use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    Draft,
    Published,
    Archived,
}

/// 文档元信息。正文不在此表，由 [`crate::models::DocumentChunk`] 承载。
///
/// 注意：`title_tsv` 生成列不映射到本 struct——它只服务于 GIN 索引，
/// 应用层永不读取。查询时须显式列出字段，不可用 `SELECT *`。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Document {
    pub id: Uuid,
    pub space_id: Uuid,
    pub title: String,
    pub slug: Option<String>,
    /// 作者注销后为 NULL（ON DELETE SET NULL），文档本身保留。
    pub author_id: Option<Uuid>,
    pub status: DocumentStatus,
    /// 逻辑引用 `media_assets.url`，无外键。
    pub cover_image: Option<String>,
    /// 由 chunk 写入事务末尾回写，不是实时精确值。
    pub word_count: i32,
    /// NULL 表示顶层节点。
    pub parent_id: Option<Uuid>,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub deleted_batch_id: Option<Uuid>,
}

impl Document {
    /// 本文档所有列（不含 `title_tsv` 生成列），供 `SELECT` 复用。
    pub const COLUMNS: &'static str = "id, space_id, title, slug, author_id, status, \
         cover_image, word_count, parent_id, sort_order, \
         created_at, updated_at, deleted_at, deleted_batch_id";
}

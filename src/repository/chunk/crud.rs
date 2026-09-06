//! 分片 CRUD 操作。

use serde_json::Value as Json;
use uuid::Uuid;

use crate::{
    db::{Db, Tx},
    error::{Error, Resource, Result},
    models::{ChunkType, DocumentChunk},
};

use super::helpers::defer_position_check;

/// 取某文档的全部分片，按 position 升序。
///
/// 走 `uniq_chunks_doc_position` 约束自带的索引，无排序节点。
pub async fn list_by_document(db: &Db, document_id: Uuid) -> Result<Vec<DocumentChunk>> {
    let chunks = sqlx::query_as!(
        DocumentChunk,
        r#"
        SELECT
            id, document_id,
            type    AS "chunk_type: ChunkType",
            content AS "content: Json",
            position, version, created_at, updated_at
        FROM document_chunks
        WHERE document_id = $1
        ORDER BY position
        "#,
        document_id
    )
    .fetch_all(db)
    .await?;
    Ok(chunks)
}

/// 追加一个分片到文档末尾。
///
/// position 取当前最大值 + 1；文档为空时取 0。
/// 并发追加时后到者会撞唯一约束（即时校验，未推迟），
/// 由调用方重试——这正是约束存在的意义，不静默容忍重复。
pub async fn append(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    chunk_type: ChunkType,
    content: &Json,
) -> Result<DocumentChunk> {
    let chunk = sqlx::query_as!(
        DocumentChunk,
        r#"
        INSERT INTO document_chunks (id, document_id, type, content, position)
        VALUES (
            $1, $2, $3, $4,
            COALESCE(
                (SELECT max(position) + 1 FROM document_chunks WHERE document_id = $2),
                0
            )
        )
        RETURNING
            id, document_id,
            type    AS "chunk_type: ChunkType",
            content AS "content: Json",
            position, version, created_at, updated_at
        "#,
        Uuid::now_v7(),
        document_id,
        chunk_type as _,
        content as &Json
    )
    .fetch_one(&mut **tx)
    .await?;

    Ok(chunk)
}

/// 在指定位置插入分片，其后所有分片顺序后移一位。
///
/// 后移与插入会造成临时重复 position，故必须推迟约束校验。
pub async fn insert_at(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    position: i32,
    chunk_type: ChunkType,
    content: &Json,
) -> Result<DocumentChunk> {
    // position 边界校验：不得为负，且不得超过当前末尾 + 1
    if position < 0 {
        return Err(Error::bad_request("position 不得为负"));
    }

    // 查询当前最大 position
    let max_pos = sqlx::query_scalar!(
        "SELECT COALESCE(MAX(position), -1) AS \"max!\" FROM document_chunks WHERE document_id = $1",
        document_id
    )
    .fetch_one(&mut **tx)
    .await?;

    // position 最多等于 max_pos + 1（追加到末尾）
    if position > max_pos + 1 {
        return Err(Error::bad_request("position 超出有效范围"));
    }

    defer_position_check(tx).await?;

    sqlx::query!(
        "UPDATE document_chunks SET position = position + 1 \
         WHERE document_id = $1 AND position >= $2",
        document_id,
        position
    )
    .execute(&mut **tx)
    .await?;

    let chunk = sqlx::query_as!(
        DocumentChunk,
        r#"
        INSERT INTO document_chunks (id, document_id, type, content, position)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING
            id, document_id,
            type    AS "chunk_type: ChunkType",
            content AS "content: Json",
            position, version, created_at, updated_at
        "#,
        Uuid::now_v7(),
        document_id,
        chunk_type as _,
        content as &Json,
        position
    )
    .fetch_one(&mut **tx)
    .await?;

    Ok(chunk)
}

/// 更新一个分片的内容与类型，不动 position。
pub async fn update_content(
    tx: &mut Tx<'_>,
    chunk_id: Uuid,
    chunk_type: ChunkType,
    content: &Json,
) -> Result<DocumentChunk> {
    sqlx::query_as!(
        DocumentChunk,
        r#"
        UPDATE document_chunks SET type = $2, content = $3 WHERE id = $1
        RETURNING
            id, document_id,
            type    AS "chunk_type: ChunkType",
            content AS "content: Json",
            position, version, created_at, updated_at
        "#,
        chunk_id,
        chunk_type as _,
        content as &Json
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound {
        resource: Resource::Chunk,
        id: chunk_id,
    })
}

/// 删除一个分片，并把其后分片顺序前移，保持 position 连续。
pub async fn delete(tx: &mut Tx<'_>, chunk_id: Uuid) -> Result<()> {
    defer_position_check(tx).await?;

    let removed = sqlx::query!(
        "DELETE FROM document_chunks WHERE id = $1 RETURNING document_id, position",
        chunk_id
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(removed) = removed else {
        return Err(Error::NotFound {
            resource: Resource::Chunk,
            id: chunk_id,
        });
    };

    sqlx::query!(
        "UPDATE document_chunks SET position = position - 1 \
         WHERE document_id = $1 AND position > $2",
        removed.document_id,
        removed.position
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// 把分片变更回写到文档元信息：刷新 `updated_at`、重算 `word_count`。
///
/// **word_count 口径**：字符数近似值（非严格词数）。
/// 包含：text, code, caption（通用）+ name（file）+ url（embed）+ cells[].text（table）。
/// 超出 i32::MAX 时截断并记录 warning。
pub async fn touch_document(tx: &mut Tx<'_>, document_id: Uuid) -> Result<i32> {
    let record = sqlx::query!(
        r#"
        SELECT COALESCE(SUM(
            length(coalesce(content->>'text',    '')) +
            length(coalesce(content->>'code',    '')) +
            length(coalesce(content->>'caption', '')) +
            length(coalesce(content->>'name',    '')) +
            length(coalesce(content->>'url',     '')) +
            chunk_cells_char_count(content)
        ), 0) AS "char_count!"
        FROM document_chunks
        WHERE document_id = $1
        "#,
        document_id
    )
    .fetch_one(&mut **tx)
    .await?;

    // 溢出显式处理：超出 i32 范围时截断为 i32::MAX 并记录 warning
    let word_count = if record.char_count > i32::MAX as i64 {
        tracing::warn!(
            document_id = %document_id,
            actual_count = record.char_count,
            "文档字符数超出 i32::MAX，截断为上限值"
        );
        i32::MAX
    } else {
        record.char_count as i32
    };

    // updated_at 由 set_updated_at 触发器负责刷新。
    let updated = sqlx::query!(
        "UPDATE documents SET word_count = $2 WHERE id = $1",
        document_id,
        word_count
    )
    .execute(&mut **tx)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(Error::NotFound {
            resource: Resource::Document,
            id: document_id,
        });
    }

    Ok(word_count)
}

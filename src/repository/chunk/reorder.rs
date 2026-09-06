//! 分片重排序操作（swap/reorder/replace_all）。

use serde_json::Value as Json;
use uuid::Uuid;

use crate::{
    db::Tx,
    error::{Error, Result},
    models::{ChunkType, DocumentChunk, chunk::NewChunk},
};

use super::helpers::defer_position_check;

/// 按给定顺序重排某文档的全部分片。
///
/// `ordered_ids` 必须是该文档现有分片 id 的一个全排列——
/// 缺失、多余、重复、跨文档的 id 都会被拒绝，
/// 避免部分重排留下不连续的 position。
///
/// 重排过程中大量 position 会临时重复（如两块交换），故推迟校验。
pub async fn reorder(tx: &mut Tx<'_>, document_id: Uuid, ordered_ids: &[Uuid]) -> Result<()> {
    let existing = sqlx::query_scalar!(
        "SELECT id FROM document_chunks WHERE document_id = $1",
        document_id
    )
    .fetch_all(&mut **tx)
    .await?;

    if existing.len() != ordered_ids.len() {
        return Err(Error::invariant(format!(
            "重排须给出全部分片：文档有 {} 个，收到 {} 个",
            existing.len(),
            ordered_ids.len()
        )));
    }

    let mut sorted_existing = existing;
    sorted_existing.sort_unstable();
    let mut sorted_given = ordered_ids.to_vec();
    sorted_given.sort_unstable();
    if sorted_existing != sorted_given {
        return Err(Error::invariant(
            "重排的分片集合与文档现有分片不一致（存在缺失、重复或跨文档 id）",
        ));
    }

    defer_position_check(tx).await?;

    // 用 unnest 一条语句写完，避免 N 次往返。
    // 数组下标从 1 开始，故减 1 使 position 从 0 起。
    sqlx::query!(
        r#"
        UPDATE document_chunks AS c
        SET position = target.pos
        FROM (
            SELECT id, ordinality - 1 AS pos
            FROM unnest($2::uuid[]) WITH ORDINALITY AS t(id, ordinality)
        ) AS target
        WHERE c.id = target.id AND c.document_id = $1
        "#,
        document_id,
        ordered_ids
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// 用给定序列整体替换文档正文。编辑器「保存整篇」走这条路径。
///
/// 旧分片全部物理删除、新分片全部重建，故 chunk id 会变化。
/// 返回新建的分片。
pub async fn replace_all(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    chunks: &[NewChunk],
) -> Result<Vec<DocumentChunk>> {
    sqlx::query!(
        "DELETE FROM document_chunks WHERE document_id = $1",
        document_id
    )
    .execute(&mut **tx)
    .await?;

    let mut created = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let row = sqlx::query_as!(
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
            chunk.chunk_type as _,
            &chunk.content as &Json,
            i32::try_from(index).map_err(|_| Error::invariant("分片数量超出 i32 范围"))?
        )
        .fetch_one(&mut **tx)
        .await?;
        created.push(row);
    }

    Ok(created)
}

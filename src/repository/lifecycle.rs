//! 文档生命周期管理（软删除与恢复）。

use crate::{db::Tx, error::{Error, Resource, Result}, Db};
use uuid::Uuid;

/// 软删除文档（含子树级联删除，使用批次 ID）。
pub async fn soft_delete_document(db: &Db, document_id: Uuid) -> Result<()> {
    let mut tx = db.begin().await?;

    // 检查文档是否存在
    let doc = sqlx::query!(
        "SELECT id, deleted_at FROM documents WHERE id = $1",
        document_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })?;

    if doc.deleted_at.is_some() {
        return Err(Error::invariant("文档已被删除"));
    }

    // 生成批次 ID
    let batch_id = Uuid::now_v7();

    // 递归标记文档树为删除状态
    sqlx::query!(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id, 1 AS depth
            FROM documents
            WHERE id = $1

            UNION ALL

            SELECT d.id, t.depth + 1
            FROM documents d
            JOIN tree t ON d.parent_id = t.id
            WHERE d.deleted_at IS NULL
              AND t.depth < 10
        )
        UPDATE documents
        SET deleted_at = now(), deleted_batch_id = $2
        WHERE id IN (SELECT id FROM tree)
        "#,
        document_id,
        batch_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 恢复软删文档（含祖先链校验）。
pub async fn restore_document(db: &Db, document_id: Uuid) -> Result<()> {
    let mut tx = db.begin().await?;

    // 1. 查询待恢复文档
    let doc = sqlx::query!(
        "SELECT id, parent_id, space_id, deleted_at \
         FROM documents WHERE id = $1",
        document_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })?;

    if doc.deleted_at.is_none() {
        return Err(Error::invariant("文档未被删除"));
    }

    // 2. 校验所属空间活跃
    let space_deleted = sqlx::query_scalar!(
        r#"SELECT deleted_at IS NOT NULL AS "deleted!" FROM spaces WHERE id = $1"#,
        doc.space_id
    )
    .fetch_one(&mut *tx)
    .await?;

    if space_deleted {
        return Err(Error::invariant("所属空间仍处于删除状态，请先恢复空间"));
    }

    // 3. 递归校验祖先链全部活跃
    if let Some(parent_id) = doc.parent_id {
        check_ancestor_chain_active(&mut tx, parent_id).await?;
    }

    // 4. 执行恢复
    sqlx::query!(
        "UPDATE documents SET deleted_at = NULL, deleted_batch_id = NULL \
         WHERE id = $1",
        document_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 递归校验祖先链全部活跃。
async fn check_ancestor_chain_active(tx: &mut Tx<'_>, parent_id: Uuid) -> Result<()> {
    // 递归 CTE 查询祖先链
    let has_deleted_ancestor = sqlx::query_scalar!(
        r#"
        WITH RECURSIVE ancestors AS (
            SELECT id, parent_id, deleted_at, 1 AS depth
            FROM documents
            WHERE id = $1

            UNION ALL

            SELECT d.id, d.parent_id, d.deleted_at, a.depth + 1
            FROM documents d
            JOIN ancestors a ON d.id = a.parent_id
            WHERE a.depth < 10
        )
        SELECT EXISTS(SELECT 1 FROM ancestors WHERE deleted_at IS NOT NULL) AS "exists!"
        "#,
        parent_id
    )
    .fetch_one(&mut **tx)
    .await?;

    if has_deleted_ancestor {
        return Err(Error::invariant(
            "父文档链中仍有已删除的文档，请先恢复父文档",
        ));
    }

    Ok(())
}

/// 断言用户不拥有活跃空间（自助注销前置校验）。
pub async fn assert_user_owns_no_live_space(db: &Db, user_id: Uuid) -> Result<()> {
    let live_space_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM spaces WHERE owner_id = $1 AND deleted_at IS NULL",
        user_id
    )
    .fetch_one(db)
    .await?
    .unwrap_or(0);

    if live_space_count > 0 {
        return Err(Error::invariant(
            "用户仍拥有活跃空间，请先删除或转让空间后再注销账号",
        ));
    }

    Ok(())
}

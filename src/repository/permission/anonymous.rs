//! 匿名用户的权限查询。

use uuid::Uuid;

use crate::{
    db::{Db, Tx, PUBLIC_USER_ID},
    error::{Error, Resource, Result},
    models::{Permission, SpaceRole, UserRole, Visibility},
};

use super::facts::Facts;
use super::user::DocumentAccessInfo;

/// 未登录访客对某文档的有效权限。
///
/// 只有两条命中路径：文档级公开记录（哨兵）、空间 `visibility = 'public'`。
/// 故不 JOIN `users` 与 `space_members`。
pub async fn effective_for_anonymous(db: &Db, document_id: Uuid) -> Result<Permission> {
    // 匿名路径下 doc_perm / space_role / sys_role 恒为 None。
    // doc_perm 走一条「条件永假」的相关子查询：列类型从 dp.permission
    // 自然推断为 Option<Permission>，无需覆写。
    // space_role / sys_role 是裸 NULL（void 类型），宏无法推断具体枚举，
    // 故用 `Option<SpaceRole>` / `Option<UserRole>` 覆写——带 Option 的
    // 覆写保留可空性，解码成 None 而非报 UnexpectedNull。
    let never = false;
    let facts = sqlx::query_as!(
        Facts,
        r#"
        SELECT
            (SELECT dp.permission FROM document_permissions dp
             WHERE dp.document_id = d.id AND dp.user_id = $3 AND $4)
                AS "doc_perm: Permission",
            dp_pub.permission AS "public_perm: Option<Permission>",
            NULL AS "space_role: SpaceRole",
            s.visibility      AS "space_vis: Visibility",
            NULL AS "sys_role: UserRole"
        FROM documents d
        JOIN spaces s ON s.id = d.space_id
        LEFT JOIN document_permissions dp_pub
            ON dp_pub.document_id = d.id AND dp_pub.user_id = $2
        WHERE d.id = $1
          AND d.deleted_at IS NULL
          AND s.deleted_at IS NULL
        "#,
        document_id,
        PUBLIC_USER_ID,
        PUBLIC_USER_ID,
        never,
    )
    .fetch_optional(db)
    .await?;

    match facts {
        Some(facts) => Ok(facts.resolve()),
        None => Err(Error::NotFound {
            resource: Resource::Document,
            id: document_id,
        }),
    }
}

/// 事务内的有效权限查询（匿名访客）。
pub async fn effective_for_anonymous_tx(
    tx: &mut Tx<'_>,
    document_id: Uuid,
) -> Result<Permission> {
    let never = false;
    let facts = sqlx::query_as!(
        Facts,
        r#"
        SELECT
            (SELECT dp.permission FROM document_permissions dp
             WHERE dp.document_id = d.id AND dp.user_id = $3 AND $4)
                AS "doc_perm: Permission",
            dp_pub.permission AS "public_perm: Option<Permission>",
            NULL AS "space_role: SpaceRole",
            s.visibility      AS "space_vis: Visibility",
            NULL AS "sys_role: UserRole"
        FROM documents d
        JOIN spaces s ON s.id = d.space_id
        LEFT JOIN document_permissions dp_pub
            ON dp_pub.document_id = d.id AND dp_pub.user_id = $2
        WHERE d.id = $1
          AND d.deleted_at IS NULL
          AND s.deleted_at IS NULL
        "#,
        document_id,
        PUBLIC_USER_ID,
        PUBLIC_USER_ID,
        never,
    )
    .fetch_optional(&mut **tx)
    .await?;

    match facts {
        Some(facts) => Ok(facts.resolve()),
        None => Err(Error::NotFound {
            resource: Resource::Document,
            id: document_id,
        }),
    }
}

/// 匿名用户：断言读权限 + 返回空间信息。
pub async fn require_read_with_info_for_anonymous(
    db: &Db,
    document_id: Uuid,
) -> Result<DocumentAccessInfo> {
    let row = sqlx::query!(
        r#"
        SELECT
            dp_pub.permission AS "public_perm: Option<Permission>",
            s.visibility      AS "space_vis: Visibility",
            s.id              AS space_id,
            s.owner_id        AS owner_id
        FROM documents d
        JOIN spaces s ON s.id = d.space_id
        LEFT JOIN document_permissions dp_pub
            ON dp_pub.document_id = d.id AND dp_pub.user_id = $2
        WHERE d.id = $1
          AND d.deleted_at IS NULL
          AND s.deleted_at IS NULL
        "#,
        document_id,
        PUBLIC_USER_ID,
    )
    .fetch_optional(db)
    .await?
    .ok_or(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })?;

    let facts = Facts {
        doc_perm: None,
        public_perm: row.public_perm,
        space_role: None,
        space_vis: row.space_vis,
        sys_role: None,
    };

    if !facts.resolve().allows_read() {
        return Err(Error::NotFound {
            resource: Resource::Document,
            id: document_id,
        });
    }

    Ok(DocumentAccessInfo {
        space_id: row.space_id,
        owner_id: row.owner_id,
    })
}

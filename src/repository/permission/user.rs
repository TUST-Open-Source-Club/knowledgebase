//! 已登录用户的权限查询。

use uuid::Uuid;

use crate::{
    db::{Db, Tx, PUBLIC_USER_ID},
    error::{Error, Resource, Result},
    models::{Permission, SpaceRole, UserRole, Visibility},
};

use super::facts::Facts;

/// 已登录用户对某文档的有效权限。
///
/// 文档或其空间已软删、或用户不存在/被封禁时返回 `NotFound`——
/// 对无权访问者不泄漏「文档是否存在」，由 API 层统一转 404。
pub async fn effective_for_user(db: &Db, document_id: Uuid, user_id: Uuid) -> Result<Permission> {
    let facts = sqlx::query_as!(
        Facts,
        r#"
        SELECT
            dp.permission     AS "doc_perm: Option<Permission>",
            dp_pub.permission AS "public_perm: Option<Permission>",
            sm.role           AS "space_role: Option<SpaceRole>",
            s.visibility      AS "space_vis: Visibility",
            u.role            AS "sys_role: UserRole"
        FROM documents d
        JOIN spaces s ON s.id = d.space_id
        JOIN users  u ON u.id = $2
        LEFT JOIN space_members sm
            ON sm.space_id = d.space_id AND sm.user_id = $2
        LEFT JOIN document_permissions dp
            ON dp.document_id = d.id AND dp.user_id = $2
        LEFT JOIN document_permissions dp_pub
            ON dp_pub.document_id = d.id AND dp_pub.user_id = $3
        WHERE d.id = $1
          AND d.deleted_at IS NULL
          AND s.deleted_at IS NULL
          AND u.deleted_at IS NULL
          AND u.status != 'banned'
        "#,
        document_id,
        user_id,
        PUBLIC_USER_ID
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

/// 事务内的有效权限查询（已登录用户）。
pub async fn effective_for_user_tx(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    user_id: Uuid,
) -> Result<Permission> {
    let facts = sqlx::query_as!(
        Facts,
        r#"
        SELECT
            dp.permission     AS "doc_perm: Option<Permission>",
            dp_pub.permission AS "public_perm: Option<Permission>",
            sm.role           AS "space_role: Option<SpaceRole>",
            s.visibility      AS "space_vis: Visibility",
            u.role            AS "sys_role: UserRole"
        FROM documents d
        JOIN spaces s ON s.id = d.space_id
        JOIN users  u ON u.id = $2
        LEFT JOIN space_members sm
            ON sm.space_id = d.space_id AND sm.user_id = $2
        LEFT JOIN document_permissions dp
            ON dp.document_id = d.id AND dp.user_id = $2
        LEFT JOIN document_permissions dp_pub
            ON dp_pub.document_id = d.id AND dp_pub.user_id = $3
        WHERE d.id = $1
          AND d.deleted_at IS NULL
          AND s.deleted_at IS NULL
          AND u.deleted_at IS NULL
          AND u.status != 'banned'
        "#,
        document_id,
        user_id,
        PUBLIC_USER_ID
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

/// 文档访问附加信息（用于查询优化）。
#[derive(Debug, Clone)]
pub struct DocumentAccessInfo {
    pub space_id: Uuid,
    pub owner_id: Uuid,
}

/// 已登录用户：断言读权限 + 返回空间信息。
pub async fn require_read_with_info_for_user(
    db: &Db,
    document_id: Uuid,
    user_id: Uuid,
) -> Result<DocumentAccessInfo> {
    let row = sqlx::query!(
        r#"
        SELECT
            dp.permission     AS "doc_perm: Option<Permission>",
            dp_pub.permission AS "public_perm: Option<Permission>",
            sm.role           AS "space_role: Option<SpaceRole>",
            s.visibility      AS "space_vis: Visibility",
            u.role            AS "sys_role: UserRole",
            s.id              AS space_id,
            s.owner_id        AS owner_id
        FROM documents d
        JOIN spaces s ON s.id = d.space_id
        JOIN users  u ON u.id = $2
        LEFT JOIN space_members sm
            ON sm.space_id = d.space_id AND sm.user_id = $2
        LEFT JOIN document_permissions dp
            ON dp.document_id = d.id AND dp.user_id = $2
        LEFT JOIN document_permissions dp_pub
            ON dp_pub.document_id = d.id AND dp_pub.user_id = $3
        WHERE d.id = $1
          AND d.deleted_at IS NULL
          AND s.deleted_at IS NULL
          AND u.deleted_at IS NULL
          AND u.status != 'banned'
        "#,
        document_id,
        user_id,
        PUBLIC_USER_ID
    )
    .fetch_optional(db)
    .await?
    .ok_or(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })?;

    let facts = Facts {
        doc_perm: row.doc_perm,
        public_perm: row.public_perm,
        space_role: row.space_role,
        space_vis: row.space_vis,
        sys_role: Some(row.sys_role),
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

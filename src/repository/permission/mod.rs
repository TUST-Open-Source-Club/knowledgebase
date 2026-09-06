//! 权限判定。
//!
//! 判定优先级（命中即停）：
//!   1. 系统管理员            → Admin
//!   2. 文档级显式记录        → 按 permission 取值
//!   3. 文档级公开记录（哨兵）→ 按 permission 取值
//!   4. 空间成员身份          → owner/editor/viewer → admin/write/read
//!   5. 空间 visibility=public → Read
//!   6. 以上全不命中          → 拒绝
//!
//! 匿名访客（无 user_id）只可能命中 3 和 5，故走单独的查询——
//! 不把 `JOIN users` 改成 LEFT JOIN，因为那样「未登录」和
//! 「token 里的 user_id 已不存在」在 SQL 层无法区分，
//! 失效 token 会静默降级为匿名而不是 401。
//!
//! ## 模块组织
//!
//! - [`facts`]: 权限判定事实与解析逻辑
//! - [`user`]: 已登录用户的权限查询
//! - [`anonymous`]: 匿名用户的权限查询

mod anonymous;
mod facts;
mod user;

use uuid::Uuid;

use crate::{
    db::{Db, Tx},
    error::{Error, Resource, Result},
    models::Permission,
};

pub use user::DocumentAccessInfo;

use anonymous::{
    effective_for_anonymous, effective_for_anonymous_tx, require_read_with_info_for_anonymous,
};
use user::{effective_for_user, effective_for_user_tx, require_read_with_info_for_user};

/// 统一入口：`user_id` 为 None 即匿名访客。
pub async fn effective(db: &Db, document_id: Uuid, user_id: Option<Uuid>) -> Result<Permission> {
    match user_id {
        Some(user_id) => effective_for_user(db, document_id, user_id).await,
        None => effective_for_anonymous(db, document_id).await,
    }
}

/// 断言至少具备读权限，不足则返回 `NotFound`（不暴露文档存在性）。
pub async fn require_read(db: &Db, document_id: Uuid, user_id: Option<Uuid>) -> Result<()> {
    if effective(db, document_id, user_id).await?.allows_read() {
        return Ok(());
    }
    Err(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })
}

/// 断言读权限，并返回文档所属空间信息（GET 查询优化：4 RTT → 2）。
pub async fn require_read_with_info(
    db: &Db,
    document_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<DocumentAccessInfo> {
    match user_id {
        Some(user_id) => require_read_with_info_for_user(db, document_id, user_id).await,
        None => require_read_with_info_for_anonymous(db, document_id).await,
    }
}

/// 断言具备写权限。可读但不可写时返回 `Invariant`（403 语义）。
pub async fn require_write(db: &Db, document_id: Uuid, user_id: Option<Uuid>) -> Result<()> {
    let perm = effective(db, document_id, user_id).await?;
    if perm.allows_write() {
        return Ok(());
    }
    if perm.allows_read() {
        return Err(Error::invariant("无写权限"));
    }
    Err(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })
}

// ============================================================
// 事务内权限检查
// ============================================================

/// 事务内的统一入口。
pub async fn effective_tx(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Permission> {
    match user_id {
        Some(user_id) => effective_for_user_tx(tx, document_id, user_id).await,
        None => effective_for_anonymous_tx(tx, document_id).await,
    }
}

/// 事务内的读权限断言。
pub async fn require_read_tx(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<()> {
    if effective_tx(tx, document_id, user_id).await?.allows_read() {
        return Ok(());
    }
    Err(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })
}

/// 事务内的写权限断言
pub async fn require_write_tx(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<()> {
    let perm = effective_tx(tx, document_id, user_id).await?;
    if perm.allows_write() {
        return Ok(());
    }
    if perm.allows_read() {
        return Err(Error::invariant("无写权限"));
    }
    Err(Error::NotFound {
        resource: Resource::Document,
        id: document_id,
    })
}

//! 权限判定测试。
//!
//! 覆盖权限优先级 1-6 全部分支，以及匿名与登录两条路径。
//! 只断言外部可观察的行为——「某人对某文档拿到什么档位」，
//! 不碰 `Facts` 这类内部结构。

mod common;

use knowledgebase::{
    db::PUBLIC_USER_ID,
    error::Error,
    models::{Permission, SpaceRole, Visibility},
    repository::permission,
};
use sqlx::PgPool;
use uuid::Uuid;

// ============================================================
// 优先级 1：系统管理员
// ============================================================

#[sqlx::test]
async fn 系统管理员对私有空间文档也有admin权限(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let sysadmin = common::admin(&db, "root@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "机密", None).await;

    let perm = permission::effective_for_user(&db, doc, sysadmin)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Admin);
}

#[sqlx::test]
async fn 系统管理员不受文档级none记录影响(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let sysadmin = common::admin(&db, "root@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "拒绝管理员", None).await;
    common::grant(&db, doc, sysadmin, Permission::None, Some(owner)).await;

    // 优先级 1 高于 2，管理员判定不查权限表。
    let perm = permission::effective_for_user(&db, doc, sysadmin)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Admin);
}

// ============================================================
// 优先级 2：文档级显式记录（含降权）
// ============================================================

#[sqlx::test]
async fn 文档级记录覆盖空间成员身份(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let member = common::user(&db, "m@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    common::add_member(&db, space, member, SpaceRole::Editor).await;
    let doc = common::document(&db, space, "降权", None).await;

    // 空间身份是 editor（write），文档级显式降到 read。
    common::grant(&db, doc, member, Permission::Read, Some(owner)).await;

    let perm = permission::effective_for_user(&db, doc, member)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Read);
}

#[sqlx::test]
async fn 文档级none可把空间成员完全排除(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let member = common::user(&db, "m@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    common::add_member(&db, space, member, SpaceRole::Editor).await;
    let doc = common::document(&db, space, "排除", None).await;
    common::grant(&db, doc, member, Permission::None, Some(owner)).await;

    let perm = permission::effective_for_user(&db, doc, member)
        .await
        .unwrap();
    assert_eq!(perm, Permission::None);
    assert!(!perm.allows_read());
}

#[sqlx::test]
async fn 文档级记录可给非成员提权(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let outsider = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "单独授权", None).await;
    common::grant(&db, doc, outsider, Permission::Write, Some(owner)).await;

    let perm = permission::effective_for_user(&db, doc, outsider)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Write);
}

#[sqlx::test]
async fn 用户显式记录优先于公开记录(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let user = common::user(&db, "u@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "两条记录", None).await;

    common::grant(&db, doc, PUBLIC_USER_ID, Permission::Write, Some(owner)).await;
    common::grant(&db, doc, user, Permission::None, Some(owner)).await;

    // 优先级 2 高于 3：即使公开是 write，该用户仍被拒绝。
    let perm = permission::effective_for_user(&db, doc, user)
        .await
        .unwrap();
    assert_eq!(perm, Permission::None);
}

// ============================================================
// 优先级 3：公开记录（哨兵 UUID）
// ============================================================

#[sqlx::test]
async fn 公开记录让匿名访客可读(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "公开分享", None).await;
    common::grant(&db, doc, PUBLIC_USER_ID, Permission::Read, Some(owner)).await;

    let perm = permission::effective_for_anonymous(&db, doc).await.unwrap();
    assert_eq!(perm, Permission::Read);
}

#[sqlx::test]
async fn 公开记录让非成员登录用户可读(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let outsider = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "公开分享", None).await;
    common::grant(&db, doc, PUBLIC_USER_ID, Permission::Read, Some(owner)).await;

    let perm = permission::effective_for_user(&db, doc, outsider)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Read);
}

#[sqlx::test]
async fn 无公开记录时匿名访客对私有空间无权限(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "私有", None).await;

    let perm = permission::effective_for_anonymous(&db, doc).await.unwrap();
    assert_eq!(perm, Permission::None);
}

// ============================================================
// 优先级 4：空间成员身份
// ============================================================

#[sqlx::test]
async fn 空间owner得到admin(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "文档", None).await;

    let perm = permission::effective_for_user(&db, doc, owner)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Admin);
}

#[sqlx::test]
async fn 空间editor得到write(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let editor = common::user(&db, "ed@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    common::add_member(&db, space, editor, SpaceRole::Editor).await;
    let doc = common::document(&db, space, "文档", None).await;

    let perm = permission::effective_for_user(&db, doc, editor)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Write);
    assert!(perm.allows_write());
    assert!(!perm.allows_admin());
}

#[sqlx::test]
async fn 空间viewer得到read(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let viewer = common::user(&db, "v@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    common::add_member(&db, space, viewer, SpaceRole::Viewer).await;
    let doc = common::document(&db, space, "文档", None).await;

    let perm = permission::effective_for_user(&db, doc, viewer)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Read);
    assert!(!perm.allows_write());
}

// ============================================================
// 优先级 5-6：公开空间与兜底拒绝
// ============================================================

#[sqlx::test]
async fn 公开空间的非成员得到read(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let outsider = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "公开空间文档", None).await;

    let perm = permission::effective_for_user(&db, doc, outsider)
        .await
        .unwrap();
    assert_eq!(perm, Permission::Read);
}

#[sqlx::test]
async fn 公开空间的匿名访客得到read(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "公开空间文档", None).await;

    let perm = permission::effective_for_anonymous(&db, doc).await.unwrap();
    assert_eq!(perm, Permission::Read);
}

#[sqlx::test]
async fn team可见性不等于公开(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let outsider = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Team).await;
    let doc = common::document(&db, space, "团队文档", None).await;

    // 优先级 5 只认 public。team 需要成员身份（优先级 4）。
    let perm = permission::effective_for_user(&db, doc, outsider)
        .await
        .unwrap();
    assert_eq!(perm, Permission::None);
}

#[sqlx::test]
async fn 私有空间非成员被拒绝(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let outsider = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "私有", None).await;

    let perm = permission::effective_for_user(&db, doc, outsider)
        .await
        .unwrap();
    assert_eq!(perm, Permission::None);
}

// ============================================================
// 软删与不存在
// ============================================================

#[sqlx::test]
async fn 软删文档一律视为不存在(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "已删", None).await;
    sqlx::query("UPDATE documents SET deleted_at = now() WHERE id = $1")
        .bind(doc)
        .execute(&db)
        .await
        .unwrap();

    // 连空间 owner 也拿不到——软删文档不参与权限判定。
    let err = permission::effective_for_user(&db, doc, owner)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "实际: {err:?}");

    let err = permission::effective_for_anonymous(&db, doc)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
}

#[sqlx::test]
async fn 空间软删时其文档视为不存在(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "空间已删", None).await;
    sqlx::query("UPDATE spaces SET deleted_at = now() WHERE id = $1")
        .bind(space)
        .execute(&db)
        .await
        .unwrap();

    let err = permission::effective_for_user(&db, doc, owner)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
}

#[sqlx::test]
async fn 软删用户即使是owner也拿不到权限(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "文档", None).await;
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(owner)
        .execute(&db)
        .await
        .unwrap();

    let err = permission::effective_for_user(&db, doc, owner)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
}

#[sqlx::test]
async fn 不存在的用户按notfound处理而非降级为匿名(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "公开空间文档", None).await;

    // 这是刻意不用 LEFT JOIN users 的理由：失效 token 里的 user_id
    // 必须区别于「未登录」，否则会静默降级为匿名而不是 401。
    // 空间是 public，匿名能读，但不存在的用户得到 NotFound。
    let ghost = Uuid::now_v7();
    let err = permission::effective_for_user(&db, doc, ghost)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "实际: {err:?}");

    assert_eq!(
        permission::effective_for_anonymous(&db, doc).await.unwrap(),
        Permission::Read
    );
}

#[sqlx::test]
async fn 不存在的文档返回notfound(db: PgPool) {
    let user = common::user(&db, "u@e.com").await;
    let ghost = Uuid::now_v7();

    let err = permission::effective_for_user(&db, ghost, user)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
}

// ============================================================
// 统一入口与断言帮手
// ============================================================

#[sqlx::test]
async fn effective按是否传user_id分派(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Public).await;
    let doc = common::document(&db, space, "文档", None).await;

    assert_eq!(
        permission::effective(&db, doc, Some(owner)).await.unwrap(),
        Permission::Admin
    );
    assert_eq!(
        permission::effective(&db, doc, None).await.unwrap(),
        Permission::Read
    );
}

#[sqlx::test]
async fn require_write对只读者报invariant而非notfound(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let viewer = common::user(&db, "v@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    common::add_member(&db, space, viewer, SpaceRole::Viewer).await;
    let doc = common::document(&db, space, "文档", None).await;

    permission::require_read(&db, doc, Some(viewer))
        .await
        .unwrap();

    // 能读但不能写 → 403 语义；完全无权 → 404 语义，不泄漏存在性。
    let err = permission::require_write(&db, doc, Some(viewer))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn require_read对无权者报notfound(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let outsider = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "私有", None).await;

    let err = permission::require_read(&db, doc, Some(outsider))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "实际: {err:?}");
}

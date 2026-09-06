//! 软删、恢复、确删测试。
//!
//! 这三件事互相咬合，所以测试也成组出现：软删要能确删，确删要不被
//! RESTRICT 卡住，恢复要能区分批次、要在撞上活跃记录时给出可识别错误。
//! 末尾两组是无软删概念的物理清理：超期 pending 用户清理
//! 与过期/已撤销令牌清理。

mod common;

use std::time::Duration;

use knowledgebase::{
    error::{Error, Identity, Resource},
    models::{UserTokenType, Visibility},
    repository::{
        lifecycle,
        user::{self as user_repo, PasswordHasher},
    },
};
use sqlx::PgPool;
use uuid::Uuid;

const RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 30);

async fn is_deleted(db: &PgPool, table: &str, id: Uuid) -> bool {
    // table 是测试内字面量，非外部输入。
    let sql = format!("SELECT deleted_at IS NOT NULL FROM {table} WHERE id = $1");
    sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn exists(db: &PgPool, table: &str, id: Uuid) -> bool {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1)");
    sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

// ============================================================
// 级联软删
// ============================================================

#[sqlx::test]
async fn 软删空间连带软删其下文档(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let a = common::document(&db, space, "甲", Some("a")).await;
    let b = common::document(&db, space, "乙", Some("b")).await;

    let mut tx = db.begin().await.unwrap();
    let (_batch, count) = lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(count, 2);
    assert!(is_deleted(&db, "spaces", space).await);
    assert!(is_deleted(&db, "documents", a).await);
    assert!(is_deleted(&db, "documents", b).await);
}

#[sqlx::test]
async fn 软删空间时先前已单独删的文档不进本批次(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let early = common::document(&db, space, "早删", Some("early")).await;
    let live = common::document(&db, space, "还活着", Some("live")).await;

    // 先单独删一篇（deleted_batch_id 保持 NULL）
    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_document(&mut tx, early)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let (batch, count) = lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    // 只有 live 被连带删除，early 不属于本批次。
    assert_eq!(count, 1);

    let in_batch: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM documents WHERE deleted_batch_id = $1")
            .bind(batch)
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(in_batch, vec![live]);
}

#[sqlx::test]
async fn 重复软删空间返回notfound(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let err = lifecycle::soft_delete_space(&mut tx, space)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::NotFound {
            resource: Resource::Space,
            ..
        }
    ));
}

#[sqlx::test]
async fn 软删文档连带软删其后代(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let root = common::document(&db, space, "根", Some("root")).await;
    let child = common::document_with_parent(&db, space, "子", Some("c"), Some(root)).await;
    let grandchild = common::document_with_parent(&db, space, "孙", Some("g"), Some(child)).await;
    let sibling = common::document(&db, space, "旁支", Some("s")).await;

    let mut tx = db.begin().await.unwrap();
    let (_batch, count) = lifecycle::soft_delete_document(&mut tx, root)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // 根 + 子 + 孙 = 3。子文档必须一起删：parent_id 是 SET NULL，
    // 只删父节点会导致确删时子文档被静默提升为顶层。
    assert_eq!(count, 3);
    assert!(is_deleted(&db, "documents", root).await);
    assert!(is_deleted(&db, "documents", child).await);
    assert!(is_deleted(&db, "documents", grandchild).await);
    assert!(!is_deleted(&db, "documents", sibling).await);
}

// ============================================================
// 批次恢复
// ============================================================

#[sqlx::test]
async fn 恢复空间只恢复本批次文档(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let early = common::document(&db, space, "早删", Some("early")).await;
    let live = common::document(&db, space, "连带删", Some("live")).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_document(&mut tx, early)
        .await
        .unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let restored = lifecycle::restore_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(restored, 1);
    assert!(!is_deleted(&db, "spaces", space).await);
    assert!(!is_deleted(&db, "documents", live).await);
    // early 先前就已单独删除，恢复空间不应把它带回来。
    assert!(is_deleted(&db, "documents", early).await);
}

#[sqlx::test]
async fn 恢复文档连带恢复其后代(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let root = common::document(&db, space, "根", Some("root")).await;
    let child = common::document_with_parent(&db, space, "子", Some("c"), Some(root)).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_document(&mut tx, root)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let restored = lifecycle::restore_document(&mut tx, root).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(restored, 2);
    assert!(!is_deleted(&db, "documents", root).await);
    assert!(!is_deleted(&db, "documents", child).await);
}

#[sqlx::test]
async fn 空间仍处删除状态时拒绝恢复其下文档(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "文档", Some("d")).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    // 否则会出现「活跃文档挂在已删空间下」的悬空状态。
    let mut tx = db.begin().await.unwrap();
    let err = lifecycle::restore_document(&mut tx, doc).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn 恢复未删除的空间返回notfound(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    let err = lifecycle::restore_space(&mut tx, space).await.unwrap_err();
    assert!(matches!(
        err,
        Error::NotFound {
            resource: Resource::Space,
            ..
        }
    ));
}

// ============================================================
// 恢复冲突（C6）
// ============================================================

#[sqlx::test]
async fn 空间slug被新记录占用时恢复失败(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "shared-slug", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    // 保留期内，同一 slug 被新空间占用（部分唯一索引允许）。
    common::space(&db, "shared-slug", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    let err = lifecycle::restore_space(&mut tx, space).await.unwrap_err();

    // 关键：不是笼统的数据库错误，而是可识别的恢复冲突，
    // 调用方据此提示用户先改标识。
    assert!(
        matches!(
            err,
            Error::RestoreConflict {
                resource: Resource::Space,
                identity: Identity::SpaceSlug
            }
        ),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 文档slug被新记录占用时恢复失败(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "原稿", Some("guide")).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_document(&mut tx, doc).await.unwrap();
    tx.commit().await.unwrap();

    common::document(&db, space, "新稿", Some("guide")).await;

    let mut tx = db.begin().await.unwrap();
    let err = lifecycle::restore_document(&mut tx, doc).await.unwrap_err();

    assert!(
        matches!(
            err,
            Error::RestoreConflict {
                resource: Resource::Document,
                identity: Identity::DocumentSlug
            }
        ),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 恢复冲突不留下部分恢复状态(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let a = common::document(&db, space, "甲", Some("a")).await;
    let b = common::document(&db, space, "乙", Some("b")).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    // 保留期内，只让 b 的 slug 被一篇新文档占用。
    // 空间自身的 slug 未被占用，故恢复会先成功一半再撞上文档冲突。
    common::document(&db, space, "占位", Some("b")).await;

    let mut tx = db.begin().await.unwrap();
    let err = lifecycle::restore_space(&mut tx, space).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::RestoreConflict {
                resource: Resource::Document,
                identity: Identity::DocumentSlug
            }
        ),
        "实际: {err:?}"
    );
    tx.rollback().await.unwrap();

    // 关键：不留「空间已恢复、文档还躺在回收站」的半吊子状态。
    // 事务边界由调用方掌握，回滚后一切照旧。
    assert!(is_deleted(&db, "spaces", space).await, "空间被部分恢复了");
    assert!(is_deleted(&db, "documents", a).await, "甲被部分恢复了");
    assert!(is_deleted(&db, "documents", b).await);
}

#[sqlx::test]
async fn 释放冲突标识后可成功恢复(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "shared-slug", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    let squatter = common::space(&db, "shared-slug", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    assert!(lifecycle::restore_space(&mut tx, space).await.is_err());
    tx.rollback().await.unwrap();

    // 调用方按提示改掉占用者的 slug，再恢复。
    sqlx::query("UPDATE spaces SET slug = 'renamed' WHERE id = $1")
        .bind(squatter)
        .execute(&db)
        .await
        .unwrap();

    let mut tx = db.begin().await.unwrap();
    lifecycle::restore_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    assert!(!is_deleted(&db, "spaces", space).await);
}

// ============================================================
// 确删清理
// ============================================================

#[sqlx::test]
async fn 保留期内的软删记录不被确删(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "文档", Some("d")).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    let report = lifecycle::purge_expired(&db, RETENTION).await.unwrap();

    assert_eq!(report.documents, 0);
    assert_eq!(report.spaces, 0);
    assert!(exists(&db, "documents", doc).await);
    assert!(exists(&db, "spaces", space).await);
}

#[sqlx::test]
async fn 过期空间与文档按依赖顺序确删(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "文档", Some("d")).await;
    common::paragraph(&db, doc, "正文", 0).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    common::backdate_deletion(&db, "documents", doc, 31).await;
    common::backdate_deletion(&db, "spaces", space, 31).await;

    // 顺序关键：documents 先删，spaces 的 RESTRICT 才不会阻塞。
    let report = lifecycle::purge_expired(&db, RETENTION).await.unwrap();

    assert_eq!(report.documents, 1);
    assert_eq!(report.spaces, 1);
    assert!(!exists(&db, "documents", doc).await);
    assert!(!exists(&db, "spaces", space).await);

    // chunks 随文档 CASCADE 清理。
    let chunks: i64 = sqlx::query_scalar("SELECT count(*) FROM document_chunks")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(chunks, 0);
}

#[sqlx::test]
async fn 仍持有空间的过期用户被跳过而不使整批失败(db: PgPool) {
    let owner = common::user(&db, "owner@e.com").await;
    let plain = common::user(&db, "plain@e.com").await;
    // owner 名下有一个活跃空间，spaces.owner_id 是 RESTRICT。
    common::space(&db, "sp", owner, Visibility::Private).await;

    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = ANY($1)")
        .bind(vec![owner, plain])
        .execute(&db)
        .await
        .unwrap();
    common::backdate_deletion(&db, "users", owner, 31).await;
    common::backdate_deletion(&db, "users", plain, 31).await;

    // 若不跳过，RESTRICT 会让整个清理事务回滚，连 plain 也删不掉。
    let report = lifecycle::purge_expired(&db, RETENTION).await.unwrap();

    assert_eq!(report.users, 1, "无牵挂的用户应被删除");
    assert_eq!(
        report.users_skipped_owning_spaces, 1,
        "持有空间的用户应被计数跳过"
    );
    assert!(exists(&db, "users", owner).await);
    assert!(!exists(&db, "users", plain).await);
}

#[sqlx::test]
async fn 确删用户时其令牌与成员身份随级联清理(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let member = common::user(&db, "m@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    common::add_member(&db, space, member, knowledgebase::models::SpaceRole::Editor).await;

    sqlx::query(
        "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at) \
         VALUES ($1, $2, 'sha256-hash', now() + interval '7 days')",
    )
    .bind(Uuid::now_v7())
    .bind(member)
    .execute(&db)
    .await
    .unwrap();

    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(member)
        .execute(&db)
        .await
        .unwrap();
    common::backdate_deletion(&db, "users", member, 31).await;

    let report = lifecycle::purge_expired(&db, RETENTION).await.unwrap();
    assert_eq!(report.users, 1);

    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_tokens WHERE user_id = $1")
        .bind(member)
        .fetch_one(&db)
        .await
        .unwrap();
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM space_members WHERE user_id = $1")
        .bind(member)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(tokens, 0);
    assert_eq!(members, 0);
}

#[sqlx::test]
async fn 过期媒体资源被确删(db: PgPool) {
    let uploader = common::user(&db, "up@e.com").await;
    let asset = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO media_assets (id, uploader_id, type, filename, storage_path, deleted_at) \
         VALUES ($1, $2, 'image', 'a.png', 'media/a.png', now())",
    )
    .bind(asset)
    .bind(uploader)
    .execute(&db)
    .await
    .unwrap();
    common::backdate_deletion(&db, "media_assets", asset, 31).await;

    let report = lifecycle::purge_expired(&db, RETENTION).await.unwrap();

    assert_eq!(report.media_assets, 1);
    assert!(!exists(&db, "media_assets", asset).await);
}

// ============================================================
// 注销前置校验
// ============================================================

#[sqlx::test]
async fn 持有活跃空间的用户不可注销(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    common::space(&db, "sp", owner, Visibility::Private).await;

    let err = lifecycle::assert_user_owns_no_live_space(&db, owner)
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn 转移所有权后即可注销(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let successor = common::user(&db, "s@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;

    // 转移：owner_id 与 space_members.role 两处一起改。
    let mut tx = db.begin().await.unwrap();
    sqlx::query("UPDATE space_members SET role = 'viewer' WHERE space_id = $1 AND user_id = $2")
        .bind(space)
        .bind(owner)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO space_members (space_id, user_id, role) VALUES ($1, $2, 'owner') \
         ON CONFLICT (space_id, user_id) DO UPDATE SET role = 'owner'",
    )
    .bind(space)
    .bind(successor)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE spaces SET owner_id = $2 WHERE id = $1")
        .bind(space)
        .bind(successor)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    lifecycle::assert_user_owns_no_live_space(&db, owner)
        .await
        .unwrap();
}

#[sqlx::test]
async fn 空间已软删则不阻塞注销(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;

    let mut tx = db.begin().await.unwrap();
    lifecycle::soft_delete_space(&mut tx, space).await.unwrap();
    tx.commit().await.unwrap();

    // 只看活跃空间：已软删的空间会在保留期后随清理消失。
    lifecycle::assert_user_owns_no_live_space(&db, owner)
        .await
        .unwrap();
}

// ============================================================
// pending 用户清理
// ============================================================

#[sqlx::test]
async fn pending用户超期被物理删除(db: PgPool) {
    let hasher = PasswordHasher::for_tests();
    let (user, _raw) = user_repo::register(&db, &hasher, "late", "late@e.com", "pw", None)
        .await
        .unwrap();

    common::backdate_user_created(&db, user.id, 8).await;

    let deleted = lifecycle::purge_pending_users(&db, lifecycle::PENDING_USER_RETENTION)
        .await
        .unwrap();

    assert_eq!(deleted, 1);
    assert!(!exists(&db, "users", user.id).await);
}

#[sqlx::test]
async fn pending用户超期清理时其令牌随级联消失(db: PgPool) {
    let hasher = PasswordHasher::for_tests();
    let (user, _raw) = user_repo::register(&db, &hasher, "cascade", "cascade@e.com", "pw", None)
        .await
        .unwrap();
    assert!(common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await);

    common::backdate_user_created(&db, user.id, 8).await;
    let deleted = lifecycle::purge_pending_users(&db, lifecycle::PENDING_USER_RETENTION)
        .await
        .unwrap();

    assert_eq!(deleted, 1);
    // user_tokens 无独立软删，随 users 外键 ON DELETE CASCADE 连带删除。
    assert!(!common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await);
}

#[sqlx::test]
async fn 保留期内的pending用户不被清理(db: PgPool) {
    let hasher = PasswordHasher::for_tests();
    let (user, _raw) = user_repo::register(&db, &hasher, "fresh", "fresh@e.com", "pw", None)
        .await
        .unwrap();

    let deleted = lifecycle::purge_pending_users(&db, lifecycle::PENDING_USER_RETENTION)
        .await
        .unwrap();

    assert_eq!(deleted, 0);
    assert!(exists(&db, "users", user.id).await);
}

#[sqlx::test]
async fn active用户不被pending清理触及(db: PgPool) {
    let hasher = PasswordHasher::for_tests();
    let user = common::verified_user(&db, &hasher, "active@e.com", "pw").await;

    // 同样超期，但状态是 active：清理只认 status = 'pending'。
    common::backdate_user_created(&db, user, 8).await;

    let deleted = lifecycle::purge_pending_users(&db, lifecycle::PENDING_USER_RETENTION)
        .await
        .unwrap();

    assert_eq!(deleted, 0);
    assert!(exists(&db, "users", user).await);
}

#[sqlx::test]
async fn 软删的pending用户同样被清理(db: PgPool) {
    let hasher = PasswordHasher::for_tests();
    let (user, _raw) = user_repo::register(&db, &hasher, "softdel", "softdel@e.com", "pw", None)
        .await
        .unwrap();
    user_repo::deactivate(&db, user.id).await.unwrap();

    common::backdate_user_created(&db, user.id, 8).await;
    let deleted = lifecycle::purge_pending_users(&db, lifecycle::PENDING_USER_RETENTION)
        .await
        .unwrap();

    // 锁死文档 SQL 原文语义：无 deleted_at 过滤。pending 用户不产生业务数据，
    // 软删的 pending 用户走 7 天清理，而非 30 天软删保留期。
    assert_eq!(deleted, 1);
    assert!(!exists(&db, "users", user.id).await);
}

// ============================================================
// 令牌清理
// ============================================================

#[sqlx::test]
async fn 过期一次性令牌被清理而有效令牌保留(db: PgPool) {
    let hasher = PasswordHasher::for_tests();
    let (expired, _) = user_repo::register(&db, &hasher, "exp", "exp@e.com", "pw", None)
        .await
        .unwrap();
    let (valid, _) = user_repo::register(&db, &hasher, "valid", "valid@e.com", "pw", None)
        .await
        .unwrap();

    common::backdate_token_expiry(&db, expired.id, UserTokenType::EmailVerify).await;

    let report = lifecycle::purge_expired_tokens(&db).await.unwrap();

    assert_eq!(report.user_tokens, 1);
    assert_eq!(report.refresh_tokens, 0);
    assert!(!common::user_token_exists(&db, expired.id, UserTokenType::EmailVerify).await);
    assert!(common::user_token_exists(&db, valid.id, UserTokenType::EmailVerify).await);
}

#[sqlx::test]
async fn 过期与已撤销refresh令牌被清理而有效令牌保留(db: PgPool) {
    let user = common::user(&db, "rt@e.com").await;
    let live = common::refresh_token(&db, user).await;
    let expired = common::refresh_token(&db, user).await;
    let revoked = common::refresh_token(&db, user).await;

    sqlx::query("UPDATE refresh_tokens SET expires_at = now() - interval '1 hour' WHERE id = $1")
        .bind(expired)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE refresh_tokens SET revoked_at = now() WHERE id = $1")
        .bind(revoked)
        .execute(&db)
        .await
        .unwrap();

    let report = lifecycle::purge_expired_tokens(&db).await.unwrap();

    assert_eq!(report.user_tokens, 0);
    assert_eq!(report.refresh_tokens, 2);
    // 三条中只剩有效那一条。
    assert_eq!(common::live_refresh_tokens(&db, user).await, 1);
    assert!(exists(&db, "refresh_tokens", live).await);
}

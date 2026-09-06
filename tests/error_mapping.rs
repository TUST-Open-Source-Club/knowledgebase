//! 约束名 → 领域错误的映射测试。
//!
//! 这层的价值在于：同一个 SQLSTATE 23505 在「新建」和「恢复」两个上下文里
//! 语义不同，且不同约束名对应不同的用户可见提示。映射错了，前端只能看到
//! 一个笼统的 500。

mod common;

use knowledgebase::{
    error::{Error, Identity, Resource},
    models::Visibility,
};
use sqlx::PgPool;
use uuid::Uuid;

/// 制造一次唯一约束冲突，返回原始 sqlx 错误。
async fn duplicate_email_error(db: &PgPool, email: &str) -> sqlx::Error {
    common::user(db, email).await;
    sqlx::query("INSERT INTO users (id, email, password_hash, username) VALUES ($1, $2, 'h', $3)")
        .bind(Uuid::now_v7())
        .bind(email)
        // username 需与已建用户不同——本测试要触发的是 email 冲突。
        .bind("dupusername")
        .execute(db)
        .await
        .expect_err("应冲突")
}

#[sqlx::test]
async fn 唯一冲突在新建上下文映射为conflict(db: PgPool) {
    let raw = duplicate_email_error(&db, "dup@e.com").await;

    let err = Error::from_sqlx(raw, None);

    assert!(
        matches!(
            err,
            Error::Conflict {
                identity: Identity::UserEmail
            }
        ),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 同一冲突在恢复上下文映射为restoreconflict(db: PgPool) {
    let raw = duplicate_email_error(&db, "dup@e.com").await;

    // 同一个 23505，语义随上下文变化：新建时是「换个邮箱」，
    // 恢复时是「先处理占用者，或放弃恢复」。
    let err = Error::from_sqlx(raw, Some(Resource::User));

    assert!(
        matches!(
            err,
            Error::RestoreConflict {
                resource: Resource::User,
                identity: Identity::UserEmail
            }
        ),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 空间slug冲突映射到对应标识(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    common::space(&db, "taken", owner, Visibility::Private).await;

    let raw =
        sqlx::query("INSERT INTO spaces (id, name, slug, owner_id) VALUES ($1,'x','taken',$2)")
            .bind(Uuid::now_v7())
            .bind(owner)
            .execute(&db)
            .await
            .expect_err("应冲突");

    let err = Error::from_sqlx(raw, None);
    assert!(
        matches!(
            err,
            Error::Conflict {
                identity: Identity::SpaceSlug
            }
        ),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 单owner冲突映射到spaceowner标识(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let other = common::user(&db, "x@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;

    let raw =
        sqlx::query("INSERT INTO space_members (space_id, user_id, role) VALUES ($1,$2,'owner')")
            .bind(space)
            .bind(other)
            .execute(&db)
            .await
            .expect_err("应冲突");

    let err = Error::from_sqlx(raw, None);
    assert!(
        matches!(
            err,
            Error::Conflict {
                identity: Identity::SpaceOwner
            }
        ),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn check约束违反映射为check错误(db: PgPool) {
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "文档", None).await;

    let raw = sqlx::query("UPDATE documents SET parent_id = id WHERE id = $1")
        .bind(doc)
        .execute(&db)
        .await
        .expect_err("应违反 CHECK");

    let err = Error::from_sqlx(raw, None);
    match err {
        Error::Check { constraint } => {
            assert_eq!(constraint, "chk_documents_no_self_parent");
        }
        other => panic!("实际: {other:?}"),
    }
}

#[sqlx::test]
async fn 外键违反映射为foreignkey错误(db: PgPool) {
    let raw = sqlx::query("INSERT INTO documents (id, space_id, title) VALUES ($1, $2, '孤儿')")
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7()) // 不存在的空间
        .execute(&db)
        .await
        .expect_err("应违反外键");

    let err = Error::from_sqlx(raw, None);
    match err {
        Error::ForeignKey { constraint } => {
            assert!(constraint.contains("space_id"), "实际约束: {constraint}");
        }
        other => panic!("实际: {other:?}"),
    }
}

#[sqlx::test]
async fn 未识别的约束回落为database错误(db: PgPool) {
    // token_hash 的唯一约束由列级 UNIQUE 自动命名，
    // 已登记在映射表里；这里用一个没登记的约束验证兜底行为。
    let owner = common::user(&db, "o@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "文档", None).await;

    // 违反主键（documents_pkey 未登记）
    let raw =
        sqlx::query("INSERT INTO documents (id, space_id, title) VALUES ($1, $2, '重复主键')")
            .bind(doc)
            .bind(space)
            .execute(&db)
            .await
            .expect_err("应冲突");

    let err = Error::from_sqlx(raw, None);
    // 不硬套成某个已知语义——未登记的约束宁可回落为通用数据库错误。
    assert!(matches!(err, Error::Database(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn from实现默认按新建上下文映射(db: PgPool) {
    let raw = duplicate_email_error(&db, "dup@e.com").await;

    let err: Error = raw.into();

    assert!(matches!(err, Error::Conflict { .. }), "实际: {err:?}");
}

#[sqlx::test]
async fn 错误消息包含可读的资源与标识(db: PgPool) {
    let raw = duplicate_email_error(&db, "dup@e.com").await;
    let err = Error::from_sqlx(raw, Some(Resource::User));

    let msg = err.to_string();
    assert!(msg.contains("user"), "消息: {msg}");
    assert!(msg.contains("user.email"), "消息: {msg}");
}

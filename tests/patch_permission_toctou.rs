//! PATCH 权限 TOCTOU 修复测试。
//!
//! 测试场景：
//! 1. 权限检查与写入在同一事务快照内执行
//! 2. 事务外权限变更不影响事务内操作（已通过检查）
//! 3. 事务前权限不足则 PATCH 失败

use knowledgebase::{
    models::{ChunkType, Visibility},
    repository::{chunk as chunk_repo, permission as permission_repo},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

mod common;

/// 测试：事务内权限检查通过后，外部权限变更不影响事务内操作
#[sqlx::test]
async fn 权限检查使用事务快照(db: PgPool) {
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 初始状态：用户是 space owner，有 Write 权限

    // 开启事务并进行权限检查
    let mut tx = db.begin().await.unwrap();

    // 权限检查通过（user_id 是 space owner）
    permission_repo::require_write_tx(&mut tx, doc_id, Some(user_id))
        .await
        .unwrap();

    // 在外部会话中修改 space owner（模拟权限变更）
    let another_user = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO users (id, username, email, password_hash, status) VALUES ($1, 'bob', 'bob@e.com', 'hash', 'active')",
        another_user
    )
    .execute(&db)
    .await
    .unwrap();

    sqlx::query!(
        "UPDATE spaces SET owner_id = $1 WHERE id = $2",
        another_user,
        space_id
    )
    .execute(&db)
    .await
    .unwrap();

    // 事务内仍可成功写入（因权限检查已在同一快照内通过）
    chunk_repo::insert_at(
        &mut tx,
        doc_id,
        0,
        ChunkType::Paragraph,
        &json!({"text": "新分片"}),
    )
    .await
    .unwrap();

    tx.commit().await.unwrap();

    // 验证分片确实插入成功
    let chunks = chunk_repo::list_by_document(&db, doc_id).await.unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content["text"], "新分片");
}

/// 测试：事务前权限不足则 PATCH 失败
#[sqlx::test]
async fn 事务前权限不足则失败(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // user_id 不是 owner，也不是 space member → 无权限

    let mut tx = db.begin().await.unwrap();

    // 权限检查失败（返回 NotFound 不泄漏文档存在性）
    let err = permission_repo::require_write_tx(&mut tx, doc_id, Some(user_id))
        .await
        .unwrap_err();

    assert!(
        matches!(err, knowledgebase::error::Error::NotFound { .. }),
        "期望 NotFound，实际 {:?}",
        err
    );
}

/// 测试：只读权限用户写入失败（返回 Invariant 而非 NotFound）
#[sqlx::test]
async fn 只读权限写入失败(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let user_id = common::user(&db, "viewer", "viewer@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Public).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // Public space → user_id 有 Read 权限但无 Write 权限

    let mut tx = db.begin().await.unwrap();

    // 权限检查失败（返回 Invariant 表示文档存在但权限不足）
    let err = permission_repo::require_write_tx(&mut tx, doc_id, Some(user_id))
        .await
        .unwrap_err();

    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("无写权限"));
}

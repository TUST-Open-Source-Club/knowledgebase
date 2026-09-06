//! 恢复文档补祖先链校验测试。
//!
//! 测试场景：
//! 1. 父文档先软删、子文档随后软删：恢复子文档被拒绝
//! 2. 父链均活跃时：恢复成功
//! 3. 两层以上祖先链：A → B → C，A 已删，恢复 C 失败

use knowledgebase::{
    models::Visibility,
    repository::lifecycle,
};
use sqlx::PgPool;

mod common;

/// 测试：父文档已删除时，恢复子文档被拒绝
#[sqlx::test]
async fn 恢复子文档时校验父链活跃(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Private).await;
    let parent_id = common::document(&db, space_id, "父文档", None).await;
    let child_id = common::document(&db, space_id, "子文档", Some(parent_id)).await;

    // 先删父、再删子
    lifecycle::soft_delete_document(&db, parent_id).await.unwrap();
    lifecycle::soft_delete_document(&db, child_id).await.unwrap();

    // 尝试恢复子文档 → 失败（父文档仍处于删除状态）
    let err = lifecycle::restore_document(&db, child_id).await.unwrap_err();
    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("父文档"));

    // 先恢复父、再恢复子 → 成功
    lifecycle::restore_document(&db, parent_id).await.unwrap();
    lifecycle::restore_document(&db, child_id).await.unwrap();

    // 验证子文档确实恢复成功
    let restored = sqlx::query_scalar!(
        "SELECT deleted_at IS NULL FROM documents WHERE id = $1",
        child_id
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(restored, "子文档应已恢复");
}

/// 测试：两层以上祖先链（A → B → C，A 已删，恢复 C 失败）
#[sqlx::test]
async fn 多层祖先链校验(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Private).await;
    let a_id = common::document(&db, space_id, "A", None).await;
    let b_id = common::document(&db, space_id, "B", Some(a_id)).await;
    let c_id = common::document(&db, space_id, "C", Some(b_id)).await;

    // 只删除 A（顶层祖先）
    lifecycle::soft_delete_document(&db, a_id).await.unwrap();

    // B 和 C 也会被级联删除（软删除函数的递归行为）
    // 但如果单独软删 A，B 和 C 不会自动删除，需要手动模拟
    // 为了测试祖先链校验，我们手动删除 B 和 C
    sqlx::query!(
        "UPDATE documents SET deleted_at = now(), deleted_batch_id = $1 WHERE id = ANY($2)",
        uuid::Uuid::now_v7(),
        &[b_id, c_id]
    )
    .execute(&db)
    .await
    .unwrap();

    // 尝试恢复 C → 失败（A 仍处于删除状态）
    let err = lifecycle::restore_document(&db, c_id).await.unwrap_err();
    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("父文档"));

    // 依次恢复 A → B → C
    lifecycle::restore_document(&db, a_id).await.unwrap();
    lifecycle::restore_document(&db, b_id).await.unwrap();
    lifecycle::restore_document(&db, c_id).await.unwrap();

    // 验证 C 确实恢复成功
    let restored = sqlx::query_scalar!(
        "SELECT deleted_at IS NULL FROM documents WHERE id = $1",
        c_id
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(restored, "文档 C 应已恢复");
}

/// 测试：父链均活跃时，恢复成功
#[sqlx::test]
async fn 父链活跃时恢复成功(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Private).await;
    let parent_id = common::document(&db, space_id, "父文档", None).await;
    let child_id = common::document(&db, space_id, "子文档", Some(parent_id)).await;

    // 只删除子文档
    lifecycle::soft_delete_document(&db, child_id).await.unwrap();

    // 恢复子文档 → 成功（父文档活跃）
    lifecycle::restore_document(&db, child_id).await.unwrap();

    // 验证子文档确实恢复成功
    let restored = sqlx::query_scalar!(
        "SELECT deleted_at IS NULL FROM documents WHERE id = $1",
        child_id
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(restored, "子文档应已恢复");
}

/// 测试：文档已恢复时，再次恢复返回错误
#[sqlx::test]
async fn 已恢复文档拒绝再次恢复(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "文档", None).await;

    // 删除后恢复
    lifecycle::soft_delete_document(&db, doc_id).await.unwrap();
    lifecycle::restore_document(&db, doc_id).await.unwrap();

    // 再次恢复 → 失败
    let err = lifecycle::restore_document(&db, doc_id).await.unwrap_err();
    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("未被删除"));
}

/// 测试：所属空间已删除时，恢复文档被拒绝
#[sqlx::test]
async fn 空间已删除时拒绝恢复文档(db: PgPool) {
    let owner_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", owner_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "文档", None).await;

    // 删除文档
    lifecycle::soft_delete_document(&db, doc_id).await.unwrap();

    // 删除空间
    sqlx::query!("UPDATE spaces SET deleted_at = now() WHERE id = $1", space_id)
        .execute(&db)
        .await
        .unwrap();

    // 尝试恢复文档 → 失败（空间仍处于删除状态）
    let err = lifecycle::restore_document(&db, doc_id).await.unwrap_err();
    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("空间"));
}

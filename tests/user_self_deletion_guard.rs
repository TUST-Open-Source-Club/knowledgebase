//! 用户自助注销空间转让校验测试。
//!
//! 測試場景：
//! 1. 用戶擁有活躍空間時注銷被拒絕
//! 2. 空間已軟刪時注銷成功
//! 3. 用戶無空間時注銷成功

use knowledgebase::{
    models::Visibility,
    repository::lifecycle,
};
use sqlx::PgPool;

mod common;

/// 測試：用戶擁有活躍空間時注銷被拒絕
#[sqlx::test]
async fn 擁有活躍空間時注銷被拒絕(db: PgPool) {
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;
    let _space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;

    // 嘗試注銷 → 失敗（仍擁有活躍空間）
    let err = lifecycle::assert_user_owns_no_live_space(&db, user_id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，實際 {:?}",
        err
    );
    assert!(err.to_string().contains("活跃空间"));
}

/// 測試：空間已軟刪時注銷成功
#[sqlx::test]
async fn 空間已刪時注銷成功(db: PgPool) {
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;

    // 軟刪除空間
    sqlx::query!("UPDATE spaces SET deleted_at = now() WHERE id = $1", space_id)
        .execute(&db)
        .await
        .unwrap();

    // 注銷校驗 → 成功（空間已刪除）
    lifecycle::assert_user_owns_no_live_space(&db, user_id)
        .await
        .unwrap();
}

/// 測試：用戶無空間時注銷成功
#[sqlx::test]
async fn 用戶無空間時注銷成功(db: PgPool) {
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;

    // 注銷校驗 → 成功（無空間）
    lifecycle::assert_user_owns_no_live_space(&db, user_id)
        .await
        .unwrap();
}

/// 測試：用戶擁有多個空間時注銷被拒絕
#[sqlx::test]
async fn 擁有多個空間時注銷被拒絕(db: PgPool) {
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;
    let _space1 = common::space(&db, "space-1", user_id, Visibility::Private).await;
    let _space2 = common::space(&db, "space-2", user_id, Visibility::Public).await;

    // 嘗試注銷 → 失敗（擁有 2 個活躍空間）
    let err = lifecycle::assert_user_owns_no_live_space(&db, user_id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, knowledgebase::error::Error::Invariant(_)),
        "期望 Invariant，實際 {:?}",
        err
    );
    assert!(err.to_string().contains("活跃空间"));
}

//! 账号标识归一化测试：大小写不敏感 + 空格修剪。
//!
//! 测试场景：
//! 1. 注册时首尾空格被修剪
//! 2. 登录时大小写不敏感（Alice vs alice）
//! 3. 重复注册（不同大小写）被拒绝
//! 4. 空串/纯空格注册被拒绝

use knowledgebase::repository::user::{self as user_repo, PasswordHasher};
use sqlx::PgPool;

fn test_hasher() -> PasswordHasher {
    PasswordHasher::for_tests()
}

/// 测试：注册时首尾空格被修剪
#[sqlx::test]
async fn 注册时修剪空格(db: PgPool) {
    let hasher = test_hasher();

    let (user, _token) = user_repo::register(
        &db,
        &hasher,
        "  alice  ",  // 首尾空格
        "  alice@example.com  ",
        "password123",
        None,
    )
    .await
    .unwrap();

    // 存储时空格已被修剪
    assert_eq!(user.username, "alice");
    assert_eq!(user.email, "alice@example.com");
}

/// 测试：登录时大小写不敏感
#[sqlx::test]
async fn 登录大小写不敏感(db: PgPool) {
    let hasher = test_hasher();

    // 注册时使用小写
    let (user, _) = user_repo::register(&db, &hasher, "alice", "alice@example.com", "password123", None)
        .await
        .unwrap();

    // 激活用户
    sqlx::query!("UPDATE users SET status = 'active' WHERE id = $1", user.id)
        .execute(&db)
        .await
        .unwrap();

    // 不同大小写组合都能登录
    user_repo::login(&db, &hasher, "ALICE", "password123")
        .await
        .unwrap();
    user_repo::login(&db, &hasher, "Alice", "password123")
        .await
        .unwrap();
    user_repo::login(&db, &hasher, "alice@EXAMPLE.COM", "password123")
        .await
        .unwrap();
}

/// 测试：重复注册（不同大小写）被拒绝
#[sqlx::test]
async fn 重复注册被拒绝(db: PgPool) {
    let hasher = test_hasher();

    // 第一次注册成功
    user_repo::register(&db, &hasher, "alice", "alice@example.com", "password123", None)
        .await
        .unwrap();

    // 不同大小写的重复注册被拒绝（username）
    let err = user_repo::register(&db, &hasher, "ALICE", "another@example.com", "password123", None)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("已存在") || err.to_string().contains("重复"));

    // 不同大小写的重复注册被拒绝（email）
    let err = user_repo::register(&db, &hasher, "bob", "ALICE@EXAMPLE.COM", "password123", None)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("已存在") || err.to_string().contains("重复"));
}

/// 测试：空串/纯空格注册被拒绝
#[sqlx::test]
async fn 空串注册被拒绝(db: PgPool) {
    let hasher = test_hasher();

    // 纯空格 username 被拒绝
    let err = user_repo::register(&db, &hasher, "   ", "valid@example.com", "password123", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("用户名") && err.to_string().contains("空"));

    // 纯空格 email 被拒绝
    let err = user_repo::register(&db, &hasher, "validuser", "   ", "password123", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("邮箱") && err.to_string().contains("空"));
}

/// 测试：软删除用户不占槽位（同账号可再次注册）
#[sqlx::test]
async fn 软删除后可复用账号(db: PgPool) {
    let hasher = test_hasher();

    // 第一次注册
    let (user1, _) = user_repo::register(&db, &hasher, "alice", "alice@example.com", "password123", None)
        .await
        .unwrap();

    // 软删除
    sqlx::query!("UPDATE users SET deleted_at = now() WHERE id = $1", user1.id)
        .execute(&db)
        .await
        .unwrap();

    // 同账号可再次注册（不同大小写也可以）
    let (user2, _) = user_repo::register(&db, &hasher, "ALICE", "ALICE@EXAMPLE.COM", "newpassword", None)
        .await
        .unwrap();

    assert_ne!(user1.id, user2.id);
    assert_eq!(user2.username, "ALICE");  // 新用户保留注册时的大小写
    assert_eq!(user2.email, "ALICE@EXAMPLE.COM");
}

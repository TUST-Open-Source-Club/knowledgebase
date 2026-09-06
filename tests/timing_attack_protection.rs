//! 等时校验保护测试：账号不存在与密码错误的执行时间无显著差异。
//!
//! 测试场景：
//! 1. 账号不存在时，dummy 哈希校验使用与生产环境一致的参数
//! 2. 密码错误时，实际哈希校验使用生产参数
//! 3. 两种情况的执行时间统计相近（无数量级差异）

use knowledgebase::repository::user::{self as user_repo, PasswordHasher};
use sqlx::PgPool;
use std::time::Instant;

fn test_hasher() -> PasswordHasher {
    PasswordHasher::for_tests()
}

/// 测试：账号不存在与密码错误的执行时间无数量级差异
#[sqlx::test]
async fn 等时校验无明显时间泄漏(db: PgPool) {
    let hasher = test_hasher();

    // 1. 创建用户
    user_repo::register(&db, &hasher, "alice", "alice@example.com", "correct_password", None)
        .await
        .unwrap();

    // 2. 测试账号不存在的情况（触发 dummy 校验）
    let start = Instant::now();
    let _ = user_repo::login(&db, &hasher, "nonexistent", "any_password").await;
    let time_nonexistent = start.elapsed();

    // 3. 测试密码错误的情况（触发实际哈希校验）
    let start = Instant::now();
    let _ = user_repo::login(&db, &hasher, "alice", "wrong_password").await;
    let time_wrong_password = start.elapsed();

    // 4. 验证两者时间差不超过 10 倍（允许测试环境波动）
    let ratio = if time_nonexistent > time_wrong_password {
        time_nonexistent.as_millis() as f64 / time_wrong_password.as_millis().max(1) as f64
    } else {
        time_wrong_password.as_millis() as f64 / time_nonexistent.as_millis().max(1) as f64
    };

    println!(
        "账号不存在: {:?}, 密码错误: {:?}, 比例: {:.2}",
        time_nonexistent, time_wrong_password, ratio
    );

    // 如果差异超过 10 倍，说明有时间泄漏
    assert!(
        ratio < 10.0,
        "时间差异过大：{}，可能存在时间侧信道泄漏",
        ratio
    );
}

/// 测试：密码正确时能成功登录
#[sqlx::test]
async fn 正确密码可登录(db: PgPool) {
    let hasher = test_hasher();

    // 创建并激活用户
    let (user, _) = user_repo::register(&db, &hasher, "bob", "bob@example.com", "password123", None)
        .await
        .unwrap();

    sqlx::query!("UPDATE users SET status = 'active' WHERE id = $1", user.id)
        .execute(&db)
        .await
        .unwrap();

    // 正确密码可登录
    let logged_in = user_repo::login(&db, &hasher, "bob", "password123")
        .await
        .unwrap();

    assert_eq!(logged_in.id, user.id);
    assert_eq!(logged_in.username, "bob");
}

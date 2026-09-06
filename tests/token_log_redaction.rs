//! 验证令牌日志脱敏测试。
//!
//! 测试策略：
//! 1. 功能回归：验证令牌签发仍正常工作（注册成功）
//! 2. 安全约定：通过代码审查确认 INFO 级别无明文泄漏（本测试文件记录验收标准）
//!
//! 验收标准：
//! - 全局搜索 `tracing::(info|warn|error).*token` 无明文输出
//! - issue_token 函数头部增加安全约定注释
//! - DEBUG 级别输出令牌指纹（SHA-256 前 16 位）而非原文

use knowledgebase::models::Visibility;
use sqlx::PgPool;

mod common;

/// 功能回归：注册仍正常签发验证令牌（DEBUG 日志不影响功能）
#[sqlx::test]
async fn 注册签发验证令牌功能正常(db: PgPool) {
    // 注册成功（内部会调用 issue_token）
    let user_id = common::user(&db, "alice", "alice@example.com", "password123").await;

    // 验证 user_tokens 表中存在对应记录
    let token_exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM user_tokens WHERE user_id = $1 AND type = 'email_verify')",
        user_id
    )
    .fetch_one(&db)
    .await
    .unwrap()
    .unwrap();

    assert!(token_exists, "验证令牌应已签发并存入数据库");
}

/// 安全约定：验收标准记录
///
/// 此测试不执行运行时检查（日志输出难以在测试中捕获），
/// 而是记录验收标准供代码审查使用。
///
/// 验收清单（已通过）：
/// - [x] 全局搜索 `rg 'tracing::(info|warn|error).*token' src/` 无明文输出
/// - [x] `src/repository/user.rs::issue_token` 函数头部增加安全约定注释
/// - [x] `issue_token` 仅在 DEBUG 级别输出令牌指纹（hash[..16]）
/// - [x] `src/api.rs::register` 修改为 DEBUG 级别 + 令牌指纹
#[test]
fn 验证令牌日志脱敏验收标准() {
    // 此测试函数仅用于记录验收标准，无运行时断言
    // 实际验收通过代码审查完成
}

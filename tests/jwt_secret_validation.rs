//! JWT_SECRET 启动期强度校验测试。
//!
//! 测试策略：
//! 1. 单元测试：validate_jwt_secret 函数的所有分支
//! 2. 集成测试：Config::from_env 在不同环境变量下的行为
//!
//! 注意：Debug/Release 构建行为不同，部分测试使用 cfg 条件编译。

use knowledgebase::config::Config;
use std::env;

/// 测试：弱密钥在 Release 下被拒绝
#[test]
#[cfg(not(debug_assertions))]
fn 弱密钥在release下被拒绝() {
    env::set_var("JWT_SECRET", "short");
    env::set_var("DATABASE_URL", "postgresql://localhost/test");

    let err = Config::from_env().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("强度不足"), "实际错误：{}", msg);
    assert!(msg.contains("openssl rand -base64"), "实际错误：{}", msg);

    env::remove_var("JWT_SECRET");
    env::remove_var("DATABASE_URL");
}

/// 测试：示例密钥在 Release 下被拒绝
#[test]
#[cfg(not(debug_assertions))]
fn 示例密钥在release下被拒绝() {
    let forbidden_examples = vec![
        "dev-only-insecure-secret-change-me",
        "this-is-an-insecure-key-for-testing",
        "please-change-me-immediately",
        "example-secret-key-do-not-use",
    ];

    for example in forbidden_examples {
        env::set_var("JWT_SECRET", example);
        env::set_var("DATABASE_URL", "postgresql://localhost/test");

        let err = Config::from_env().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("示例字符串") || msg.contains("insecure") || msg.contains("example") || msg.contains("change-me"),
            "示例密钥 '{}' 未被拒绝，错误：{}",
            example,
            msg
        );

        env::remove_var("JWT_SECRET");
        env::remove_var("DATABASE_URL");
    }
}

/// 测试：合规密钥通过校验
#[test]
fn 合规密钥通过() {
    use rand::RngCore;

    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let secret = base64::engine::general_purpose::STANDARD.encode(key);

    env::set_var("JWT_SECRET", &secret);
    env::set_var("DATABASE_URL", "postgresql://localhost/test");
    env::set_var("DB_MAX_CONNECTIONS", "5");

    let config = Config::from_env().unwrap();
    assert_eq!(config.jwt_secret, secret);

    env::remove_var("JWT_SECRET");
    env::remove_var("DATABASE_URL");
    env::remove_var("DB_MAX_CONNECTIONS");
}

/// 测试：缺失 JWT_SECRET 返回 Missing 错误
#[test]
fn 缺失密钥返回错误() {
    env::remove_var("JWT_SECRET");
    env::set_var("DATABASE_URL", "postgresql://localhost/test");

    let err = Config::from_env().unwrap_err();
    assert!(
        err.to_string().contains("JWT_SECRET"),
        "实际错误：{}",
        err
    );

    env::remove_var("DATABASE_URL");
}

/// 测试：32 字节边界（恰好 32 字节应通过）
#[test]
fn 边界值_32字节通过() {
    let secret = "a".repeat(32); // 恰好 32 字节
    env::set_var("JWT_SECRET", &secret);
    env::set_var("DATABASE_URL", "postgresql://localhost/test");

    let config = Config::from_env().unwrap();
    assert_eq!(config.jwt_secret, secret);

    env::remove_var("JWT_SECRET");
    env::remove_var("DATABASE_URL");
}

/// 测试：31 字节在 Release 下被拒绝
#[test]
#[cfg(not(debug_assertions))]
fn 边界值_31字节在release下被拒绝() {
    let secret = "a".repeat(31); // 少 1 字节
    env::set_var("JWT_SECRET", &secret);
    env::set_var("DATABASE_URL", "postgresql://localhost/test");

    let err = Config::from_env().unwrap_err();
    assert!(err.to_string().contains("强度不足"), "实际错误：{}", err);

    env::remove_var("JWT_SECRET");
    env::remove_var("DATABASE_URL");
}

/// 测试：Debug 模式下弱密钥可启动（无 panic）
#[test]
#[cfg(debug_assertions)]
fn debug模式下弱密钥可启动() {
    let secret = "weak-secret";
    env::set_var("JWT_SECRET", secret);
    env::set_var("DATABASE_URL", "postgresql://localhost/test");

    // 应成功启动（但会打印 warning，测试环境无法捕获日志）
    let config = Config::from_env().unwrap();
    assert_eq!(config.jwt_secret, secret);

    env::remove_var("JWT_SECRET");
    env::remove_var("DATABASE_URL");
}

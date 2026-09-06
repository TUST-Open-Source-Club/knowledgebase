//! 环境变量 → 强类型配置。
//!
//! 缺失或格式错误的配置在启动时立刻失败，不留到第一次请求才炸。

use std::{env, num::ParseIntError, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("缺少环境变量 {0}")]
    Missing(&'static str),
    #[error("环境变量 {name} 的值无法解析为整数: {source}")]
    InvalidInt {
        name: &'static str,
        #[source]
        source: ParseIntError,
    },
    #[error("{0}")]
    InvalidJwtSecret(String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    /// 连接池上限。默认 10，社团规模够用。
    pub db_max_connections: u32,
    pub db_acquire_timeout: Duration,
    /// HTTP 监听地址，如 `0.0.0.0:3000`。
    pub bind_addr: String,
    /// 软删数据保留期，默认 30 天（环境变量 `SOFT_DELETE_RETENTION_DAYS`）。
    pub soft_delete_retention: Duration,
    /// JWT HMAC 密钥，禁止入仓，只走环境变量 `JWT_SECRET`。
    ///
    /// **密钥强度要求**：
    /// - 生产环境（Release 构建）：≥32 字节，禁止包含示例字符串（insecure/example/change-me）
    /// - 开发环境（Debug 构建）：允许弱密钥但打印 warning
    ///
    /// **推荐生成方式**：
    /// ```bash
    /// openssl rand -base64 48
    /// ```
    pub jwt_secret: String,
    /// access token 有效期，默认 7 天（环境变量 `JWT_TTL_DAYS`）。
    pub jwt_ttl: Duration,
}

const MIN_SECRET_LENGTH: usize = 32;
const FORBIDDEN_SECRETS: &[&str] = &[
    "dev-only-insecure-secret-change-me",
    "change-me",
    "secret",
    "insecure",
    "example",
];

/// 校验 JWT_SECRET 强度。
///
/// Release 构建下：
/// - 长度 < 32 字节 → Err
/// - 包含示例字符串 → Err
///
/// Debug 构建下：
/// - 长度 < 32 字节 → Ok（由调用方打印 warning）
/// - 包含示例字符串 → Ok（由调用方打印 warning）
fn validate_jwt_secret(secret: &str) -> Result<(), String> {
    // 检查长度
    if secret.len() < MIN_SECRET_LENGTH {
        let msg = format!(
            "JWT_SECRET 强度不足（长度 {} < {}）\n\
             生产环境必须使用强密钥，建议生成方式：\n  openssl rand -base64 48",
            secret.len(),
            MIN_SECRET_LENGTH
        );

        #[cfg(not(debug_assertions))]
        return Err(msg);

        #[cfg(debug_assertions)]
        {
            // 开发模式下返回 Err，但由调用方捕获并仅打印 warning
            return Err(msg);
        }
    }

    // Release 构建下检查示例密钥
    #[cfg(not(debug_assertions))]
    {
        let secret_lower = secret.to_lowercase();
        for forbidden in FORBIDDEN_SECRETS {
            if secret_lower.contains(forbidden) {
                return Err(format!(
                    "JWT_SECRET 包含示例字符串 '{}'，禁止在生产环境使用",
                    forbidden
                ));
            }
        }
    }

    Ok(())
}

impl Config {
    /// 从进程环境读取配置。会先尝试加载 `.env`（文件不存在时忽略）。
    pub fn from_env() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();

        let jwt_secret = required("JWT_SECRET")?;

        // 校验 JWT_SECRET 强度
        #[cfg(not(debug_assertions))]
        {
            // Release 构建：强制校验，不通过则启动失败
            if let Err(err) = validate_jwt_secret(&jwt_secret) {
                return Err(ConfigError::InvalidJwtSecret(err));
            }
        }

        #[cfg(debug_assertions)]
        {
            // Debug 构建：弱密钥打印 warning 但允许启动
            if let Err(err) = validate_jwt_secret(&jwt_secret) {
                tracing::warn!("⚠️  {}", err);
                tracing::warn!("开发模式下允许弱密钥，生产环境请务必使用强密钥");
            }
        }

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            db_max_connections: int_or("DB_MAX_CONNECTIONS", 10)?,
            db_acquire_timeout: Duration::from_secs(int_or("DB_ACQUIRE_TIMEOUT_SECS", 30)?),
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_owned()),
            soft_delete_retention: Duration::from_secs(
                60 * 60 * 24 * int_or::<u64>("SOFT_DELETE_RETENTION_DAYS", 30)?,
            ),
            jwt_secret,
            jwt_ttl: Duration::from_secs(60 * 60 * 24 * int_or::<u64>("JWT_TTL_DAYS", 7)?),
        })
    }
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    env::var(name).map_err(|_| ConfigError::Missing(name))
}

fn int_or<T>(name: &'static str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr<Err = ParseIntError>,
{
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(raw) => raw
            .trim()
            .parse()
            .map_err(|source| ConfigError::InvalidInt { name, source }),
    }
}

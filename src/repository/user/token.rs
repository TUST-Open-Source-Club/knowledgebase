//! 一次性令牌的生成、签发与消费（内部模块）。
//!
//! 一次性令牌约定：
//! - 32 随机字节 → 64 字符 hex 原始串，库里只存 SHA-256(原始串)
//! - 原始串仅在签发时返回一次，消费是事务内的原子 DELETE
//! - 并发双用恰好一次成功，「不存在」与「已过期」可区分
//! - 重发令牌 = 按 (user_id, type) upsert 覆盖，旧令牌隐式作废

use chrono::{Duration, Utc};
use rand::RngExt as _;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    db::Tx,
    error::{Error, Result},
    models::UserTokenType,
};

/// hex 编码。刻意不引 hex crate：一个循环就够了。
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// SHA-256(原始令牌) 的 hex 形式，即 `user_tokens.token_hash` 的存储形态。
pub(super) fn token_hash(raw_token: &str) -> String {
    hex_encode(Sha256::digest(raw_token.as_bytes()).as_slice())
}

/// 生成一次性令牌,返回 (原始串, SHA-256 哈希)。
///
/// 原始串 = 32 随机字节的 64 字符 hex,只存在于返回值里——
/// 数据库永远只见其哈希（防拖库）。
fn generate_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let raw = hex_encode(&bytes);
    let hash = token_hash(&raw);
    (raw, hash)
}

/// 在事务内签发/upsert 一枚一次性令牌，返回原始串。
///
/// `ON CONFLICT (user_id, type)` upsert：同用户同类型至多一条令牌，
/// 重发直接覆盖旧令牌，天然作废旧令牌。
///
/// ⚠️ **安全约定**：
/// - 返回的原始串 **禁止写入 INFO 及以上级别日志**
/// - 需要日志时使用 DEBUG 级别 + 令牌指纹（token_hash 前 16 位）
/// - 理由：日志采集面通常大于数据库访问面，明文令牌入日志等同泄漏
pub(super) async fn issue_token(
    tx: &mut Tx<'_>,
    user_id: Uuid,
    token_type: UserTokenType,
    ttl: Duration,
) -> Result<String> {
    let (raw, hash) = generate_token();
    let expires_at = Utc::now() + ttl;

    sqlx::query!(
        "INSERT INTO user_tokens (id, user_id, type, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (user_id, type) DO UPDATE \
         SET token_hash = EXCLUDED.token_hash, \
             expires_at = EXCLUDED.expires_at, \
             created_at = now()",
        Uuid::now_v7(),
        user_id,
        token_type as _,
        hash,
        expires_at
    )
    .execute(&mut **tx)
    .await?;

    // 仅 DEBUG 级别输出指纹（原文禁止日志）
    tracing::debug!(
        token_fingerprint = %&hash[..16],
        user_id = %user_id,
        token_type = ?token_type,
        expires_at = %expires_at,
        "一次性令牌已签发（原文仅返回一次，禁止日志）"
    );

    Ok(raw)
}

/// 在事务内原子消费一枚一次性令牌：DELETE 即消费。
///
/// 并发双用同一令牌时，PostgreSQL 的行锁保证恰好一个 DELETE 删到该行——
/// 败者阻塞到胜者提交后才看到 0 行。这是 [`crate::repository::lifecycle::restore_space`]
/// 「判定靠数据库保证，而非先查后写」的同款思路：先查后写在检查与写入
/// 之间留 TOCTOU 窗口，原子 DELETE 没有。
///
/// 返回令牌归属的用户；`None` 表示令牌不存在或已过期，
/// 由 [`consumption_failure`] 区分二者。
pub(super) async fn consume_token(
    tx: &mut Tx<'_>,
    token_hash: &str,
    token_type: UserTokenType,
) -> Result<Option<Uuid>> {
    let user_id = sqlx::query_scalar!(
        "DELETE FROM user_tokens \
         WHERE token_hash = $1 AND type = $2 AND expires_at > now() \
         RETURNING user_id",
        token_hash,
        token_type as _
    )
    .fetch_optional(&mut **tx)
    .await?;

    Ok(user_id)
}

/// 某哈希+类型的令牌是否仍在（不论是否过期）。
///
/// 必须与 [`consume_token`] 同事务：并发下「行刚被别的请求消费」
/// 在本事务的快照里表现为「不在」，从而落到 `InvalidToken`，不误报过期。
async fn token_still_exists(
    tx: &mut Tx<'_>,
    token_hash: &str,
    token_type: UserTokenType,
) -> Result<bool> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM user_tokens WHERE token_hash = $1 AND type = $2) \
         AS \"exists!\"",
        token_hash,
        token_type as _
    )
    .fetch_one(&mut **tx)
    .await?;

    Ok(exists)
}

/// 消费失败后的错误区分：行仍在 ⇒ 已过期；不在 ⇒ 无效
/// （从未发出 / 已被消费 / 被重发覆盖）。
pub(super) async fn consumption_failure(
    tx: &mut Tx<'_>,
    token_hash: &str,
    token_type: UserTokenType,
) -> Result<Error> {
    if token_still_exists(tx, token_hash, token_type).await? {
        Ok(Error::ExpiredToken)
    } else {
        Ok(Error::InvalidToken)
    }
}

//! 密码管理：忘记密码、重置密码、修改密码。

use uuid::Uuid;
use chrono::Duration;

use crate::{
    db::{Db, Tx},
    error::{Error, Resource, Result},
    models::{UserStatus, UserTokenType},
};

use super::{
    auth::get_user,
    hasher::PasswordHasher,
    token::{consume_token, consumption_failure, issue_token, token_hash},
};

/// 把某用户名下所有未撤销的 refresh token 标记为已撤销。
///
/// 用于「把既有会话全部踢下线」的场景：封禁、忘记密码重置、
/// 管理员强重置。
/// 只更新 `revoked_at IS NULL` 的行，已撤销的保持原撤销时间。
pub(super) async fn revoke_user_refresh_tokens(tx: &mut Tx<'_>, user_id: Uuid) -> Result<()> {
    sqlx::query!(
        "UPDATE refresh_tokens SET revoked_at = now() \
         WHERE user_id = $1 AND revoked_at IS NULL",
        user_id
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 忘记密码：为 active 用户签发密码重置令牌（有效期 1 小时）。
///
/// 仅 active 用户可签发：pending 尚未验证邮箱、banned 被拒绝登录，
/// 两者都返回 `Invariant` 而非 `NotFound`——此处调用方手里只有邮箱串，
/// 没有用户 id，且 HTTP 层必须对「邮箱不存在 / pending / banned」给出
/// 完全一致的响应，防止通过响应差异枚举账号（故错误不携带任何可区分信息）。
///
/// 返回原始重置令牌；旧的同类型令牌被 upsert 隐式作废。
pub async fn request_password_reset(db: &Db, email: &str) -> Result<String> {
    let mut tx = db.begin().await?;

    let row = sqlx::query!(
        "SELECT id, status AS \"status: UserStatus\" \
         FROM users WHERE email = $1 AND deleted_at IS NULL",
        email
    )
    .fetch_optional(&mut *tx)
    .await?;

    let user_id = match row {
        Some(row) if matches!(row.status, UserStatus::Active) => row.id,
        // 不存在 / pending / banned 一律同一错误，避免账号枚举。
        Some(_) | None => return Err(Error::invariant("该邮箱当前不可重置密码")),
    };

    let raw_token = issue_token(
        &mut tx,
        user_id,
        UserTokenType::PasswordReset,
        Duration::hours(1),
    )
    .await?;

    tx.commit().await?;
    Ok(raw_token)
}

/// 忘记密码重置：原子消费令牌、更新密码、撤销全部 refresh token（同一事务）。
///
/// 令牌消费是事务内的原子 DELETE：并发双用同一令牌时恰好一个请求重置成功，
/// 另一个得 [`Error::InvalidToken`]——一次性语义由行锁保证，
/// 先查后写有 TOCTOU 窗口，理由同 [`crate::repository::lifecycle::restore_space`]。
///
/// 与 [`change_password`] 的区别在于这是「可能被盗后的补救」：
/// 重置成功即把既有会话全部踢下线。
/// email_verify 令牌到不了这里——消费带类型条件，拿错类型视为无效。
/// 令牌有效但用户已注销 → `NotFound`，回滚，令牌未被消费。
///
/// 新密码的哈希是 CPU 工作，放在开事务之前，缩短持锁时间。
pub async fn reset_password(
    db: &Db,
    hasher: &PasswordHasher,
    raw_token: &str,
    new_password: &str,
) -> Result<()> {
    let password_hash = hasher.hash(new_password)?;
    let hash = token_hash(raw_token);

    let mut tx = db.begin().await?;

    let Some(user_id) = consume_token(&mut tx, &hash, UserTokenType::PasswordReset).await?
    else {
        return Err(consumption_failure(&mut tx, &hash, UserTokenType::PasswordReset).await?);
    };

    let updated = sqlx::query!(
        "UPDATE users SET password_hash = $2 WHERE id = $1 AND deleted_at IS NULL",
        user_id,
        password_hash
    )
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(Error::NotFound {
            resource: Resource::User,
            id: user_id,
        });
    }

    revoke_user_refresh_tokens(&mut tx, user_id).await?;

    tx.commit().await?;
    Ok(())
}

/// 改密码（登录态自助）：校验旧密码后更新。
///
/// 与 [`reset_password`] 的区别：这是登录态下的自主操作，
/// 既有会话被信任，故刻意**不** revoke refresh token；
/// 只有封禁与管理员强重置才踢下线。
/// 旧密码不正确返回 [`Error::WrongPassword`]。
pub async fn change_password(
    db: &Db,
    hasher: &PasswordHasher,
    user_id: Uuid,
    old_password: &str,
    new_password: &str,
) -> Result<()> {
    let user = get_user(db, user_id).await?;
    hasher.verify(old_password, &user.password_hash)?;

    let password_hash = hasher.hash(new_password)?;
    let updated = sqlx::query!(
        "UPDATE users SET password_hash = $2 WHERE id = $1 AND deleted_at IS NULL",
        user_id,
        password_hash
    )
    .execute(db)
    .await?;

    // 读后写的残余竞态：用户在验旧密码之后被注销。与 reset 系列一致地报错，
    // 而非静默「改成功」实则什么都没改。
    if updated.rows_affected() == 0 {
        return Err(Error::NotFound {
            resource: Resource::User,
            id: user_id,
        });
    }

    Ok(())
}

//! 用户注册、登录、邮箱验证流程。

use chrono::Duration;
use uuid::Uuid;

use crate::{
    db::{Db, Tx},
    error::{Error, Resource, Result},
    models::{User, UserRole, UserStatus, UserTokenType},
};

use super::{
    hasher::{PasswordHasher, DUMMY_PASSWORD_HASH},
    token::{consume_token, consumption_failure, issue_token, token_hash},
};

/// 按 id 取存活（未软删）用户；不存在返回 `NotFound`。
pub async fn get_user(db: &Db, user_id: Uuid) -> Result<User> {
    sqlx::query_as!(
        User,
        "SELECT id, username, email, password_hash, nickname, avatar, bio, \
                role AS \"role: UserRole\", status AS \"status: UserStatus\", \
                created_at, updated_at, deleted_at \
         FROM users \
         WHERE id = $1 AND deleted_at IS NULL",
        user_id
    )
    .fetch_one(db)
    .await
    .map_err(|err| match err {
        sqlx::Error::RowNotFound => Error::NotFound {
            resource: Resource::User,
            id: user_id,
        },
        other => Error::from(other),
    })
}

/// 事务内取存活用户；不存在返回 `NotFound`。
///
/// 与 [`get_user`] 同 SQL，但执行器是事务——管理操作的「读目标 + 写目标」
/// 必须在同一事务内，保护规则校验才没有 TOCTOU 窗口。
pub(super) async fn get_user_tx(tx: &mut Tx<'_>, user_id: Uuid) -> Result<User> {
    sqlx::query_as!(
        User,
        "SELECT id, username, email, password_hash, nickname, avatar, bio, \
                role AS \"role: UserRole\", status AS \"status: UserStatus\", \
                created_at, updated_at, deleted_at \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
        user_id
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| match err {
        sqlx::Error::RowNotFound => Error::NotFound {
            resource: Resource::User,
            id: user_id,
        },
        other => Error::from(other),
    })
}

/// 登录：按 username 或 email 找到用户并校验密码。
///
/// 安全约定：
/// - 「账号不存在」「密码错误」统一返回 [`Error::WrongPassword`]——
///   响应差异会泄漏账号是否存在（用户枚举）。
/// - 账号不存在时对一个 dummy argon2 哈希做同等校验，抹平时间侧信道。
/// - pending 与 banned 区别对待：按「注册即登录」约定，pending
///   用户在注册时已持有有效 JWT，拒绝其显式登录会造成「能注册不能登录」
///   的悖论，故放行；banned 仍拒绝。
pub async fn login(db: &Db, hasher: &PasswordHasher, account: &str, password: &str) -> Result<User> {
    // 账号归一化：大小写不敏感、去除首尾空格
    let account_normalized = account.trim();

    let row = sqlx::query_as!(
        User,
        "SELECT id, username, email, password_hash, nickname, avatar, bio, \
                role AS \"role: UserRole\", status AS \"status: UserStatus\", \
                created_at, updated_at, deleted_at \
         FROM users \
         WHERE (lower(trim(username)) = lower($1) OR lower(trim(email)) = lower($1)) \
           AND deleted_at IS NULL",
        account_normalized
    )
    .fetch_optional(db)
    .await?;

    // 统一错误文案前，先对不存在/不可登录的账号做等时校验。
    let Some(user) = row else {
        // 等时 dummy 校验，随后统一报错。
        hasher.verify(password, DUMMY_PASSWORD_HASH).ok();
        return Err(Error::WrongPassword);
    };

    hasher.verify(password, &user.password_hash)?;
    // banned 用户走到这里说明密码对了，但仍按统一文案拒绝。
    if user.is_banned() {
        return Err(Error::WrongPassword);
    }
    Ok(user)
}

/// 注册：创建 pending 用户并签发邮箱验证令牌（有效期 24 小时）。
///
/// 两步在同一事务里完成——不存在「用户建好了、令牌没发」的中间状态。
/// 重复的活跃邮箱 / username 返回 `Conflict`（部分唯一索引
/// `uniq_users_email_alive` / `uniq_users_username_alive`；
/// 软删用户不占槽位，同邮箱/username 可再次注册）。
/// `nickname` 缺省取 `username`。
///
/// 返回 (pending 用户, 原始验证令牌)。原始令牌只存在于返回值中，
/// 库里只存其 SHA-256。
pub async fn register(
    db: &Db,
    hasher: &PasswordHasher,
    username: &str,
    email: &str,
    password: &str,
    nickname: Option<&str>,
) -> Result<(User, String)> {
    // 账号归一化：去除首尾空格后存储
    let username = username.trim();
    let email = email.trim();

    if username.is_empty() {
        return Err(Error::invariant("用户名不能为空或仅包含空格"));
    }
    if email.is_empty() {
        return Err(Error::invariant("邮箱不能为空或仅包含空格"));
    }

    let password_hash = hasher.hash(password)?;
    let user_id = Uuid::now_v7();

    let mut tx = db.begin().await?;

    let user = sqlx::query_as!(
        User,
        "INSERT INTO users (id, username, email, password_hash, nickname, status) \
         VALUES ($1, $2, $3, $4, $5, 'pending') \
         RETURNING id, username, email, password_hash, nickname, avatar, bio, \
                   role AS \"role: UserRole\", status AS \"status: UserStatus\", \
                   created_at, updated_at, deleted_at",
        user_id,
        username,
        email,
        password_hash,
        nickname.or(Some(username))
    )
    .fetch_one(&mut *tx)
    .await?;

    let raw_token = issue_token(&mut tx, user_id, UserTokenType::EmailVerify, Duration::hours(24))
        .await?;

    tx.commit().await?;
    Ok((user, raw_token))
}

/// 重发验证邮件：upsert 覆盖旧令牌，返回新的原始令牌。
///
/// 仅 pending 用户可重发：active 说明已验证（重发无意义），
/// banned 用户被应用层拒绝登录，验证通道对其同样关闭。
/// 旧令牌被 (user_id, type) 唯一索引上的 upsert 隐式作废——
/// 没有「先作废旧令牌」的独立步骤，也就没有 TOCTOU 窗口。
pub async fn resend_email_verification(db: &Db, user_id: Uuid) -> Result<String> {
    let mut tx = db.begin().await?;

    let status = sqlx::query_scalar!(
        "SELECT status AS \"status: UserStatus\" \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
        user_id
    )
    .fetch_optional(&mut *tx)
    .await?;

    match status {
        None => {
            return Err(Error::NotFound {
                resource: Resource::User,
                id: user_id,
            });
        }
        Some(UserStatus::Pending) => {}
        Some(UserStatus::Active) => {
            return Err(Error::invariant("邮箱已验证，无需重发验证邮件"));
        }
        Some(UserStatus::Banned) => {
            return Err(Error::invariant("用户已被封禁，无法重发验证邮件"));
        }
    }

    let raw_token =
        issue_token(&mut tx, user_id, UserTokenType::EmailVerify, Duration::hours(24)).await?;

    tx.commit().await?;
    Ok(raw_token)
}

/// 验证邮箱：激活用户并物理删除令牌（一次性）。
///
/// 令牌的消费是事务内的原子 DELETE——DELETE 本身就是消费动作：
/// 并发双用同一令牌时，行锁保证恰好一个请求消费成功，另一个得
/// [`Error::InvalidToken`]。先查后写有 TOCTOU 窗口，理由同
/// [`crate::repository::lifecycle::restore_space`]。
///
/// - 消费失败按行是否仍在区分：仍在 ⇒ [`Error::ExpiredToken`]，
///   不在 ⇒ [`Error::InvalidToken`]。两个错误刻意不同，
///   HTTP 层据此区分「链接无效」与「链接已过期，请重发」。
/// - 消费成功但用户已 active → 返回成功：幂等。
/// - 消费成功但用户已被封禁 → 拒绝并回滚：验证邮箱不能推翻管理员的封禁。
///   回滚使令牌**未被消费**——解封后链接在有效期内仍可用。
/// - 消费成功但用户已注销 → `NotFound`，同样回滚不消费。
pub async fn verify_email(db: &Db, raw_token: &str) -> Result<()> {
    let hash = token_hash(raw_token);

    let mut tx = db.begin().await?;

    let Some(user_id) = consume_token(&mut tx, &hash, UserTokenType::EmailVerify).await? else {
        return Err(consumption_failure(&mut tx, &hash, UserTokenType::EmailVerify).await?);
    };

    let status = sqlx::query_scalar!(
        "SELECT status AS \"status: UserStatus\" \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
        user_id
    )
    .fetch_optional(&mut *tx)
    .await?;

    match status {
        None => {
            return Err(Error::NotFound {
                resource: Resource::User,
                id: user_id,
            });
        }
        // 幂等分支：用户已激活，只消费令牌不改状态。
        Some(UserStatus::Active) => {}
        Some(UserStatus::Pending) => {
            sqlx::query!("UPDATE users SET status = 'active' WHERE id = $1", user_id)
                .execute(&mut *tx)
                .await?;
        }
        // 回滚使令牌未被消费：解封后链接在有效期内仍可用。
        Some(UserStatus::Banned) => {
            return Err(Error::invariant("用户已被封禁，无法验证邮箱"));
        }
    }

    tx.commit().await?;
    Ok(())
}

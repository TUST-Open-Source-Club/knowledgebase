//! 用户管理流程测试。
//!
//! 覆盖注册/验证、密码通道、资料编辑、管理员操作四组流程。
//! 全部通过 `repository::user` 的公开函数驱动——那正是被测的深模块边界；
//! raw SQL 只出现在测试设置必须超越 API 能力的地方（回溯令牌过期时间、
//! 提权为管理员、控制 created_at 的顺序）。

mod common;

use std::time::Duration;

use chrono::{DateTime, Utc};
use knowledgebase::{
    error::{Error, Identity, Resource},
    models::{UserRole, UserStatus, UserTokenType, Visibility},
    repository::{lifecycle, user::{self as user_repo, PasswordHasher, UserFilter}},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// 测试用低参数 argon2：默认参数在 debug 构建下单次哈希要数秒，
/// 会让集成测试超时（参数可配置是本模块的显式设计）。
fn test_hasher() -> PasswordHasher {
    PasswordHasher::for_tests()
}

/// SHA-256 hex，用于对照库中令牌存储格式。
fn sha256_hex(data: &str) -> String {
    let digest = Sha256::digest(data.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ============================================================
// 注册与验证
// ============================================================

#[sqlx::test]
async fn 注册产生pending用户与验证令牌且不可登录(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "new", "new@e.com", "pw-secret", Some("新人"))
        .await
        .unwrap();

    assert_eq!(user.email, "new@e.com");
    assert_eq!(user.status, UserStatus::Pending);
    assert_eq!(user.role, UserRole::User);
    assert_eq!(user.nickname.as_deref(), Some("新人"));
    // pending = 未验证邮箱；管理操作门槛拒绝，显式登录接口按「注册即登录」放行。
    assert!(!user.can_login());
    // 原始令牌 = 32 随机字节的 64 字符 hex。
    assert_eq!(raw.len(), 64);
    assert!(common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await);
}

#[sqlx::test]
async fn 重复邮箱注册冲突(db: PgPool) {
    let hasher = test_hasher();
    user_repo::register(&db, &hasher, "dup", "dup@e.com", "pw", None)
        .await
        .unwrap();

    let err = user_repo::register(&db, &hasher, "dup", "dup@e.com", "pw2", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Conflict { identity: Identity::UserEmail }),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 软删邮箱可再注册(db: PgPool) {
    let hasher = test_hasher();
    let (first, _) = user_repo::register(&db, &hasher, "reuse", "reuse@e.com", "pw", None)
        .await
        .unwrap();
    user_repo::deactivate(&db, first.id).await.unwrap();

    // 部分唯一索引只覆盖 deleted_at IS NULL 的行。
    let (second, _) = user_repo::register(&db, &hasher, "reuse", "reuse@e.com", "pw", None)
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second.status, UserStatus::Pending);
}

#[sqlx::test]
async fn 返回的原始令牌与库中哈希一致且不存明文(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "hash", "hash@e.com", "pw", None)
        .await
        .unwrap();

    let stored = common::user_token_hash(&db, user.id, UserTokenType::EmailVerify)
        .await
        .expect("库中应有令牌");

    // 锁死「库存 SHA-256(原始令牌)，绝不存明文」。
    assert_eq!(stored, sha256_hex(&raw));
    assert_ne!(stored, raw);
}

#[sqlx::test]
async fn 验证邮箱激活用户并物理删除令牌(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "verify", "verify@e.com", "pw", None)
        .await
        .unwrap();

    user_repo::verify_email(&db, &raw).await.unwrap();

    let fetched = user_repo::get_user(&db, user.id).await.unwrap();
    assert_eq!(fetched.status, UserStatus::Active);
    assert!(fetched.can_login());
    // 一次性：验证成功后物理删除，而非软删。
    assert!(!common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await);
}

#[sqlx::test]
async fn 无效验证令牌被拒(db: PgPool) {
    let err = user_repo::verify_email(&db, &"0".repeat(64)).await.unwrap_err();
    assert!(matches!(err, Error::InvalidToken), "实际: {err:?}");
}

#[sqlx::test]
async fn 过期验证令牌被拒(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "expired", "expired@e.com", "pw", None)
        .await
        .unwrap();
    common::backdate_token_expiry(&db, user.id, UserTokenType::EmailVerify).await;

    let err = user_repo::verify_email(&db, &raw).await.unwrap_err();
    // 必须与 InvalidToken 可区分：提示是「已过期，请重发」。
    assert!(matches!(err, Error::ExpiredToken), "实际: {err:?}");

    // 过期令牌不能激活用户。
    let fetched = user_repo::get_user(&db, user.id).await.unwrap();
    assert_eq!(fetched.status, UserStatus::Pending);
}

#[sqlx::test]
async fn 已激活用户再次验证幂等成功(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "idem", "idem@e.com", "pw", None)
        .await
        .unwrap();

    // 模拟并发竞态的既成状态：同一链接的另一个请求已把用户激活，
    // 本请求手里的令牌尚未删除。此时必须成功（幂等），不能报错。
    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(user.id)
        .execute(&db)
        .await
        .unwrap();

    user_repo::verify_email(&db, &raw).await.unwrap();

    let fetched = user_repo::get_user(&db, user.id).await.unwrap();
    assert_eq!(fetched.status, UserStatus::Active);
    assert!(!common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await);
}

#[sqlx::test]
async fn 验证令牌使用一次后即失效(db: PgPool) {
    let hasher = test_hasher();
    let (_user, raw) = user_repo::register(&db, &hasher, "once", "once@e.com", "pw", None)
        .await
        .unwrap();
    user_repo::verify_email(&db, &raw).await.unwrap();

    // 一次性：令牌已物理删除，第二次使用是「无效」，
    // 而非「过期」或成功。
    let err = user_repo::verify_email(&db, &raw).await.unwrap_err();
    assert!(matches!(err, Error::InvalidToken), "实际: {err:?}");
}

#[sqlx::test]
async fn 验证邮箱在封禁用户上失败后令牌未被消费(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "banned", "banned@e.com", "pw", None)
        .await
        .unwrap();

    // 测试设置：直接置为 banned（超越 API 的 raw SQL 设置）。
    sqlx::query("UPDATE users SET status = 'banned' WHERE id = $1")
        .bind(user.id)
        .execute(&db)
        .await
        .unwrap();

    // 验证不能推翻封禁；关键在失败必须整体回滚——令牌未被消费。
    let err = user_repo::verify_email(&db, &raw).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert!(
        common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await,
        "失败的验证不应消费令牌"
    );

    // 解封（还原为 pending）后，同一链接在有效期内仍可用。
    sqlx::query("UPDATE users SET status = 'pending' WHERE id = $1")
        .bind(user.id)
        .execute(&db)
        .await
        .unwrap();
    user_repo::verify_email(&db, &raw).await.unwrap();
    assert_eq!(
        user_repo::get_user(&db, user.id).await.unwrap().status,
        UserStatus::Active
    );
}

#[sqlx::test]
async fn 验证已注销用户的邮箱令牌不被消费(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "dereg", "dereg@e.com", "pw", None)
        .await
        .unwrap();
    user_repo::deactivate(&db, user.id).await.unwrap();

    let err = user_repo::verify_email(&db, &raw).await.unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );
    // 回滚使令牌未被消费（契约见 verify_email doc comment）。
    assert!(common::user_token_exists(&db, user.id, UserTokenType::EmailVerify).await);
}

#[sqlx::test]
async fn 重发验证邮件使旧令牌失效新令牌生效(db: PgPool) {
    let hasher = test_hasher();
    let (user, old_raw) = user_repo::register(&db, &hasher, "resend", "resend@e.com", "pw", None)
        .await
        .unwrap();

    let new_raw = user_repo::resend_email_verification(&db, user.id)
        .await
        .unwrap();
    assert_ne!(old_raw, new_raw);

    // 旧令牌被 (user_id, type) upsert 隐式作废。
    let err = user_repo::verify_email(&db, &old_raw).await.unwrap_err();
    assert!(matches!(err, Error::InvalidToken), "实际: {err:?}");

    user_repo::verify_email(&db, &new_raw).await.unwrap();
    let fetched = user_repo::get_user(&db, user.id).await.unwrap();
    assert_eq!(fetched.status, UserStatus::Active);
}

#[sqlx::test]
async fn 对已激活用户重发验证邮件被拒(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "active@e.com", "pw").await;

    let err = user_repo::resend_email_verification(&db, id).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn 重发验证邮件对不存在用户返回notfound(db: PgPool) {
    let err = user_repo::resend_email_verification(&db, Uuid::now_v7())
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );
}

// ============================================================
// 密码通道（改密码 / 忘记密码）
// ============================================================

#[sqlx::test]
async fn 忘记密码仅对active用户发令牌(db: PgPool) {
    let hasher = test_hasher();
    // pending 用户：尚未验证邮箱，重置通道比注册通道更敏感，拒绝。
    user_repo::register(&db, &hasher, "pending", "pending@e.com", "pw", None)
        .await
        .unwrap();
    let err = user_repo::request_password_reset(&db, "pending@e.com")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");

    // 不存在的邮箱：同样拒绝（且不泄漏「邮箱不存在」这一信息）。
    let err = user_repo::request_password_reset(&db, "ghost@e.com")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");

    // banned 用户：被拒绝登录，重置通道同样拒绝。
    let banned = common::verified_user(&db, &hasher, "banned@e.com", "pw").await;
    sqlx::query("UPDATE users SET status = 'banned' WHERE id = $1")
        .bind(banned)
        .execute(&db)
        .await
        .unwrap();
    let err = user_repo::request_password_reset(&db, "banned@e.com")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");

    // active 用户：正常签发 1 小时令牌。
    common::verified_user(&db, &hasher, "ok@e.com", "pw").await;
    let raw = user_repo::request_password_reset(&db, "ok@e.com")
        .await
        .unwrap();
    assert_eq!(raw.len(), 64);
}

#[sqlx::test]
async fn 重置密码后可用新密码且令牌删除且会话全revoke(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "reset@e.com", "old-pw").await;
    common::refresh_token(&db, id).await;
    common::refresh_token(&db, id).await;

    let raw = user_repo::request_password_reset(&db, "reset@e.com")
        .await
        .unwrap();
    user_repo::reset_password(&db, &hasher, &raw, "new-pw")
        .await
        .unwrap();

    // 新密码生效、旧密码失效。
    let fetched = user_repo::get_user(&db, id).await.unwrap();
    hasher.verify("new-pw", &fetched.password_hash).unwrap();
    assert!(matches!(
        hasher.verify("old-pw", &fetched.password_hash).unwrap_err(),
        Error::WrongPassword
    ));

    // 令牌一次性使用后即物理删除。
    assert!(!common::user_token_exists(&db, id, UserTokenType::PasswordReset).await);

    // 重置密码 = 可能被盗后的补救：既有会话全部踢下线。
    assert_eq!(common::live_refresh_tokens(&db, id).await, 0);
}

#[sqlx::test]
async fn 验证邮箱令牌不可用于重置密码(db: PgPool) {
    let hasher = test_hasher();
    let (_user, raw) = user_repo::register(&db, &hasher, "cross", "cross@e.com", "pw", None)
        .await
        .unwrap();

    // email_verify 令牌撞 password_reset 通道：按类型隔离，视为无效。
    let err = user_repo::reset_password(&db, &hasher, &raw, "new-pw")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidToken), "实际: {err:?}");
}

#[sqlx::test]
async fn 并发双用同一重置令牌恰好一次成功(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "race@e.com", "old-pw").await;
    let raw = user_repo::request_password_reset(&db, "race@e.com")
        .await
        .unwrap();

    // 确定性撑开竞态窗口：外部事务先锁住 users 行。两个重置都会阻塞在
    // 「消费令牌之后、UPDATE users 之前」（修复后）或「查找之后、UPDATE
    // users 之前」（修复前），从而保证双方的令牌查找都在任何提交之前完成。
    // 不加这把锁时，单线程测试运行时 + 冷连接建立会把两个调用天然串行化
    // （第二个查找拖到第一个提交之后），竞态窗口打不开——已实测确认。
    let mut locker = db.acquire().await.unwrap();
    let mut lock_tx = sqlx::Connection::begin(&mut *locker).await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(id)
        .execute(&mut *lock_tx)
        .await
        .unwrap();

    // 同一令牌、不同新密码的两个重置并发到达：一次性语义
    // 要求恰好一个消费成功。
    let (r1, r2, ()) = tokio::join!(
        user_repo::reset_password(&db, &hasher, &raw, "pw-a"),
        user_repo::reset_password(&db, &hasher, &raw, "pw-b"),
        async {
            // 等两个重置都阻塞在锁上再放行；超时说明有请求提前结束，断言兜底。
            let mut both_blocked = false;
            for _ in 0..500 {
                let blocked: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_stat_activity \
                     WHERE datname = current_database() AND wait_event_type = 'Lock'",
                )
                .fetch_one(&db)
                .await
                .unwrap();
                if blocked >= 2 {
                    both_blocked = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            lock_tx.commit().await.unwrap();
            assert!(both_blocked, "两个并发重置应同时阻塞在行锁上");
        }
    );

    let ok_count = usize::from(r1.is_ok()) + usize::from(r2.is_ok());
    assert_eq!(ok_count, 1, "r1={r1:?}, r2={r2:?}");

    // 败者得 InvalidToken（令牌已被胜者消费），而非 ExpiredToken。
    let loser = if r1.is_ok() { &r2 } else { &r1 };
    assert!(matches!(loser, Err(Error::InvalidToken)), "实际: {loser:?}");

    // 最终密码必须是胜者的那个——「双 Ok、后提交者获胜」的旧行为被排除。
    let winner_pw = if r1.is_ok() { "pw-a" } else { "pw-b" };
    let fetched = user_repo::get_user(&db, id).await.unwrap();
    hasher.verify(winner_pw, &fetched.password_hash).unwrap();
}

#[sqlx::test]
async fn 重置已注销用户的密码令牌不被消费(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "gone@e.com", "pw").await;
    let raw = user_repo::request_password_reset(&db, "gone@e.com")
        .await
        .unwrap();
    user_repo::deactivate(&db, id).await.unwrap();

    let err = user_repo::reset_password(&db, &hasher, &raw, "new-pw")
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );
    // 回滚使令牌未被消费（契约见 reset_password doc comment）。
    assert!(common::user_token_exists(&db, id, UserTokenType::PasswordReset).await);
}

#[sqlx::test]
async fn 令牌有效期按文档定值写入(db: PgPool) {
    let hasher = test_hasher();
    let (user, raw) = user_repo::register(&db, &hasher, "ttl", "ttl@e.com", "pw", None)
        .await
        .unwrap();

    // email_verify 有效期 24 小时。
    let expires_at: DateTime<Utc> = sqlx::query_scalar(
        "SELECT expires_at FROM user_tokens WHERE user_id = $1 AND type = 'email_verify'",
    )
    .bind(user.id)
    .fetch_one(&db)
    .await
    .unwrap();
    let now = Utc::now();
    assert!(
        expires_at > now + chrono::Duration::hours(23)
            && expires_at < now + chrono::Duration::hours(25),
        "email_verify 有效期偏离 24h：{expires_at}（now: {now}）"
    );

    // password_reset 有效期 1 小时；签发需 active。
    user_repo::verify_email(&db, &raw).await.unwrap();
    user_repo::request_password_reset(&db, "ttl@e.com")
        .await
        .unwrap();
    let expires_at: DateTime<Utc> = sqlx::query_scalar(
        "SELECT expires_at FROM user_tokens WHERE user_id = $1 AND type = 'password_reset'",
    )
    .bind(user.id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(
        expires_at > now + chrono::Duration::minutes(59)
            && expires_at < now + chrono::Duration::minutes(61),
        "password_reset 有效期偏离 1h：{expires_at}（now: {now}）"
    );
}

#[sqlx::test]
async fn 改密码需验旧密码(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "change@e.com", "old-pw").await;

    // 错误旧密码：独立可识别的错误，不与令牌错误混淆。
    let err = user_repo::change_password(&db, &hasher, id, "wrong", "new-pw")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::WrongPassword), "实际: {err:?}");

    // 正确旧密码：更新成功。
    user_repo::change_password(&db, &hasher, id, "old-pw", "new-pw")
        .await
        .unwrap();
    let fetched = user_repo::get_user(&db, id).await.unwrap();
    hasher.verify("new-pw", &fetched.password_hash).unwrap();
}

#[sqlx::test]
async fn 改密码不revoke刷新令牌(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "keep@e.com", "old-pw").await;
    common::refresh_token(&db, id).await;
    common::refresh_token(&db, id).await;

    user_repo::change_password(&db, &hasher, id, "old-pw", "new-pw")
        .await
        .unwrap();

    // 刻意不 revoke：自助改密码是登录态下的自主操作，
    // 既有会话继续有效；只有封禁与管理员强重置才踢下线。
    assert_eq!(common::live_refresh_tokens(&db, id).await, 2);
}

// ============================================================
// 资料编辑（用户可自编辑 nickname/avatar/bio）
// ============================================================

#[sqlx::test]
async fn 更新资料三字段可设可清(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "profile@e.com", "pw").await;

    // 设置三个字段。
    let updated = user_repo::update_profile(
        &db,
        id,
        Some("新昵称"),
        Some("https://cdn.example.com/a.png"),
        Some("个人简介"),
    )
    .await
    .unwrap();
    assert_eq!(updated.nickname.as_deref(), Some("新昵称"));
    assert_eq!(updated.avatar.as_deref(), Some("https://cdn.example.com/a.png"));
    assert_eq!(updated.bio.as_deref(), Some("个人简介"));
    // role / status 不在此函数的签名里——类型系统杜绝越权修改。
    assert_eq!(updated.role, UserRole::User);
    assert_eq!(updated.status, UserStatus::Active);

    // None = 清空字段（「按传入设置」语义，非增量 PATCH）。
    let cleared = user_repo::update_profile(&db, id, None, None, None)
        .await
        .unwrap();
    assert_eq!(cleared.nickname, None);
    assert_eq!(cleared.avatar, None);
    assert_eq!(cleared.bio, None);
}

#[sqlx::test]
async fn 更新资料对不存在用户返回notfound(db: PgPool) {
    let err = user_repo::update_profile(&db, Uuid::now_v7(), Some("x"), None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );
}

// ============================================================
// 查询与注销（注册流程的收尾动作）
// ============================================================

#[sqlx::test]
async fn 查询不存在或已注销用户返回notfound(db: PgPool) {
    let err = user_repo::get_user(&db, Uuid::now_v7()).await.unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );

    let hasher = test_hasher();
    let (user, _) = user_repo::register(&db, &hasher, "gone", "gone@e.com", "pw", None)
        .await
        .unwrap();
    user_repo::deactivate(&db, user.id).await.unwrap();

    let err = user_repo::get_user(&db, user.id).await.unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 注销自己成功软删(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "bye@e.com", "pw").await;

    user_repo::deactivate(&db, id).await.unwrap();

    // 软删：API 视角消失，行仍在（保留归属历史）。
    let err = user_repo::get_user(&db, id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }));
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(deleted);
}

#[sqlx::test]
async fn 名下有活跃空间时注销被拒(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "owner@e.com", "pw").await;
    common::space(&db, "sp", id, Visibility::Private).await;

    // spaces.owner_id 是 RESTRICT：不先转移所有权就注销，
    // 会让后续确删永远卡住。
    let err = user_repo::deactivate(&db, id).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");

    // 拒绝后用户仍然存活。
    assert!(user_repo::get_user(&db, id).await.is_ok());
}

#[sqlx::test]
async fn 最后一个active管理员自助注销被拒(db: PgPool) {
    let hasher = test_hasher();
    let sole = common::verified_admin(&db, &hasher, "sole@e.com", "pw").await;

    // 规则 1 无条件适用：文档禁止最后一个 active admin 退出管理员集合，
    // 自助通道不豁免：最后一个 active admin 不可注销。
    let err = user_repo::deactivate(&db, sole).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");

    let fetched = user_repo::get_user(&db, sole).await.unwrap();
    assert_eq!(fetched.role, UserRole::Admin);
    assert_eq!(fetched.status, UserStatus::Active);
}

#[sqlx::test]
async fn 还有别的admin时自助注销成功(db: PgPool) {
    let hasher = test_hasher();
    let admin = common::verified_admin(&db, &hasher, "a@e.com", "pw").await;
    common::verified_admin(&db, &hasher, "b@e.com", "pw").await;

    // 管理员集合仍有其他 active admin，自助注销放行。
    user_repo::deactivate(&db, admin).await.unwrap();
    assert!(matches!(
        user_repo::get_user(&db, admin).await.unwrap_err(),
        Error::NotFound { .. }
    ));
}

// ============================================================
// 管理员列表
// ============================================================

/// raw SQL 造一个可控 created_at / role / status / 软删状态的用户。
/// 列表测试需要精确控制这些维度，走 repository 反而绕远。
async fn seed_user(
    db: &PgPool,
    email: &str,
    nickname: Option<&str>,
    role: &str,
    status: &str,
    created_hours_ago: i64,
    deleted: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    // username 取 email 本地部分（不足 3 字符补到 3），满足 chk_users_username_len。
    let local = email.split('@').next().unwrap_or("user");
    let username = if local.chars().count() < 3 {
        format!("{local}user")
    } else {
        local.to_owned()
    };
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, nickname, role, status, created_at, deleted_at, username) \
         VALUES ($1, $2, 'x', $3, $4, $5, now() - ($6 || ' hours')::interval, \
                 CASE WHEN $7 THEN now() ELSE NULL END, $8)",
    )
    .bind(id)
    .bind(email)
    .bind(nickname)
    .bind(role)
    .bind(status)
    .bind(created_hours_ago.to_string())
    .bind(deleted)
    .bind(username)
    .execute(db)
    .await
    .expect("造列表用户");
    id
}

#[sqlx::test]
async fn 管理员列表默认按创建时间倒序(db: PgPool) {
    let oldest = seed_user(&db, "old@e.com", None, "user", "active", 30, false).await;
    let mid = seed_user(&db, "mid@e.com", None, "user", "active", 20, false).await;
    let newest = seed_user(&db, "new@e.com", None, "user", "active", 10, false).await;

    let (rows, total) = user_repo::list_users(&db, &UserFilter::default(), 1, 10)
        .await
        .unwrap();

    assert_eq!(total, 3);
    let ids: Vec<Uuid> = rows.iter().map(|u| u.id).collect();
    // created_at DESC：最新在前。
    assert_eq!(ids, vec![newest, mid, oldest]);
}

#[sqlx::test]
async fn 管理员列表软删用户不出现(db: PgPool) {
    let alive = seed_user(&db, "alive@e.com", None, "user", "active", 20, false).await;
    let _gone = seed_user(&db, "gone@e.com", None, "user", "active", 10, true).await;

    let (rows, total) = user_repo::list_users(&db, &UserFilter::default(), 1, 10)
        .await
        .unwrap();

    // 软删用户被 deleted_at IS NULL 过滤掉，total 也不计它。
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, alive);
}

#[sqlx::test]
async fn 管理员列表按角色过滤(db: PgPool) {
    let admin = seed_user(&db, "a@e.com", None, "admin", "active", 20, false).await;
    let _user = seed_user(&db, "u@e.com", None, "user", "active", 10, false).await;

    let filter = UserFilter { role: Some(UserRole::Admin), ..UserFilter::default() };
    let (rows, total) = user_repo::list_users(&db, &filter, 1, 10).await.unwrap();

    assert_eq!(total, 1);
    assert_eq!(rows[0].id, admin);
}

#[sqlx::test]
async fn 管理员列表按状态过滤(db: PgPool) {
    let _active = seed_user(&db, "a@e.com", None, "user", "active", 20, false).await;
    let banned = seed_user(&db, "b@e.com", None, "user", "banned", 10, false).await;

    let filter = UserFilter { status: Some(UserStatus::Banned), ..UserFilter::default() };
    let (rows, total) = user_repo::list_users(&db, &filter, 1, 10).await.unwrap();

    assert_eq!(total, 1);
    assert_eq!(rows[0].id, banned);
}

#[sqlx::test]
async fn 管理员列表搜索词命中邮箱与昵称(db: PgPool) {
    let by_email = seed_user(&db, "alice@e.com", None, "user", "active", 30, false).await;
    let by_nickname = seed_user(&db, "bob@e.com", Some("爱丽丝"), "user", "active", 20, false).await;
    let _other = seed_user(&db, "carol@e.com", None, "user", "active", 10, false).await;

    // 搜「alice」命中邮箱。
    let filter = UserFilter { search: Some("alice".into()), ..UserFilter::default() };
    let (rows, total) = user_repo::list_users(&db, &filter, 1, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, by_email);

    // 搜「爱丽丝」命中昵称（ILIKE，中文同样适用）。
    let filter = UserFilter { search: Some("爱丽丝".into()), ..UserFilter::default() };
    let (rows, total) = user_repo::list_users(&db, &filter, 1, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].id, by_nickname);
}

#[sqlx::test]
async fn 管理员列表多过滤条件组合生效(db: PgPool) {
    // 唯一命中：admin + banned + 昵称含搜索词。
    let hit = seed_user(&db, "hit@e.com", Some("组合词"), "admin", "banned", 50, false).await;
    // 干扰项：各满足部分条件。
    seed_user(&db, "d1@e.com", Some("组合词"), "admin", "active", 40, false).await;
    seed_user(&db, "d2@e.com", Some("组合词"), "user", "banned", 30, false).await;
    seed_user(&db, "d3@e.com", None, "admin", "banned", 20, false).await;
    // 软删者：满足全部过滤条件但不应出现。
    seed_user(&db, "d4@e.com", Some("组合词"), "admin", "banned", 10, true).await;

    let filter = UserFilter {
        role: Some(UserRole::Admin),
        status: Some(UserStatus::Banned),
        search: Some("组合词".into()),
    };
    let (rows, total) = user_repo::list_users(&db, &filter, 1, 10).await.unwrap();

    // 三过滤 + 软删排除必须组合出恰好一条——锁死动态 SQL 的
    // 子句拼接与 bind 顺序同步（错位会静默返回错误行）。
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, hit);
}

#[sqlx::test]
async fn 管理员列表分页且total为全量(db: PgPool) {
    for i in 0..5 {
        seed_user(&db, &format!("u{i}@e.com"), None, "user", "active", i, false).await;
    }

    // 第 1 页，每页 2 条。
    let (rows, total) = user_repo::list_users(&db, &UserFilter::default(), 1, 2)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    // total 是过滤后的全量，不随分页缩小。
    assert_eq!(total, 5);

    // 第 3 页，每页 2 条 → 只剩 1 条。
    let (rows, total) = user_repo::list_users(&db, &UserFilter::default(), 3, 2)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(total, 5);

    // 越界页返回空，但 total 不变。
    let (rows, total) = user_repo::list_users(&db, &UserFilter::default(), 99, 2)
        .await
        .unwrap();
    assert_eq!(rows.len(), 0);
    assert_eq!(total, 5);
}

#[sqlx::test]
async fn 分页参数极端值不panic返回空页(db: PgPool) {
    seed_user(&db, "u@e.com", None, "user", "active", 10, false).await;

    // 上限不做业务限制：(page-1)*size 在 u32::MAX 下会溢出 i64，
    // 饱和乘法必须兜住——超大 offset 得到空页而非 panic。
    let (rows, total) = user_repo::list_users(&db, &UserFilter::default(), u32::MAX, u32::MAX)
        .await
        .unwrap();
    assert_eq!(rows.len(), 0);
    assert_eq!(total, 1);
}

// ============================================================
// 管理员操作与自我保护
// ============================================================

#[sqlx::test]
async fn 改角色升迁与降级成功(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let target = common::verified_user(&db, &hasher, "target@e.com", "pw").await;

    // 升迁。
    user_repo::set_role(&db, actor, target, UserRole::Admin).await.unwrap();
    assert_eq!(user_repo::get_user(&db, target).await.unwrap().role, UserRole::Admin);

    // 降级（此时有两个 admin，target 不是最后一个）。
    user_repo::set_role(&db, actor, target, UserRole::User).await.unwrap();
    assert_eq!(user_repo::get_user(&db, target).await.unwrap().role, UserRole::User);
}

#[sqlx::test]
async fn 降级最后一个active管理员被拒(db: PgPool) {
    let hasher = test_hasher();
    // 唯一的 admin。
    let sole = common::verified_admin(&db, &hasher, "sole@e.com", "pw").await;

    // 即便是对自己降级也不行——规则 1 不豁免自我操作。
    let err = user_repo::set_role(&db, sole, sole, UserRole::User)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert_eq!(user_repo::get_user(&db, sole).await.unwrap().role, UserRole::Admin);
}

#[sqlx::test]
async fn 还有别的admin时可降级(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let other = common::verified_admin(&db, &hasher, "other@e.com", "pw").await;

    // actor 降级 other：还有 actor 这个 admin，允许。
    user_repo::set_role(&db, actor, other, UserRole::User).await.unwrap();
    assert_eq!(user_repo::get_user(&db, other).await.unwrap().role, UserRole::User);
}

#[sqlx::test]
async fn 封禁成功且刷新令牌被revoke(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let target = common::verified_user(&db, &hasher, "target@e.com", "pw").await;
    common::refresh_token(&db, target).await;
    common::refresh_token(&db, target).await;

    user_repo::ban_user(&db, actor, target).await.unwrap();

    assert_eq!(user_repo::get_user(&db, target).await.unwrap().status, UserStatus::Banned);
    // 封禁即踢下线。
    assert_eq!(common::live_refresh_tokens(&db, target).await, 0);
}

#[sqlx::test]
async fn 封禁自己被拒(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    // 另有 admin 在场：此刻撞的纯粹是规则 2，与「最后一个 admin」无关。
    common::verified_admin(&db, &hasher, "other@e.com", "pw").await;

    let err = user_repo::ban_user(&db, actor, actor).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert_eq!(user_repo::get_user(&db, actor).await.unwrap().status, UserStatus::Active);
}

#[sqlx::test]
async fn 封禁最后一个active管理员被拒(db: PgPool) {
    let hasher = test_hasher();
    // 唯一的 admin。
    let sole = common::verified_admin(&db, &hasher, "sole@e.com", "pw").await;

    // 「最后一个 active admin 不可被封禁」的业务结果必须成立。
    // 能到达这里的唯一路径是自我封禁——操作者必须是活跃管理员且
    // 不可是目标本人以外的「最后一个」，所以规则 2 先于规则 1 生效；
    // 规则 1 在 ban 通道是纵深防御（降级/注销通道有可直接命中的用例）。
    let err = user_repo::ban_user(&db, sole, sole).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert_eq!(user_repo::get_user(&db, sole).await.unwrap().status, UserStatus::Active);
}

#[sqlx::test]
async fn 解封banned用户成功(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let target = common::verified_user(&db, &hasher, "target@e.com", "pw").await;
    user_repo::ban_user(&db, actor, target).await.unwrap();

    user_repo::unban_user(&db, actor, target).await.unwrap();
    assert_eq!(user_repo::get_user(&db, target).await.unwrap().status, UserStatus::Active);

    // 已 active 的用户无需解封：仅 banned 是合法源状态。
    let err = user_repo::unban_user(&db, actor, target).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn 解封pending用户被拒(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    // pending 用户：未验证邮箱。
    let (pending, _) = user_repo::register(&db, &hasher, "pending", "pending@e.com", "pw", None)
        .await
        .unwrap();

    // pending → active 会绕过邮箱验证，被拒。
    let err = user_repo::unban_user(&db, actor, pending.id).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert_eq!(user_repo::get_user(&db, pending.id).await.unwrap().status, UserStatus::Pending);
}

#[sqlx::test]
async fn 强重置密码成功且revoke(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let target = common::verified_user(&db, &hasher, "target@e.com", "old-pw").await;
    common::refresh_token(&db, target).await;

    user_repo::admin_reset_password(&db, &hasher, actor, target, "brand-new")
        .await
        .unwrap();

    let fetched = user_repo::get_user(&db, target).await.unwrap();
    hasher.verify("brand-new", &fetched.password_hash).unwrap();
    assert_eq!(common::live_refresh_tokens(&db, target).await, 0);
}

#[sqlx::test]
async fn 管理员操作对不存在或已注销目标返回notfound(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;

    // 不存在的目标：admin_reset_password 的唯一防线是 rows_affected 检查。
    let err = user_repo::admin_reset_password(&db, &hasher, actor, Uuid::now_v7(), "x")
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );

    // 已注销的目标。
    let target = common::verified_user(&db, &hasher, "target@e.com", "pw").await;
    user_repo::deactivate(&db, target).await.unwrap();
    let err = user_repo::admin_reset_password(&db, &hasher, actor, target, "x")
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotFound { resource: Resource::User, .. }),
        "实际: {err:?}"
    );
}

#[sqlx::test]
async fn 注销自己名下无空间成功(db: PgPool) {
    let hasher = test_hasher();
    let id = common::verified_user(&db, &hasher, "self@e.com", "pw").await;
    user_repo::deactivate(&db, id).await.unwrap();
    assert!(matches!(
        user_repo::get_user(&db, id).await.unwrap_err(),
        Error::NotFound { .. }
    ));
}

#[sqlx::test]
async fn 管理员强注销成功(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let target = common::verified_user(&db, &hasher, "target@e.com", "pw").await;

    user_repo::admin_deactivate(&db, actor, target).await.unwrap();
    assert!(matches!(
        user_repo::get_user(&db, target).await.unwrap_err(),
        Error::NotFound { .. }
    ));
}

#[sqlx::test]
async fn 管理员强注销自己被拒(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    // 另有 admin 在场：此刻撞的纯粹是规则 2，与「最后一个 admin」无关。
    common::verified_admin(&db, &hasher, "other@e.com", "pw").await;

    let err = user_repo::admin_deactivate(&db, actor, actor).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert!(user_repo::get_user(&db, actor).await.is_ok());
}

#[sqlx::test]
async fn 强注销最后一个active管理员被拒(db: PgPool) {
    let hasher = test_hasher();
    // 唯一的 admin。
    let sole = common::verified_admin(&db, &hasher, "sole@e.com", "pw").await;

    // 「最后一个 active admin 不可被注销」的业务结果必须成立。
    // 操作者必须是活跃管理员且不可注销自己，故到达此结果的唯一路径
    // 先撞上规则 2；规则 1 在强注销通道是纵深防御（降级通道有可直接命中的用例）。
    let err = user_repo::admin_deactivate(&db, sole, sole).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert!(user_repo::get_user(&db, sole).await.is_ok());
}

#[sqlx::test]
async fn 强注销名下有活跃空间的用户被拒(db: PgPool) {
    let hasher = test_hasher();
    let actor = common::verified_admin(&db, &hasher, "actor@e.com", "pw").await;
    let target = common::verified_user(&db, &hasher, "target@e.com", "pw").await;
    common::space(&db, "sp", target, Visibility::Private).await;

    // 管理员不豁免空间所有权校验。
    let err = user_repo::admin_deactivate(&db, actor, target).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    assert!(user_repo::get_user(&db, target).await.is_ok());
}

#[sqlx::test]
async fn 非admin执行管理操作被拒(db: PgPool) {
    let hasher = test_hasher();
    let plain = common::verified_user(&db, &hasher, "plain@e.com", "pw").await;
    let victim = common::verified_user(&db, &hasher, "victim@e.com", "pw").await;

    // 抽样覆盖四个管理操作：全部因「非 admin」被拒。
    let err = user_repo::set_role(&db, plain, victim, UserRole::Admin).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "set_role 实际: {err:?}");

    let err = user_repo::ban_user(&db, plain, victim).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "ban_user 实际: {err:?}");

    let err = user_repo::admin_reset_password(&db, &hasher, plain, victim, "x")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "admin_reset_password 实际: {err:?}");

    let err = user_repo::admin_deactivate(&db, plain, victim).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "admin_deactivate 实际: {err:?}");

    let err = user_repo::unban_user(&db, plain, victim).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "unban_user 实际: {err:?}");

    // 受害者毫发无损。
    let fetched = user_repo::get_user(&db, victim).await.unwrap();
    assert_eq!(fetched.role, UserRole::User);
    assert_eq!(fetched.status, UserStatus::Active);
}

#[sqlx::test]
async fn 被封禁的管理员不可执行管理操作(db: PgPool) {
    let hasher = test_hasher();
    let banned_admin = common::verified_admin(&db, &hasher, "banned@e.com", "pw").await;
    let other = common::verified_admin(&db, &hasher, "other@e.com", "pw").await;

    user_repo::ban_user(&db, other, banned_admin).await.unwrap();

    // banned 管理员被拒绝登录，操作者校验要求 active，
    // 故其不能执行任何管理操作。
    let err = user_repo::set_role(&db, banned_admin, other, UserRole::User)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");

    // 目标不受影响。
    assert_eq!(user_repo::get_user(&db, other).await.unwrap().role, UserRole::Admin);
}

// ============================================================
// 文档定值锁定（无需数据库）
// ============================================================

/// pending 用户清理保留期是 7 天，锁死防漂移。
#[test]
fn pending用户清理保留期为七天() {
    assert_eq!(lifecycle::PENDING_USER_RETENTION, Duration::from_secs(7 * 24 * 3600));
}

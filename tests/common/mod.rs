//! 集成测试的共享夹具。
//!
//! 所有测试跑在真实 PostgreSQL 上（`#[sqlx::test]` 为每个测试建一个
//! 临时库并跑完全部迁移）。刻意不 mock 数据库：本项目要验证的正是
//! DEFERRABLE 唯一约束、部分唯一索引、生成列这些数据库行为，
//! mock 掉之后测试就一文不值了。

// 每个测试二进制都包含整个模块，但各自只用其中一部分夹具。
#![allow(dead_code)]

use knowledgebase::{
    db::Db,
    models::{ChunkType, Permission, SpaceRole, UserTokenType, Visibility},
    repository::user::{self as user_repo, PasswordHasher},
};
use serde_json::json;
use uuid::Uuid;

/// 建一个用户，返回其 id。`email` 需在测试内唯一。
pub async fn user(db: &Db, email: &str) -> Uuid {
    user_with_role(db, email, "user").await
}

/// 建一个系统管理员。
pub async fn admin(db: &Db, email: &str) -> Uuid {
    user_with_role(db, email, "admin").await
}

async fn user_with_role(db: &Db, email: &str, role: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, username) VALUES ($1, $2, 'x-argon2-hash', $3, $4)",
    )
    .bind(id)
    .bind(email)
    .bind(role)
    // username 取 email 本地部分，满足 3–32 且各测试唯一。
    .bind(username_from(email))
    .execute(db)
    .await
    .expect("建用户");
    id
}

/// 由 email 本地部分派生一个合法的测试 username（3–32 字符）。
fn username_from(email: &str) -> String {
    let local: String = email
        .split('@')
        .next()
        .unwrap_or("user")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let local = if local.len() < 3 {
        format!("{local}user")
    } else {
        local
    };
    local.chars().take(32).collect()
}

/// 建一个空间，并把 owner 写入 space_members。
pub async fn space(db: &Db, slug: &str, owner_id: Uuid, visibility: Visibility) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO spaces (id, name, slug, owner_id, visibility) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(slug)
    .bind(slug)
    .bind(owner_id)
    .bind(visibility)
    .execute(db)
    .await
    .expect("建空间");

    add_member(db, id, owner_id, SpaceRole::Owner).await;
    id
}

pub async fn add_member(db: &Db, space_id: Uuid, user_id: Uuid, role: SpaceRole) {
    sqlx::query("INSERT INTO space_members (space_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(space_id)
        .bind(user_id)
        .bind(role)
        .execute(db)
        .await
        .expect("加成员");
}

/// 建一个文档。`slug` 传 None 表示不占用 slug 唯一槽位。
pub async fn document(db: &Db, space_id: Uuid, title: &str, slug: Option<&str>) -> Uuid {
    document_with_parent(db, space_id, title, slug, None).await
}

pub async fn document_with_parent(
    db: &Db,
    space_id: Uuid,
    title: &str,
    slug: Option<&str>,
    parent_id: Option<Uuid>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO documents (id, space_id, title, slug, parent_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(space_id)
    .bind(title)
    .bind(slug)
    .bind(parent_id)
    .execute(db)
    .await
    .expect("建文档");
    id
}

/// 给文档追加一个段落分片，返回 chunk id。
pub async fn paragraph(db: &Db, document_id: Uuid, text: &str, position: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(document_id)
    .bind(ChunkType::Paragraph)
    .bind(json!({ "text": text }))
    .bind(position)
    .execute(db)
    .await
    .expect("建分片");
    id
}

/// 授予文档级权限。`user_id` 传 `PUBLIC_USER_ID` 即公开权限。
pub async fn grant(
    db: &Db,
    document_id: Uuid,
    user_id: Uuid,
    permission: Permission,
    granted_by: Option<Uuid>,
) {
    sqlx::query(
        "INSERT INTO document_permissions (document_id, user_id, permission, granted_by) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(document_id)
    .bind(user_id)
    .bind(permission)
    .bind(granted_by)
    .execute(db)
    .await
    .expect("授权");
}

/// 取某文档全部分片的 position，升序。用于断言连续性。
pub async fn positions(db: &Db, document_id: Uuid) -> Vec<i32> {
    sqlx::query_scalar(
        "SELECT position FROM document_chunks WHERE document_id = $1 ORDER BY position",
    )
    .bind(document_id)
    .fetch_all(db)
    .await
    .expect("查 position")
}

/// 把某记录的 `deleted_at` 强行改到指定天数之前，用于测试保留期到期。
pub async fn backdate_deletion(db: &Db, table: &str, id: Uuid, days_ago: i64) {
    // table 来自测试代码的字面量，非外部输入。
    let sql =
        format!("UPDATE {table} SET deleted_at = now() - ($2 || ' days')::interval WHERE id = $1");
    sqlx::query(&sql)
        .bind(id)
        .bind(days_ago.to_string())
        .execute(db)
        .await
        .expect("回溯删除时间");
}

// ============================================================
// 用户管理（v1.2）夹具
// ============================================================

/// 注册 + 验证邮箱，返回已激活用户的 id。
///
/// 走 `repository::user` 的 register + verify_email 完整流程，
/// 不绕过被测 API。
pub async fn verified_user(db: &Db, hasher: &PasswordHasher, email: &str, password: &str) -> Uuid {
    let (user, raw) = user_repo::register(db, hasher, &username_from(email), email, password, None)
        .await
        .expect("注册");
    user_repo::verify_email(db, &raw).await.expect("验证邮箱");
    user.id
}

/// 注册 + 验证 + 提权的管理员。
///
/// 系统里没有「注册为管理员」的通道（role 仅管理员可改），
/// 提权只能 raw SQL——夹点设置允许的例外。
pub async fn verified_admin(db: &Db, hasher: &PasswordHasher, email: &str, password: &str) -> Uuid {
    let id = verified_user(db, hasher, email, password).await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .expect("提权为管理员");
    id
}

/// 插入一条未撤销的 refresh token，返回其 id。
///
/// token_hash 需全局唯一，用 UUID 占位（生产中是 SHA-256 hex）。
pub async fn refresh_token(db: &Db, user_id: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at) \
         VALUES ($1, $2, $3, now() + interval '7 days')",
    )
    .bind(id)
    .bind(user_id)
    .bind(format!("rt-{}", Uuid::now_v7()))
    .execute(db)
    .await
    .expect("建 refresh token");
    id
}

/// 某用户未撤销的 refresh token 数。
pub async fn live_refresh_tokens(db: &Db, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
    .expect("查 refresh token 数")
}

/// 某用户某类型的一次性令牌是否仍存在。
pub async fn user_token_exists(db: &Db, user_id: Uuid, token_type: UserTokenType) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_tokens WHERE user_id = $1 AND type = $2)",
    )
    .bind(user_id)
    .bind(token_type)
    .fetch_one(db)
    .await
    .expect("查令牌存在性")
}

/// 某用户某类型令牌的存储哈希，用于断言「不存明文」。
pub async fn user_token_hash(db: &Db, user_id: Uuid, token_type: UserTokenType) -> Option<String> {
    sqlx::query_scalar("SELECT token_hash FROM user_tokens WHERE user_id = $1 AND type = $2")
        .bind(user_id)
        .bind(token_type)
        .fetch_optional(db)
        .await
        .expect("查令牌哈希")
}

/// 把某用户的 `created_at` 强行改到指定天数之前，用于测试 pending 保留期到期。
pub async fn backdate_user_created(db: &Db, user_id: Uuid, days_ago: i64) {
    sqlx::query("UPDATE users SET created_at = now() - ($2 || ' days')::interval WHERE id = $1")
        .bind(user_id)
        .bind(days_ago.to_string())
        .execute(db)
        .await
        .expect("回溯创建时间");
}

/// 把某用户某类型令牌的 `expires_at` 回溯到过去，制造「已过期」状态。
pub async fn backdate_token_expiry(db: &Db, user_id: Uuid, token_type: UserTokenType) {
    sqlx::query(
        "UPDATE user_tokens SET expires_at = now() - interval '1 hour' \
         WHERE user_id = $1 AND type = $2",
    )
    .bind(user_id)
    .bind(token_type)
    .execute(db)
    .await
    .expect("回溯令牌过期时间");
}

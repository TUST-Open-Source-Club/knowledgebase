//! 连接池构建与事务帮手。

use sqlx::{
    Postgres, Transaction,
    postgres::{PgPool, PgPoolOptions},
};

use crate::{config::Config, error::Result};

/// 全局共享的连接池类型别名。
pub type Db = PgPool;

/// 事务句柄。repository 层的多语句操作统一接收 `&mut Tx`，
/// 由调用方决定事务边界（级联软删、chunk reorder 都要求同一事务）。
pub type Tx<'a> = Transaction<'a, Postgres>;

/// 内嵌 `migrations/` 目录。二进制自带迁移，部署时不需要额外分发 SQL 文件。
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(config: &Config) -> Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(config.db_acquire_timeout)
        .connect(&config.database_url)
        .await?;

    Ok(pool)
}

/// 跑到最新版本。启动时调用，保证 schema 与二进制同步。
pub async fn migrate(db: &Db) -> Result<()> {
    MIGRATOR.run(db).await?;
    Ok(())
}

/// 公开权限哨兵：`document_permissions.user_id` 取此值表示「所有用户」。
///
/// 该 UUID 不存在于 `users` 表中，由 `chk_users_id_not_public_sentinel` 约束保证。
/// 仅允许出现在 `document_permissions.user_id` 一列。
pub const PUBLIC_USER_ID: uuid::Uuid = uuid::Uuid::nil();

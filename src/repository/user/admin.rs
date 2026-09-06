//! 管理员操作与自我保护。

use uuid::Uuid;

use crate::{
    db::{Db, Tx},
    error::{Error, Resource, Result},
    models::{User, UserRole, UserStatus},
    repository::lifecycle,
};

use super::{
    auth::get_user_tx,
    hasher::PasswordHasher,
    password::revoke_user_refresh_tokens,
};

/// 管理员列表的过滤条件。全部可选，`None` 即不过滤。
#[derive(Debug, Clone, Default)]
pub struct UserFilter {
    pub role: Option<UserRole>,
    pub status: Option<UserStatus>,
    /// 单一搜索词，对 email 与 nickname 做 ILIKE 匹配。
    /// 注意：词中的 `%` / `_` 会当通配符生效（ILIKE 语义），一期接受不转义。
    pub search: Option<String>,
}

/// 管理员用户列表：分页 + 全量总数，可选 role/status 过滤与搜索词。
///
/// 只出 `deleted_at IS NULL` 的用户；默认 `created_at DESC`。
pub async fn list_users(
    db: &Db,
    filter: &UserFilter,
    page: u32,
    size: u32,
) -> Result<(Vec<User>, i64)> {
    let page = page.max(1);
    let size = size.max(1);

    // 动态拼 WHERE：每个启用的过滤条件占一个占位符。
    let mut clauses = vec!["deleted_at IS NULL".to_string()];
    let mut param_idx = 0;
    if filter.role.is_some() {
        param_idx += 1;
        clauses.push(format!("role = ${param_idx}"));
    }
    if filter.status.is_some() {
        param_idx += 1;
        clauses.push(format!("status = ${param_idx}"));
    }
    if filter.search.is_some() {
        param_idx += 1;
        clauses.push(format!(
            "(email ILIKE '%' || ${param_idx} || '%' \
             OR username ILIKE '%' || ${param_idx} || '%' \
             OR nickname ILIKE '%' || ${param_idx} || '%')"
        ));
    }
    let where_sql = clauses.join(" AND ");

    let limit_idx = param_idx + 1;
    let offset_idx = param_idx + 2;
    let sql = format!(
        "SELECT id, username, email, password_hash, nickname, avatar, bio, role, status, \
                created_at, updated_at, deleted_at \
         FROM users WHERE {where_sql} \
         ORDER BY created_at DESC LIMIT ${limit_idx} OFFSET ${offset_idx}"
    );
    // 总数与列表共享同一 WHERE，保证「同过滤条件」。
    let count_sql = format!("SELECT count(*) FROM users WHERE {where_sql}");

    // 饱和乘法：u32::MAX 级入参下 (page-1)*size 会溢出 i64，
    // 饱和到 i64::MAX 得到空页而非 panic / 环绕成负 offset。
    let offset = i64::from(page - 1).saturating_mul(i64::from(size));

    let mut query = sqlx::query_as::<sqlx::Postgres, User>(&sql);
    let mut count_query = sqlx::query_scalar::<sqlx::Postgres, i64>(&count_sql);
    // 绑定顺序必须与占位符顺序一致：role → status → search。
    if let Some(role) = filter.role {
        query = query.bind(role);
        count_query = count_query.bind(role);
    }
    if let Some(status) = filter.status {
        query = query.bind(status);
        count_query = count_query.bind(status);
    }
    if let Some(search) = &filter.search {
        query = query.bind(search.as_str());
        count_query = count_query.bind(search.as_str());
    }

    let users = query.bind(i64::from(size)).bind(offset).fetch_all(db).await?;
    let total = count_query.fetch_one(db).await?;

    Ok((users, total))
}

/// 校验操作者是活跃管理员：role=admin 且 status=active 且未软删。
///
/// banned / pending 的管理员被应用层拒绝登录，
/// 自然也不能执行管理操作，故要求 active 而非仅「未删除」。
///
/// 这是操作开始时的一次性鉴权快照：并发地降级 / 封禁操作者，不会
/// 追溯撤销已进入事务的操作。彻底闭合需对操作者行加锁，不值得——
/// 后果有界：操作完成时操作者身份可能已变，但操作本身是合法发起的。
async fn assert_actor_is_admin(db: &Db, actor_id: Uuid) -> Result<()> {
    let row = sqlx::query!(
        "SELECT role AS \"role: UserRole\", status AS \"status: UserStatus\" \
         FROM users WHERE id = $1 AND deleted_at IS NULL",
        actor_id
    )
    .fetch_optional(db)
    .await?;

    match row {
        None => Err(Error::NotFound {
            resource: Resource::User,
            id: actor_id,
        }),
        Some(row)
            if matches!(row.role, UserRole::Admin)
                && matches!(row.status, UserStatus::Active) =>
        {
            Ok(())
        }
        Some(_) => Err(Error::invariant("无权限：仅活跃管理员可执行此操作")),
    }
}

/// 规则 1：最后一个 active admin 不可被降级 / 封禁 / 注销。
///
/// 数「目标之外」还有几个活跃管理员：为 0 说明目标就是最后一个，拒绝。
/// 在事务内计数、紧贴后续写入——检查与落库之间无 TOCTOU 窗口。
/// 调用前提：目标当前是 active admin，且本操作会把 ta 移出活跃管理员集合。
pub(super) async fn assert_not_last_active_admin(tx: &mut Tx<'_>, target_id: Uuid) -> Result<()> {
    let others = sqlx::query_scalar!(
        "SELECT count(*) AS \"count!\" FROM users \
         WHERE role = 'admin' AND status = 'active' AND deleted_at IS NULL \
           AND id <> $1",
        target_id
    )
    .fetch_one(&mut **tx)
    .await?;

    if others == 0 {
        return Err(Error::invariant(
            "最后一个活跃管理员不可被降级、封禁或注销",
        ));
    }
    Ok(())
}

/// 修改用户角色（管理员操作）。升迁 / 降级均可。
///
/// 规则 1：若操作会把最后一个 active admin 移出管理员（目标是 active admin
/// 且新角色不是 admin），拒绝。
pub async fn set_role(db: &Db, actor_id: Uuid, target_id: Uuid, role: UserRole) -> Result<()> {
    assert_actor_is_admin(db, actor_id).await?;

    let mut tx = db.begin().await?;

    let target = get_user_tx(&mut tx, target_id).await?;

    if matches!(target.status, UserStatus::Active)
        && matches!(target.role, UserRole::Admin)
        && !matches!(role, UserRole::Admin)
    {
        assert_not_last_active_admin(&mut tx, target_id).await?;
    }

    sqlx::query!(
        "UPDATE users SET role = $2 WHERE id = $1 AND deleted_at IS NULL",
        target_id,
        role as _
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 封禁用户（管理员操作）：置为 banned 并同事务 revoke 全部 refresh token。
///
/// 规则 2：admin 不可封禁自己。规则 1：最后一个 active admin 不可被封禁
/// revoke 令牌即「踢下线」：否则被封禁者的既有会话
/// 会存活到令牌自然过期。
pub async fn ban_user(db: &Db, actor_id: Uuid, target_id: Uuid) -> Result<()> {
    assert_actor_is_admin(db, actor_id).await?;

    if actor_id == target_id {
        return Err(Error::invariant("管理员不可封禁自己"));
    }

    let mut tx = db.begin().await?;

    let target = get_user_tx(&mut tx, target_id).await?;

    if matches!(target.status, UserStatus::Banned) {
        return Err(Error::invariant("用户已被封禁"));
    }

    if matches!(target.status, UserStatus::Active) && matches!(target.role, UserRole::Admin) {
        assert_not_last_active_admin(&mut tx, target_id).await?;
    }

    sqlx::query!(
        "UPDATE users SET status = 'banned' WHERE id = $1 AND deleted_at IS NULL",
        target_id
    )
    .execute(&mut *tx)
    .await?;
    revoke_user_refresh_tokens(&mut tx, target_id).await?;

    tx.commit().await?;
    Ok(())
}

/// 解封用户（管理员操作）：banned → active。
///
/// 仅 banned 可解封：pending → active 会绕过邮箱验证，
/// 而 active 用户本就无需解封。两者都返回 `Invariant`。
pub async fn unban_user(db: &Db, actor_id: Uuid, target_id: Uuid) -> Result<()> {
    assert_actor_is_admin(db, actor_id).await?;

    let mut tx = db.begin().await?;

    let target = get_user_tx(&mut tx, target_id).await?;

    match target.status {
        UserStatus::Banned => {}
        UserStatus::Pending => {
            return Err(Error::invariant(
                "未验证邮箱的用户不可直接解封，请先完成邮箱验证",
            ));
        }
        UserStatus::Active => {
            return Err(Error::invariant("用户未被封禁"));
        }
    }

    sqlx::query!(
        "UPDATE users SET status = 'active' WHERE id = $1 AND deleted_at IS NULL",
        target_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 管理员强制重置密码：直接给定新密码，
/// 并同事务 revoke 该用户全部 refresh token。
///
/// revoke 与封禁同理——强重置往往意味着账号失控，既有会话一律作废。
/// 本操作不设「最后一个 admin」限制。
pub async fn admin_reset_password(
    db: &Db,
    hasher: &PasswordHasher,
    actor_id: Uuid,
    target_id: Uuid,
    new_password: &str,
) -> Result<()> {
    assert_actor_is_admin(db, actor_id).await?;

    let password_hash = hasher.hash(new_password)?;

    let mut tx = db.begin().await?;

    let updated = sqlx::query!(
        "UPDATE users SET password_hash = $2 WHERE id = $1 AND deleted_at IS NULL",
        target_id,
        password_hash
    )
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(Error::NotFound {
            resource: Resource::User,
            id: target_id,
        });
    }

    revoke_user_refresh_tokens(&mut tx, target_id).await?;

    tx.commit().await?;
    Ok(())
}

/// 管理员强制注销：软删除目标用户。
///
/// 与自助注销相同的两道前置：空间所有权校验（管理员不豁免，
/// 否则 `spaces.owner_id` RESTRICT 会卡住后续确删）与规则 1
/// （最后一个 active admin 不可被注销）。规则 2：不可强注销自己——
/// 自助注销有专门通道 [`crate::repository::user::deactivate`]。空间校验在事务外，目标在
/// 校验后新建空间的残余竞态最坏后果是确删清理跳过并告警，可接受。
pub async fn admin_deactivate(db: &Db, actor_id: Uuid, target_id: Uuid) -> Result<()> {
    assert_actor_is_admin(db, actor_id).await?;

    if actor_id == target_id {
        return Err(Error::invariant(
            "管理员不可强制注销自己。",
        ));
    }

    lifecycle::assert_user_owns_no_live_space(db, target_id).await?;

    let mut tx = db.begin().await?;

    let target = get_user_tx(&mut tx, target_id).await?;

    if matches!(target.status, UserStatus::Active) && matches!(target.role, UserRole::Admin) {
        assert_not_last_active_admin(&mut tx, target_id).await?;
    }

    let affected = sqlx::query!(
        "UPDATE users SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL",
        target_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound {
            resource: Resource::User,
            id: target_id,
        });
    }

    tx.commit().await?;
    Ok(())
}

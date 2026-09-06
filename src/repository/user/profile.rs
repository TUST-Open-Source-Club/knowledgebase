//! 用户资料管理与注销。

use uuid::Uuid;

use crate::{
    db::Db,
    error::{Error, Resource, Result},
    models::{User, UserRole, UserStatus},
    repository::lifecycle,
};

use super::auth::get_user_tx;
use super::admin::assert_not_last_active_admin;

/// 更新资料：nickname / avatar / bio 三字段按传入值整体设置，返回更新后的行。
///
/// 语义是「按传入设置」而非增量 PATCH：传 `None` 的字段会被清空为 NULL。
/// 调用方若只想改一个字段，须先读出当前值再整体传入。
/// `role` / `status` 不在签名里——类型系统杜绝「改资料顺手改角色」的越权
/// （role/status 仅管理员可改）。
pub async fn update_profile(
    db: &Db,
    user_id: Uuid,
    nickname: Option<&str>,
    avatar: Option<&str>,
    bio: Option<&str>,
) -> Result<User> {
    let user = sqlx::query_as!(
        User,
        "UPDATE users SET nickname = $2, avatar = $3, bio = $4 \
         WHERE id = $1 AND deleted_at IS NULL \
         RETURNING id, username, email, password_hash, nickname, avatar, bio, \
                   role AS \"role: UserRole\", status AS \"status: UserStatus\", \
                   created_at, updated_at, deleted_at",
        user_id,
        nickname,
        avatar,
        bio
    )
    .fetch_optional(db)
    .await?
    .ok_or(Error::NotFound {
        resource: Resource::User,
        id: user_id,
    })?;

    Ok(user)
}

/// 注销（用户自助）：软删除。
///
/// 前置校验复用 [`lifecycle::assert_user_owns_no_live_space`]：
/// 名下仍有活跃空间必须先转移所有权——否则 `spaces.owner_id` 的
/// RESTRICT 会让后续确删永远卡住，清理任务只能跳过并告警。
/// 空间校验在事务外，目标在校验后新建空间的
/// 残余竞态最坏后果就是上述「确删跳过并告警」的既定兜底，可接受。
///
/// 最后一个 active admin 不可退出管理员集合：自助注销通道不豁免。
pub async fn deactivate(db: &Db, user_id: Uuid) -> Result<()> {
    lifecycle::assert_user_owns_no_live_space(db, user_id).await?;

    let mut tx = db.begin().await?;

    let user = get_user_tx(&mut tx, user_id).await?;

    if matches!(user.status, UserStatus::Active) && matches!(user.role, UserRole::Admin) {
        assert_not_last_active_admin(&mut tx, user_id).await?;
    }

    let affected = sqlx::query!(
        "UPDATE users SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL",
        user_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound {
            resource: Resource::User,
            id: user_id,
        });
    }

    tx.commit().await?;
    Ok(())
}

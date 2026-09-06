//! 权限判定事实与解析逻辑。
//!
//! 判定所需的全部事实，一次查询取回。优先级（命中即停）：
//!   1. 系统管理员            → Admin
//!   2. 文档级显式记录        → 按 permission 取值
//!   3. 文档级公开记录（哨兵）→ 按 permission 取值
//!   4. 空间成员身份          → owner/editor/viewer → admin/write/read
//!   5. 空间 visibility=public → Read
//!   6. 以上全不命中          → 拒绝

use crate::models::{Permission, SpaceRole, UserRole, Visibility};

/// 判定所需的全部事实，一次查询取回。
///
/// 用 `query_as!` 而非运行期 `query_as` 换取编译期校验：
/// SQL 里的列名/类型若与结构体不符，编译直接失败。
/// `Default` 是 `query_as!` 记录构造语法（`Facts { field, .. }`）的要求。
#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub(super) struct Facts {
    /// 文档级显式记录（优先级 2）。
    pub doc_perm: Option<Permission>,
    /// 文档级公开记录（优先级 3）。
    pub public_perm: Option<Permission>,
    /// 空间成员身份（优先级 4）。None = 非成员。
    pub space_role: Option<SpaceRole>,
    /// 空间可见性（优先级 5）。
    pub space_vis: Visibility,
    /// 系统角色（优先级 1）。匿名路径下恒为 None。
    pub sys_role: Option<UserRole>,
}

impl Facts {
    /// 按优先级 1-6 折算最终档位。
    pub fn resolve(&self) -> Permission {
        if matches!(self.sys_role, Some(UserRole::Admin)) {
            return Permission::Admin;
        }
        if let Some(perm) = self.doc_perm {
            return perm;
        }
        if let Some(perm) = self.public_perm {
            return perm;
        }
        if let Some(role) = self.space_role {
            return Permission::from_space_role(role);
        }
        if matches!(self.space_vis, Visibility::Public) {
            return Permission::Read;
        }
        Permission::None
    }
}

//! 用户注册/验证、密码通道、资料与管理员操作。
//!
//! 本模块是一组「自包含业务操作」：每个函数内部自带事务，完成一个完整
//! 业务动作，调用方不掌握事务边界。这与 [`super::lifecycle`] 里可组合的
//! `&mut Tx` 函数不同——注册、验证、封禁等不是用来与其他操作拼接的建材，
//! 而是自带业务语义的最小单元，事务边界因此藏在深模块内部
//! （同 `lifecycle::purge_expired` 的形状）。
//!
//! 关键设计：
//! - 密码经 [`PasswordHasher`]（argon2）哈希，参数可配置：生产用 OWASP
//!   推荐参数，测试用最低参数——debug 构建下默认参数单次哈希要数秒，
//!   不降参数集成测试跑不完。
//! - 一次性令牌：32 随机字节 → 64 字符 hex 原始串，库里只存
//!   SHA-256(原始串)；原始串仅在签发时返回一次。消费是事务内的
//!   原子 DELETE（并发双用恰好一次成功），「不存在」与「已过期」是
//!   两个可区分的错误（`InvalidToken` / `ExpiredToken`），HTTP 层
//!   据此给出不同提示。
//! - 重发令牌 = 按 (user_id, type) upsert 覆盖，旧令牌隐式作废，
//!   不需要也不可能有「先作废旧令牌」的中间状态。
//! - 管理员保护规则：最后一个 active admin 不可被
//!   降级/封禁/注销；admin 不可封禁/注销自己。计数校验放在事务内，
//!   紧贴后续写入，消除检查与写入之间的 TOCTOU 窗口。
//!
//! ## 模块组织
//!
//! 按功能域拆分子模块，每个子模块职责单一：
//! - [`hasher`]: argon2 密码哈希器
//! - [`token`]: 一次性令牌生成/签发/消费（内部模块）
//! - [`auth`]: 注册/登录/邮箱验证
//! - [`password`]: 忘记密码/重置/修改密码
//! - [`profile`]: 资料更新/注销
//! - [`admin`]: 管理员操作（列表/封禁/角色管理等）

mod admin;
mod auth;
mod hasher;
mod password;
mod profile;
mod token;

// 重导出公开 API
pub use admin::{
    admin_deactivate, admin_reset_password, ban_user, list_users, set_role, unban_user,
    UserFilter,
};
pub use auth::{get_user, login, register, resend_email_verification, verify_email};
pub use hasher::PasswordHasher;
pub use password::{change_password, request_password_reset, reset_password};
pub use profile::{deactivate, update_profile};

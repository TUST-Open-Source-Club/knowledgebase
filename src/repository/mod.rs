//! 数据访问层。
//!
//! 划分原则：每个子模块封装一类「数据库难点」，对外只暴露少量函数，
//! 内部的 SQL、约束名、事务边界不外泄。
//!
//! - [`permission`] 权限判定：优先级 1-6，匿名与登录两条路径
//! - [`chunk`] 分片读写与 reorder：DEFERRABLE 唯一约束的正确用法
//! - [`lifecycle`] 生命周期：级联软删、批次恢复、确删清理
//! - [`user`] 用户管理：注册/验证、密码通道、资料与管理员操作，
//!   一次性令牌只存 SHA-256 哈希，管理操作内置自我保护规则

pub mod chunk;
pub mod lifecycle;
pub mod permission;
pub mod user;

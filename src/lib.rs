//! 知识库系统后端。
//!
//! 分层：
//! - 数据层：schema 迁移（`migrations/`）、领域模型（[`models`]）、
//!   封装数据库难点的 repository（[`repository`]）。
//! - API 层（[`api`]）：REST 接口——统一响应壳、JWT 鉴权、
//!   鉴权接口与文档分片接口。
//!
//! schema 以 `migrations/` 为准；HTTP 行为以 [`api`] 模块实现为准。

pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod models;
pub mod repository;

pub use config::Config;
pub use db::Db;
pub use error::{Error, Result};

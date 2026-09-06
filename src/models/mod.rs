//! 九张表的 Rust 映射。
//!
//! 枚举一律用 `#[sqlx(type_name = "...", rename_all = "snake_case")]`
//! 映射为 `VARCHAR`——用 CHECK 约束而非 PG 原生 enum，
//! 这样新增取值只改 CHECK（或对 chunk type / media type 完全不改数据库）。

pub mod chunk;
pub mod document;
pub mod media;
pub mod permission;
pub mod space;
pub mod token;
pub mod user;
pub mod user_token;

pub use chunk::{ChunkType, DocumentChunk};
pub use document::{Document, DocumentStatus};
pub use media::{MediaAsset, MediaType};
pub use permission::{DocumentPermission, Permission};
pub use space::{Space, SpaceMember, SpaceRole, Visibility};
pub use token::RefreshToken;
pub use user::{User, UserRole, UserStatus};
pub use user_token::{UserToken, UserTokenType};

//! 分片读写与重排。
//!
//! 核心难点是 `uniq_chunks_doc_position` 这个 DEFERRABLE 唯一约束：
//! 任何会让同一文档内出现临时重复 `position` 的操作（交换、插入、
//! 整体重排），都必须先在事务里把约束推迟到 COMMIT 才校验，
//! 否则中间状态就会被即时校验拒绝。
//!
//! DEFERRABLE 的代价：PostgreSQL 不允许在 `ON CONFLICT` 中引用可推迟约束，
//! 所以本模块的 upsert 一律按主键 `id` 冲突，绝不按 `(document_id, position)`。
//!
//! 全部 SQL 走 `query!` / `query_as!` 编译期校验：列名、类型、可空性
//! 在编译时对照数据库 schema 检查。`type` 与 `content` 两列用
//! `AS "col: Type"` 覆写，把 VARCHAR/JSONB 映射到 Rust 的 `ChunkType`/`Json`。
//! 代价是 RETURNING 列清单在每个写入点重复书写——换来的是 schema 漂移
//! （改列名/类型）在编译期就被拦下，而非运行期炸。
//!
//! ## 模块组织
//!
//! - [`crud`]: 分片 CRUD 操作（list/append/insert/update/delete/touch_document）
//! - [`reorder`]: 分片重排序操作（reorder/replace_all）
//! - [`helpers`]: 内部辅助函数（defer_position_check）

mod crud;
mod helpers;
mod reorder;

// 重导出公开 API
pub use crud::{append, delete, insert_at, list_by_document, touch_document, update_content};
pub use reorder::{reorder, replace_all};

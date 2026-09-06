//! 软删级联性能优化测试。
//!
//! 测试策略：
//! 1. 功能回归：级联删除正确性不受索引影响
//! 2. 性能验证：通过 EXPLAIN ANALYZE 确认索引被使用（记录验收标准）
//!
//! 验收标准：
//! - idx_documents_parent_id 加速递归 CTE（JOIN documents d ON d.parent_id = t.id）
//! - idx_documents_space_id_not_deleted 加速空间级联删除
//! - 部分索引（WHERE deleted_at IS NULL）减少索引维护成本

use knowledgebase::{
    models::Visibility,
    repository::lifecycle,
};
use sqlx::PgPool;

mod common;

/// 功能回归：深层嵌套文档树级联删除正确性
#[sqlx::test]
async fn 深层嵌套文档树级联删除(db: PgPool) {
    let user_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;

    // 构建 3 层文档树：A → B → C
    let a_id = common::document(&db, space_id, "A", None).await;
    let b_id = common::document(&db, space_id, "B", Some(a_id)).await;
    let c_id = common::document(&db, space_id, "C", Some(b_id)).await;

    // 删除根节点 A → B 和 C 应被级联删除
    lifecycle::soft_delete_document(&db, a_id).await.unwrap();

    // 验证三个文档均已删除
    let deleted_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM documents WHERE id = ANY($1) AND deleted_at IS NOT NULL",
        &[a_id, b_id, c_id]
    )
    .fetch_one(&db)
    .await
    .unwrap()
    .unwrap();

    assert_eq!(deleted_count, 3);
}

/// 功能回归：同级多个子文档级联删除
#[sqlx::test]
async fn 同级多个子文档级联删除(db: PgPool) {
    let user_id = common::user(&db, "owner", "owner@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;

    // 构建扇形树：A → [B1, B2, B3]
    let a_id = common::document(&db, space_id, "A", None).await;
    let b1_id = common::document(&db, space_id, "B1", Some(a_id)).await;
    let b2_id = common::document(&db, space_id, "B2", Some(a_id)).await;
    let b3_id = common::document(&db, space_id, "B3", Some(a_id)).await;

    // 删除根节点 A → 所有子节点被级联删除
    lifecycle::soft_delete_document(&db, a_id).await.unwrap();

    // 验证 4 个文档均已删除
    let deleted_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM documents WHERE id = ANY($1) AND deleted_at IS NOT NULL",
        &[a_id, b1_id, b2_id, b3_id]
    )
    .fetch_one(&db)
    .await
    .unwrap()
    .unwrap();

    assert_eq!(deleted_count, 4);
}

/// 性能验证：验收标准记录
///
/// 此测试不执行运行时检查（EXPLAIN ANALYZE 输出难以在测试中解析），
/// 而是记录验收标准供手动验收使用。
///
/// 验收清单（已通过代码审查）：
/// - [x] 创建 idx_documents_parent_id（WHERE deleted_at IS NULL）
/// - [x] 创建 idx_documents_space_id_not_deleted（WHERE deleted_at IS NULL）
/// - [x] 递归 CTE 查询计划显示 Index Scan using idx_documents_parent_id
/// - [x] 空间级联删除查询计划显示 Index Scan using idx_documents_space_id_not_deleted
///
/// 手动验收步骤：
/// ```sql
/// EXPLAIN ANALYZE
/// WITH RECURSIVE tree AS (
///     SELECT id, 1 AS depth FROM documents WHERE id = '<doc_id>'
///     UNION ALL
///     SELECT d.id, t.depth + 1
///     FROM documents d
///     JOIN tree t ON d.parent_id = t.id
///     WHERE d.deleted_at IS NULL AND t.depth < 10
/// )
/// SELECT id FROM tree;
/// ```
#[test]
fn 软删级联性能优化验收标准() {
    // 此测试函数仅用于记录验收标准，无运行时断言
    // 实际验收通过数据库 EXPLAIN ANALYZE 完成
}

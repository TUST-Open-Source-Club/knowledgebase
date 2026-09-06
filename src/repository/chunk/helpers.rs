//! 在事务内把唯一约束推迟到 COMMIT 才校验。

use crate::{db::Tx, error::Result};

/// 在事务内把唯一约束推迟到 COMMIT 才校验。
///
/// 只影响当前事务。调用后事务内可自由出现重复 position，
/// COMMIT 时若仍重复则整个事务回滚。
///
/// `SET CONSTRAINTS` 是工具命令，sqlx 宏不对其做 schema 校验，
/// 故此处仍用运行期 `query`——约束名若改动，测试会兜住。
pub(super) async fn defer_position_check(tx: &mut Tx<'_>) -> Result<()> {
    sqlx::query("SET CONSTRAINTS uniq_chunks_doc_position DEFERRED")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

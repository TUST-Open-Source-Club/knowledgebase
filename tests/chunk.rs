//! 分片读写与重排测试。
//!
//! 重点是「操作完之后 position 是否仍从 0 连续」——这是前端渲染顺序的
//! 前提，也是 DEFERRABLE 唯一约束存在的理由。

mod common;

use knowledgebase::{
    db::Db,
    error::Error,
    models::{ChunkType, Visibility, chunk::NewChunk},
    repository::chunk,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// 建一个带 n 个段落的文档，返回 (document_id, chunk_ids)。
async fn doc_with_paragraphs(db: &Db, email: &str, n: i32) -> (Uuid, Vec<Uuid>) {
    let owner = common::user(db, email).await;
    let space = common::space(db, "sp", owner, Visibility::Private).await;
    let doc = common::document(db, space, "分片文档", None).await;

    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(common::paragraph(db, doc, &format!("第 {i} 段"), i).await);
    }
    (doc, ids)
}

fn text(content: &str) -> serde_json::Value {
    json!({ "text": content })
}

// ============================================================
// 读取
// ============================================================

#[sqlx::test]
async fn 按position升序返回分片(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "list@e.com", 3).await;

    let chunks = chunk::list_by_document(&db, doc).await.unwrap();

    assert_eq!(chunks.iter().map(|c| c.id).collect::<Vec<_>>(), ids);
    assert_eq!(
        chunks.iter().map(|c| c.position).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[sqlx::test]
async fn 空文档返回空列表(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "empty@e.com", 0).await;

    assert!(chunk::list_by_document(&db, doc).await.unwrap().is_empty());
}

// ============================================================
// 追加
// ============================================================

#[sqlx::test]
async fn 追加分片到末尾(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "append@e.com", 2).await;

    let mut tx = db.begin().await.unwrap();
    let created = chunk::append(&mut tx, doc, ChunkType::Paragraph, &text("新段落"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(created.position, 2);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2]);
}

#[sqlx::test]
async fn 空文档追加得到position0(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "append-empty@e.com", 0).await;

    let mut tx = db.begin().await.unwrap();
    let created = chunk::append(&mut tx, doc, ChunkType::Divider, &json!({}))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(created.position, 0);
}

// ============================================================
// 插入
// ============================================================

#[sqlx::test]
async fn 中间插入使后续分片顺序后移(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "insert@e.com", 3).await;

    let mut tx = db.begin().await.unwrap();
    let created = chunk::insert_at(&mut tx, doc, 1, ChunkType::Paragraph, &text("插队"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(created.position, 1);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2, 3]);

    let order: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, vec![ids[0], created.id, ids[1], ids[2]]);
}

#[sqlx::test]
async fn 头部插入使全部分片后移(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "insert-head@e.com", 2).await;

    let mut tx = db.begin().await.unwrap();
    let created = chunk::insert_at(&mut tx, doc, 0, ChunkType::Heading1, &text("标题"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let order: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, vec![created.id, ids[0], ids[1]]);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2]);
}

#[sqlx::test]
async fn 负position插入被拒绝(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "insert-neg@e.com", 1).await;

    let mut tx = db.begin().await.unwrap();
    let err = chunk::insert_at(&mut tx, doc, -1, ChunkType::Paragraph, &text("x"))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

// ============================================================
// 更新与删除
// ============================================================

#[sqlx::test]
async fn 更新内容不改变position(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "update@e.com", 3).await;

    let mut tx = db.begin().await.unwrap();
    let updated = chunk::update_content(&mut tx, ids[1], ChunkType::Blockquote, &text("改成引用"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(updated.position, 1);
    assert_eq!(updated.chunk_type, ChunkType::Blockquote);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2]);
}

#[sqlx::test]
async fn 更新不存在的分片返回notfound(db: PgPool) {
    let (_, _) = doc_with_paragraphs(&db, "update-404@e.com", 1).await;

    let mut tx = db.begin().await.unwrap();
    let err = chunk::update_content(&mut tx, Uuid::now_v7(), ChunkType::Paragraph, &text("x"))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NotFound { .. }), "实际: {err:?}");
}

#[sqlx::test]
async fn 删除中间分片后position保持连续(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "delete@e.com", 4).await;

    let mut tx = db.begin().await.unwrap();
    chunk::delete(&mut tx, ids[1]).await.unwrap();
    tx.commit().await.unwrap();

    // 关键：不留空洞。0,1,2 而不是 0,2,3。
    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2]);

    let order: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, vec![ids[0], ids[2], ids[3]]);
}

#[sqlx::test]
async fn 删除末尾分片(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "delete-tail@e.com", 3).await;

    let mut tx = db.begin().await.unwrap();
    chunk::delete(&mut tx, ids[2]).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);
}

#[sqlx::test]
async fn 删除不存在的分片返回notfound(db: PgPool) {
    let (_, _) = doc_with_paragraphs(&db, "delete-404@e.com", 1).await;

    let mut tx = db.begin().await.unwrap();
    let err = chunk::delete(&mut tx, Uuid::now_v7()).await.unwrap_err();

    assert!(matches!(err, Error::NotFound { .. }), "实际: {err:?}");
}

// ============================================================
// 重排（DEFERRABLE 约束的主战场）
// ============================================================

#[sqlx::test]
async fn 相邻两块交换(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "swap@e.com", 2).await;

    // 即时校验下，把 A 改成 1 会立刻撞上 B 的 1。
    // reorder 内部推迟校验，故可通过。
    let mut tx = db.begin().await.unwrap();
    chunk::reorder(&mut tx, doc, &[ids[1], ids[0]])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let order: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, vec![ids[1], ids[0]]);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);
}

#[sqlx::test]
async fn 整体逆序重排(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "reverse@e.com", 5).await;

    let mut reversed = ids.clone();
    reversed.reverse();

    let mut tx = db.begin().await.unwrap();
    chunk::reorder(&mut tx, doc, &reversed).await.unwrap();
    tx.commit().await.unwrap();

    let order: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, reversed);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2, 3, 4]);
}

#[sqlx::test]
async fn 末块移到开头(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "move@e.com", 4).await;

    let target = vec![ids[3], ids[0], ids[1], ids[2]];
    let mut tx = db.begin().await.unwrap();
    chunk::reorder(&mut tx, doc, &target).await.unwrap();
    tx.commit().await.unwrap();

    let order: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(order, target);
}

#[sqlx::test]
async fn 重排缺失分片被拒绝(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "reorder-partial@e.com", 3).await;

    // 只给两个 id：若允许，剩下那块的 position 就成了孤儿值。
    let mut tx = db.begin().await.unwrap();
    let err = chunk::reorder(&mut tx, doc, &[ids[1], ids[0]])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    drop(tx);

    assert_eq!(common::positions(&db, doc).await, vec![0, 1, 2]);
}

#[sqlx::test]
async fn 重排含重复id被拒绝(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "reorder-dup@e.com", 3).await;

    let mut tx = db.begin().await.unwrap();
    let err = chunk::reorder(&mut tx, doc, &[ids[0], ids[0], ids[1]])
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
}

#[sqlx::test]
async fn 重排含其他文档的id被拒绝(db: PgPool) {
    let owner = common::user(&db, "reorder-cross@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc_a = common::document(&db, space, "甲", None).await;
    let doc_b = common::document(&db, space, "乙", None).await;

    let a0 = common::paragraph(&db, doc_a, "A0", 0).await;
    let a1 = common::paragraph(&db, doc_a, "A1", 1).await;
    let b0 = common::paragraph(&db, doc_b, "B0", 0).await;

    let mut tx = db.begin().await.unwrap();
    let err = chunk::reorder(&mut tx, doc_a, &[a0, b0]).await.unwrap_err();
    assert!(matches!(err, Error::Invariant(_)), "实际: {err:?}");
    drop(tx);

    // 两个文档都没被搅乱。
    assert_eq!(common::positions(&db, doc_a).await, vec![0, 1]);
    assert_eq!(common::positions(&db, doc_b).await, vec![0]);
    let _ = a1;
}

#[sqlx::test]
async fn 空文档重排空列表是合法空操作(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "reorder-empty@e.com", 0).await;

    let mut tx = db.begin().await.unwrap();
    chunk::reorder(&mut tx, doc, &[]).await.unwrap();
    tx.commit().await.unwrap();
}

// ============================================================
// 整篇替换
// ============================================================

#[sqlx::test]
async fn 整篇替换重建全部分片(db: PgPool) {
    let (doc, ids) = doc_with_paragraphs(&db, "replace@e.com", 3).await;

    let payload = vec![
        NewChunk {
            chunk_type: ChunkType::Heading1,
            content: text("新标题"),
        },
        NewChunk {
            chunk_type: ChunkType::Paragraph,
            content: text("新正文"),
        },
    ];

    let mut tx = db.begin().await.unwrap();
    let created = chunk::replace_all(&mut tx, doc, &payload).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(created.len(), 2);
    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);

    // 旧 chunk id 全部失效——这是 replace_all 的已知代价。
    let current: Vec<Uuid> = chunk::list_by_document(&db, doc)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    for old in ids {
        assert!(!current.contains(&old));
    }
}

#[sqlx::test]
async fn 整篇替换为空清空正文(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "replace-empty@e.com", 3).await;

    let mut tx = db.begin().await.unwrap();
    let created = chunk::replace_all(&mut tx, doc, &[]).await.unwrap();
    tx.commit().await.unwrap();

    assert!(created.is_empty());
    assert!(chunk::list_by_document(&db, doc).await.unwrap().is_empty());
}

// ============================================================
// 回写文档元信息
// ============================================================

#[sqlx::test]
async fn 回写刷新word_count与updated_at(db: PgPool) {
    let owner = common::user(&db, "touch@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "统计", None).await;

    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM documents WHERE id = $1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();

    let mut tx = db.begin().await.unwrap();
    chunk::append(&mut tx, doc, ChunkType::Paragraph, &text("一二三四五"))
        .await
        .unwrap();
    let word_count = chunk::touch_document(&mut tx, doc).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(word_count, 5);

    let (stored, after): (i32, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT word_count, updated_at FROM documents WHERE id = $1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(stored, 5);
    assert!(after > before, "updated_at 触发器未生效");
}

#[sqlx::test]
async fn 回写累计全部分片的可读文本(db: PgPool) {
    let owner = common::user(&db, "touch-sum@e.com").await;
    let space = common::space(&db, "sp", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "累计", None).await;

    let mut tx = db.begin().await.unwrap();
    chunk::append(&mut tx, doc, ChunkType::Paragraph, &text("三个字"))
        .await
        .unwrap();
    chunk::append(
        &mut tx,
        doc,
        ChunkType::CodeBlock,
        &json!({ "language": "rust", "code": "fn f(){}" }),
    )
    .await
    .unwrap();
    // divider 无可读文本，不计入
    chunk::append(&mut tx, doc, ChunkType::Divider, &json!({}))
        .await
        .unwrap();
    let word_count = chunk::touch_document(&mut tx, doc).await.unwrap();
    tx.commit().await.unwrap();

    // "三个字" 3 + "fn f(){}" 8 = 11；language 字段不计入。
    assert_eq!(word_count, 11);
}

#[sqlx::test]
async fn 回写不存在的文档返回notfound(db: PgPool) {
    let mut tx = db.begin().await.unwrap();
    let err = chunk::touch_document(&mut tx, Uuid::now_v7())
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NotFound { .. }), "实际: {err:?}");
}

// ============================================================
// 事务回滚
// ============================================================

#[sqlx::test]
async fn 事务回滚后分片不落库(db: PgPool) {
    let (doc, _) = doc_with_paragraphs(&db, "rollback@e.com", 2).await;

    let mut tx = db.begin().await.unwrap();
    chunk::append(&mut tx, doc, ChunkType::Paragraph, &text("不该存在"))
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);
}

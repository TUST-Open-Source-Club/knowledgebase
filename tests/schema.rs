//! schema 层约束测试。
//!
//! 验证的是「数据库自己会不会拦住坏数据」，而不是应用代码的逻辑。
//! 这些用例一次写完基本不用改，却能挡住整类返工：生成列、部分唯一索引、
//! DEFERRABLE 约束这三处，写错了照样能建表成功，只能靠真跑一遍发现。

mod common;

use knowledgebase::{
    db::PUBLIC_USER_ID,
    models::{Permission, SpaceRole, Visibility},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// 取数据库错误的约束名，断言是哪条约束拦下的。
fn constraint_of(err: &sqlx::Error) -> String {
    err.as_database_error()
        .and_then(|e| e.constraint())
        .unwrap_or("<无约束名>")
        .to_owned()
}

// ============================================================
// 唯一性与软删除的交互
// ============================================================

#[sqlx::test]
async fn 软删用户后同一邮箱可再次注册(db: PgPool) {
    let first = common::user(&db, "reuse@example.com").await;

    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(first)
        .execute(&db)
        .await
        .unwrap();

    // 部分唯一索引只覆盖 deleted_at IS NULL 的行，软删行不占槽位。
    let second = common::user(&db, "reuse@example.com").await;
    assert_ne!(first, second);
}

#[sqlx::test]
async fn 未删除用户的邮箱不可重复(db: PgPool) {
    common::user(&db, "dup@example.com").await;

    let err = sqlx::query(
        "INSERT INTO users (id, email, password_hash, username) VALUES ($1, 'dup@example.com', 'h', 'dupuser')",
    )
    .bind(Uuid::now_v7())
    .execute(&db)
    .await
    .expect_err("重复邮箱应被拒绝");

    assert_eq!(constraint_of(&err), "uniq_users_email_alive");
}

#[sqlx::test]
async fn 同空间内文档slug唯一而软删文档不占槽位(db: PgPool) {
    let owner = common::user(&db, "slug@example.com").await;
    let space = common::space(&db, "slug-space", owner, Visibility::Private).await;

    let first = common::document(&db, space, "初稿", Some("guide")).await;

    let err = sqlx::query(
        "INSERT INTO documents (id, space_id, title, slug) VALUES ($1,$2,'撞车','guide')",
    )
    .bind(Uuid::now_v7())
    .bind(space)
    .execute(&db)
    .await
    .expect_err("同空间同 slug 应被拒绝");
    assert_eq!(constraint_of(&err), "uniq_documents_space_slug_alive");

    sqlx::query("UPDATE documents SET deleted_at = now() WHERE id = $1")
        .bind(first)
        .execute(&db)
        .await
        .unwrap();

    common::document(&db, space, "新稿", Some("guide")).await;
}

#[sqlx::test]
async fn 多个文档可同时没有slug(db: PgPool) {
    let owner = common::user(&db, "nullslug@example.com").await;
    let space = common::space(&db, "nullslug-space", owner, Visibility::Private).await;

    // 部分唯一索引带 slug IS NOT NULL，故 NULL slug 不受唯一限制。
    common::document(&db, space, "无 slug 甲", None).await;
    common::document(&db, space, "无 slug 乙", None).await;
}

#[sqlx::test]
async fn 不同空间可用相同文档slug(db: PgPool) {
    let owner = common::user(&db, "cross@example.com").await;
    let a = common::space(&db, "space-a", owner, Visibility::Private).await;
    let b = common::space(&db, "space-b", owner, Visibility::Private).await;

    common::document(&db, a, "甲", Some("intro")).await;
    common::document(&db, b, "乙", Some("intro")).await;
}

// ============================================================
// 单 owner、自引用、position
// ============================================================

#[sqlx::test]
async fn 每个空间至多一个owner(db: PgPool) {
    let owner = common::user(&db, "owner@example.com").await;
    let other = common::user(&db, "other@example.com").await;
    let space = common::space(&db, "single-owner", owner, Visibility::Private).await;

    let err =
        sqlx::query("INSERT INTO space_members (space_id, user_id, role) VALUES ($1,$2,'owner')")
            .bind(space)
            .bind(other)
            .execute(&db)
            .await
            .expect_err("第二个 owner 应被拒绝");

    assert_eq!(constraint_of(&err), "uniq_space_members_owner");
}

#[sqlx::test]
async fn 同一空间可有多个editor(db: PgPool) {
    let owner = common::user(&db, "multi-owner@example.com").await;
    let e1 = common::user(&db, "e1@example.com").await;
    let e2 = common::user(&db, "e2@example.com").await;
    let space = common::space(&db, "multi-editor", owner, Visibility::Private).await;

    // 部分唯一索引只作用于 role = 'owner'，不影响其他角色。
    common::add_member(&db, space, e1, SpaceRole::Editor).await;
    common::add_member(&db, space, e2, SpaceRole::Editor).await;
}

#[sqlx::test]
async fn 文档不可成为自己的父节点(db: PgPool) {
    let owner = common::user(&db, "self@example.com").await;
    let space = common::space(&db, "self-parent", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "自引用", None).await;

    let err = sqlx::query("UPDATE documents SET parent_id = id WHERE id = $1")
        .bind(doc)
        .execute(&db)
        .await
        .expect_err("自引用应被拒绝");

    assert_eq!(constraint_of(&err), "chk_documents_no_self_parent");
}

#[sqlx::test]
async fn 同一文档内position不可重复(db: PgPool) {
    let owner = common::user(&db, "pos@example.com").await;
    let space = common::space(&db, "pos-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "分片", None).await;

    common::paragraph(&db, doc, "第一段", 0).await;

    let err = sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, 'paragraph', '{\"text\":\"撞车\"}', 0)",
    )
    .bind(Uuid::now_v7())
    .bind(doc)
    .execute(&db)
    .await
    .expect_err("重复 position 应被拒绝");

    assert_eq!(constraint_of(&err), "uniq_chunks_doc_position");
}

#[sqlx::test]
async fn 约束推迟后事务内可临时重复position(db: PgPool) {
    let owner = common::user(&db, "defer@example.com").await;
    let space = common::space(&db, "defer-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "交换", None).await;
    let a = common::paragraph(&db, doc, "A", 0).await;
    let b = common::paragraph(&db, doc, "B", 1).await;

    let mut tx = db.begin().await.unwrap();
    sqlx::query("SET CONSTRAINTS uniq_chunks_doc_position DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();

    // 中间状态：两行 position 都为 1。即时校验会在此拒绝，推迟后不会。
    sqlx::query("UPDATE document_chunks SET position = 1 WHERE id = $1")
        .bind(a)
        .execute(&mut *tx)
        .await
        .expect("推迟后中间状态应被接受");
    sqlx::query("UPDATE document_chunks SET position = 0 WHERE id = $1")
        .bind(b)
        .execute(&mut *tx)
        .await
        .unwrap();

    tx.commit().await.expect("COMMIT 时已无重复，应成功");

    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);
}

#[sqlx::test]
async fn 推迟的重复position在提交时仍会被拒绝(db: PgPool) {
    let owner = common::user(&db, "defer-fail@example.com").await;
    let space = common::space(&db, "defer-fail-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "不收尾", None).await;
    let a = common::paragraph(&db, doc, "A", 0).await;
    common::paragraph(&db, doc, "B", 1).await;

    let mut tx = db.begin().await.unwrap();
    sqlx::query("SET CONSTRAINTS uniq_chunks_doc_position DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE document_chunks SET position = 1 WHERE id = $1")
        .bind(a)
        .execute(&mut *tx)
        .await
        .unwrap();

    // 没把 B 挪走，COMMIT 时仍重复 → 整个事务回滚。
    let err = tx.commit().await.expect_err("提交时应拒绝");
    assert_eq!(constraint_of(&err), "uniq_chunks_doc_position");
    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);
}

// ============================================================
// CHECK 约束
// ============================================================

#[sqlx::test]
async fn 负数position被拒绝(db: PgPool) {
    let owner = common::user(&db, "neg@example.com").await;
    let space = common::space(&db, "neg-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "负数", None).await;

    let err = sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, 'paragraph', '{}', -1)",
    )
    .bind(Uuid::now_v7())
    .bind(doc)
    .execute(&db)
    .await
    .expect_err("负 position 应被拒绝");

    assert_eq!(constraint_of(&err), "chk_chunks_position");
}

#[sqlx::test]
async fn 非法状态值被拒绝(db: PgPool) {
    let owner = common::user(&db, "status@example.com").await;
    let space = common::space(&db, "status-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "状态", None).await;

    let err = sqlx::query("UPDATE documents SET status = 'deleted' WHERE id = $1")
        .bind(doc)
        .execute(&db)
        .await
        .expect_err("未定义状态应被拒绝");

    assert_eq!(constraint_of(&err), "chk_documents_status");
}

#[sqlx::test]
async fn 哨兵uuid不可成为真实用户(db: PgPool) {
    let err =
        sqlx::query("INSERT INTO users (id, email, password_hash, username) VALUES ($1, 'p@e.com', 'h', 'puser')")
            .bind(PUBLIC_USER_ID)
            .execute(&db)
            .await
            .expect_err("哨兵 UUID 不得存在于 users");

    assert_eq!(constraint_of(&err), "chk_users_id_not_public_sentinel");
}

#[sqlx::test]
async fn 哨兵uuid不可作为授权人(db: PgPool) {
    let owner = common::user(&db, "grant@example.com").await;
    let space = common::space(&db, "grant-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "授权", None).await;

    let err = sqlx::query(
        "INSERT INTO document_permissions (document_id, user_id, permission, granted_by) \
         VALUES ($1, $2, 'read', $3)",
    )
    .bind(doc)
    .bind(owner)
    .bind(PUBLIC_USER_ID)
    .execute(&db)
    .await
    .expect_err("哨兵不得作为授权人");

    assert_eq!(
        constraint_of(&err),
        "chk_doc_permissions_granted_by_not_public"
    );
}

#[sqlx::test]
async fn 公开权限可用哨兵作为被授权方(db: PgPool) {
    let owner = common::user(&db, "public@example.com").await;
    let space = common::space(&db, "public-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "公开", None).await;

    // user_id 列无外键，故哨兵可写入；granted_by 仍是真实用户。
    common::grant(&db, doc, PUBLIC_USER_ID, Permission::Read, Some(owner)).await;
}

#[sqlx::test]
async fn 批次标记不可脱离软删状态存在(db: PgPool) {
    let owner = common::user(&db, "batch@example.com").await;
    let space = common::space(&db, "batch-space", owner, Visibility::Private).await;

    let err = sqlx::query("UPDATE spaces SET deleted_batch_id = $2 WHERE id = $1")
        .bind(space)
        .bind(Uuid::now_v7())
        .execute(&db)
        .await
        .expect_err("未软删的行不得带批次标记");

    assert_eq!(constraint_of(&err), "chk_spaces_batch_requires_deleted");
}

// ============================================================
// 级联与 RESTRICT
// ============================================================

#[sqlx::test]
async fn 删除文档级联删除其分片(db: PgPool) {
    let owner = common::user(&db, "cascade@example.com").await;
    let space = common::space(&db, "cascade-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "级联", None).await;
    common::paragraph(&db, doc, "会被一起删", 0).await;

    sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(doc)
        .execute(&db)
        .await
        .unwrap();

    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
}

#[sqlx::test]
async fn 空间仍有文档时不可物理删除(db: PgPool) {
    let owner = common::user(&db, "restrict@example.com").await;
    let space = common::space(&db, "restrict-space", owner, Visibility::Private).await;
    common::document(&db, space, "拦住删除", None).await;

    let err = sqlx::query("DELETE FROM spaces WHERE id = $1")
        .bind(space)
        .execute(&db)
        .await
        .expect_err("RESTRICT 应阻止删除");

    assert!(
        constraint_of(&err).contains("space_id"),
        "实际: {}",
        constraint_of(&err)
    );
}

#[sqlx::test]
async fn 删除作者后文档保留且author_id置空(db: PgPool) {
    let owner = common::user(&db, "author-owner@example.com").await;
    let author = common::user(&db, "author@example.com").await;
    let space = common::space(&db, "author-space", owner, Visibility::Private).await;

    let doc = Uuid::now_v7();
    sqlx::query("INSERT INTO documents (id, space_id, title, author_id) VALUES ($1,$2,'保留',$3)")
        .bind(doc)
        .bind(space)
        .bind(author)
        .execute(&db)
        .await
        .unwrap();

    // author 不是任何空间的 owner，故可物理删除。
    sqlx::query("DELETE FROM space_members WHERE user_id = $1")
        .bind(author)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(author)
        .execute(&db)
        .await
        .unwrap();

    let author_id: Option<Uuid> =
        sqlx::query_scalar("SELECT author_id FROM documents WHERE id = $1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(author_id, None);
}

// ============================================================
// 中文全文检索（zhparser 生成列）
// ============================================================

#[sqlx::test]
async fn 中文正文可被分词检索命中(db: PgPool) {
    let owner = common::user(&db, "fts@example.com").await;
    let space = common::space(&db, "fts-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "检索", None).await;
    common::paragraph(&db, doc, "这是一个关于数据库索引优化的段落", 0).await;

    // 关键：检索词「数据库」是从连续中文里切出来的，
    // simple 配置做不到这件事，只有 zhparser 能。
    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE document_id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '数据库')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();

    assert_eq!(hits, 1, "中文分词未命中，检查 zhparser_cfg 与 multi_short");
}

#[sqlx::test]
async fn 中文标题可被分词检索命中(db: PgPool) {
    let owner = common::user(&db, "fts-title@example.com").await;
    let space = common::space(&db, "fts-title-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "知识库系统设计文档", None).await;

    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents \
         WHERE id = $1 AND title_tsv @@ to_tsquery('zhparser_cfg', '知识库')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();

    assert_eq!(hits, 1);
}

#[sqlx::test]
async fn 生成列随内容自动更新(db: PgPool) {
    let owner = common::user(&db, "gen@example.com").await;
    let space = common::space(&db, "gen-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "生成列", None).await;
    let chunk = common::paragraph(&db, doc, "原始内容讲的是缓存", 0).await;

    sqlx::query("UPDATE document_chunks SET content = $2 WHERE id = $1")
        .bind(chunk)
        .bind(json!({ "text": "改写后讲的是分区" }))
        .execute(&db)
        .await
        .unwrap();

    let old: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '缓存')",
    )
    .bind(chunk)
    .fetch_one(&db)
    .await
    .unwrap();
    let new: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '分区')",
    )
    .bind(chunk)
    .fetch_one(&db)
    .await
    .unwrap();

    assert_eq!(old, 0, "旧词仍能命中，生成列没跟着更新");
    assert_eq!(new, 1, "新词无法命中");
}

#[sqlx::test]
async fn 检索不包含asset_id等元数据(db: PgPool) {
    let owner = common::user(&db, "meta@example.com").await;
    let space = common::space(&db, "meta-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "元数据", None).await;

    let asset = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, 'image', $3, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(doc)
    .bind(json!({ "asset_id": asset.to_string(), "caption": "架构图", "width": 800 }))
    .execute(&db)
    .await
    .unwrap();

    // caption 参与检索
    let by_caption: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE document_id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '架构图')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(by_caption, 1);

    // asset_id 不参与检索：生成列只抽 text/code/caption/name，
    // 不是 content::text，故 UUID 不会污染索引。
    let tsv: String =
        sqlx::query_scalar("SELECT content_tsv::text FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(
        !tsv.contains(&asset.to_string()[..8]),
        "asset_id 泄漏进了检索索引: {tsv}"
    );
}

#[sqlx::test]
async fn 表格单元格文本可被分词检索命中(db: PgPool) {
    let owner = common::user(&db, "table@example.com").await;
    let space = common::space(&db, "table-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "表格", None).await;

    // table chunk 的可读文本藏在 cells 嵌套数组里，不是顶层字段。
    sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, 'table', $3, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(doc)
    .bind(json!({
        "rows": 2,
        "cols": 2,
        "cells": [
            { "text": "数据库索引" },
            { "text": "查询性能优化" },
            { "text": "分片策略" },
            { "text": "缓存命中率" }
        ]
    }))
    .execute(&db)
    .await
    .unwrap();

    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE document_id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '索引')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(hits, 1, "cells 文本未进入检索索引");

    // 另一个单元格也要能命中，证明整列都被抽取而非只有第一个。
    let hits2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE document_id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '缓存')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(hits2, 1, "只有第一个单元格被抽取");
}

#[sqlx::test]
async fn 单元格缺text字段时不产生脏词(db: PgPool) {
    let owner = common::user(&db, "cellnull@example.com").await;
    let space = common::space(&db, "cellnull-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "空单元格", None).await;

    // 混合：有的单元格没有 text 键，cells 也可能是非数组。
    sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, 'table', $3, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(doc)
    .bind(json!({ "rows": 1, "cols": 2, "cells": [{ "colspan": 2 }, { "text": "有效内容" }] }))
    .execute(&db)
    .await
    .unwrap();

    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE document_id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', '有效')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(hits, 1);

    // 结构键 colspan 不得成为检索词。
    let tsv: String =
        sqlx::query_scalar("SELECT content_tsv::text FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(!tsv.contains("colspan"), "结构键泄漏进索引: {tsv}");
}

#[sqlx::test]
async fn 嵌入块的url可被检索命中(db: PgPool) {
    let owner = common::user(&db, "embed@example.com").await;
    let space = common::space(&db, "embed-space", owner, Visibility::Private).await;
    let doc = common::document(&db, space, "嵌入", None).await;

    sqlx::query(
        "INSERT INTO document_chunks (id, document_id, type, content, position) \
         VALUES ($1, $2, 'embed', $3, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(doc)
    .bind(json!({ "url": "https://example.com/knowledgebase", "type": "iframe" }))
    .execute(&db)
    .await
    .unwrap();

    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_chunks \
         WHERE document_id = $1 AND content_tsv @@ to_tsquery('zhparser_cfg', 'knowledgebase')",
    )
    .bind(doc)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(hits, 1, "embed.url 未进入检索索引");
}

// ============================================================
// user_tokens 与 pending 状态（v1.2 用户管理）
// ============================================================

#[sqlx::test]
async fn pending状态被check接受(db: PgPool) {
    // chk_users_status 接受 pending/active/banned。
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, status, username) \
         VALUES ($1, 'pending@example.com', 'h', 'pending', 'pendinguser')",
    )
    .bind(Uuid::now_v7())
    .execute(&db)
    .await
    .expect("pending 应被 chk_users_status 接受");
}

#[sqlx::test]
async fn 同用户同类型第二条令牌被唯一索引拒绝(db: PgPool) {
    let user = common::user(&db, "token@example.com").await;
    sqlx::query(
        "INSERT INTO user_tokens (id, user_id, type, token_hash, expires_at) \
         VALUES ($1, $2, 'email_verify', 'hash-a', now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .execute(&db)
    .await
    .unwrap();

    // 同用户同类型至多一条，重发靠 upsert 覆盖而非插入第二条。
    let err = sqlx::query(
        "INSERT INTO user_tokens (id, user_id, type, token_hash, expires_at) \
         VALUES ($1, $2, 'email_verify', 'hash-b', now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .execute(&db)
    .await
    .expect_err("同用户同类型第二条令牌应被拒绝");
    assert_eq!(constraint_of(&err), "uniq_user_tokens_user_type");
}

#[sqlx::test]
async fn 令牌哈希重复被唯一约束拒绝(db: PgPool) {
    let alice = common::user(&db, "alice@example.com").await;
    let bob = common::user(&db, "bob@example.com").await;
    sqlx::query(
        "INSERT INTO user_tokens (id, user_id, type, token_hash, expires_at) \
         VALUES ($1, $2, 'email_verify', 'same-hash', now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(alice)
    .execute(&db)
    .await
    .unwrap();

    // token_hash 全局唯一：不同用户、不同类型也不行。
    let err = sqlx::query(
        "INSERT INTO user_tokens (id, user_id, type, token_hash, expires_at) \
         VALUES ($1, $2, 'password_reset', 'same-hash', now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(bob)
    .execute(&db)
    .await
    .expect_err("重复 token_hash 应被拒绝");
    assert_eq!(constraint_of(&err), "user_tokens_token_hash_key");
}

#[sqlx::test]
async fn 删除用户级联删除其用户令牌(db: PgPool) {
    let user = common::user(&db, "cascade@example.com").await;
    sqlx::query(
        "INSERT INTO user_tokens (id, user_id, type, token_hash, expires_at) \
         VALUES ($1, $2, 'email_verify', 'hash-c', now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(user)
    .execute(&db)
    .await
    .unwrap();

    // 物理删除（pending 用户超期清理走此路径），令牌随 CASCADE 清理。
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(&db)
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tokens WHERE user_id = $1")
        .bind(user)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

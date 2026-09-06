//! PATCH 分片操作边界加固测试。
//!
//! 测试场景：
//! 1. insert position 超上界（如 100000）返回 400/1001
//! 2. insert position = 当前分片数可正常追加到末尾
//! 3. insert position < 0 返回 400/1001
//! 4. operations 条数超限（> 200）返回 400/1001
//! 5. 空 operations 返回 400/1001

use knowledgebase::{
    models::Visibility,
    repository::chunk as chunk_repo,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

mod common;

/// 测试：insert position 超上界返回 BadRequest
#[sqlx::test]
async fn insert_position超上界被拒绝(db: PgPool) {
    let user_id = common::user(&db, "alice", "alice@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 创建 3 个分片（position 0, 1, 2）
    let mut tx = db.begin().await.unwrap();
    for i in 0..3 {
        chunk_repo::insert_at(
            &mut tx,
            doc_id,
            i,
            knowledgebase::models::ChunkType::Paragraph,
            &json!({"text": format!("段落 {}", i)}),
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    // 尝试在 position=100000 插入（超上界）
    let mut tx = db.begin().await.unwrap();
    let err = chunk_repo::insert_at(
        &mut tx,
        doc_id,
        100000,
        knowledgebase::models::ChunkType::Paragraph,
        &json!({"text": "非法插入"}),
    )
    .await
    .unwrap_err();

    // 验证错误类型
    assert!(
        matches!(err, knowledgebase::error::Error::BadRequest(_)),
        "期望 BadRequest，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("超出有效范围"));

    // 验证数据库未被破坏（position 仍为 0, 1, 2）
    let positions = sqlx::query_scalar!("SELECT position FROM document_chunks WHERE document_id = $1 ORDER BY position", doc_id)
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(positions, vec![0, 1, 2]);
}

/// 测试：position = 当前分片数可正常追加到末尾
#[sqlx::test]
async fn insert_追加到末尾合法(db: PgPool) {
    let user_id = common::user(&db, "bob", "bob@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 创建 3 个分片（position 0, 1, 2）
    let mut tx = db.begin().await.unwrap();
    for i in 0..3 {
        chunk_repo::insert_at(
            &mut tx,
            doc_id,
            i,
            knowledgebase::models::ChunkType::Paragraph,
            &json!({"text": format!("段落 {}", i)}),
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    // 在 position=3 插入（追加到末尾）
    let mut tx = db.begin().await.unwrap();
    chunk_repo::insert_at(
        &mut tx,
        doc_id,
        3,
        knowledgebase::models::ChunkType::Paragraph,
        &json!({"text": "追加分片"}),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // 验证 position 为 0, 1, 2, 3
    let positions = sqlx::query_scalar!("SELECT position FROM document_chunks WHERE document_id = $1 ORDER BY position", doc_id)
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(positions, vec![0, 1, 2, 3]);
}

/// 测试：position < 0 返回 BadRequest
#[sqlx::test]
async fn insert_负position被拒绝(db: PgPool) {
    let user_id = common::user(&db, "carol", "carol@e.com", "password123").await;
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 尝试插入负 position
    let mut tx = db.begin().await.unwrap();
    let err = chunk_repo::insert_at(
        &mut tx,
        doc_id,
        -1,
        knowledgebase::models::ChunkType::Paragraph,
        &json!({"text": "非法插入"}),
    )
    .await
    .unwrap_err();

    // 验证错误类型
    assert!(
        matches!(err, knowledgebase::error::Error::BadRequest(_)),
        "期望 BadRequest，实际 {:?}",
        err
    );
    assert!(err.to_string().contains("不得为负"));
}

/// 测试：operations 条数超限返回 400/1001（API 层测试）
#[sqlx::test]
async fn operations条数超限被拒绝(db: PgPool) {
    use axum::{Router, body::{Body, to_bytes}, http::{Request, StatusCode, header}};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    const TEST_SECRET: &str = "test-secret-do-not-use-in-prod";

    fn app(db: PgPool) -> Router {
        knowledgebase::api::router(knowledgebase::api::AppState::new(db, TEST_SECRET.to_owned(), 7 * 24 * 3600))
    }

    let router = app(db.clone());

    // 注册并登录
    let register_resp = router
        .clone()
        .oneshot(
            Request::post("/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": "dave",
                        "email": "dave@e.com",
                        "password": "password123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(register_resp.into_body(), 1024 * 1024).await.unwrap();
    let register_body: Value = serde_json::from_slice(&bytes).unwrap();
    let token = register_body["data"]["token"].as_str().unwrap();
    let user_id: Uuid = register_body["data"]["user"]["id"].as_str().unwrap().parse().unwrap();

    // 创建空间和文档
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 构造 201 个 operations（超过上限 200）
    let operations: Vec<_> = (0..201)
        .map(|i| {
            json!({
                "type": "insert",
                "position": i,
                "chunk": {
                    "type": "paragraph",
                    "content": {"text": format!("分片 {}", i)}
                }
            })
        })
        .collect();

    // 发送 PATCH 请求
    let patch_resp = router
        .oneshot(
            Request::patch(format!("/documents/{}/content", doc_id))
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"operations": operations}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    // 验证返回 400/1001
    assert_eq!(patch_resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(patch_resp.into_body(), 1024 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], 1001);
    assert!(body["message"].as_str().unwrap().contains("超限"));
    assert!(body["message"].as_str().unwrap().contains("200"));
}

/// 测试：空 operations 返回 400/1001
#[sqlx::test]
async fn 空operations被拒绝(db: PgPool) {
    use axum::{Router, body::{Body, to_bytes}, http::{Request, StatusCode, header}};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    const TEST_SECRET: &str = "test-secret-do-not-use-in-prod";

    fn app(db: PgPool) -> Router {
        knowledgebase::api::router(knowledgebase::api::AppState::new(db, TEST_SECRET.to_owned(), 7 * 24 * 3600))
    }

    let router = app(db.clone());

    // 注册并登录
    let register_resp = router
        .clone()
        .oneshot(
            Request::post("/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": "eve",
                        "email": "eve@e.com",
                        "password": "password123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(register_resp.into_body(), 1024 * 1024).await.unwrap();
    let register_body: Value = serde_json::from_slice(&bytes).unwrap();
    let token = register_body["data"]["token"].as_str().unwrap();
    let user_id: Uuid = register_body["data"]["user"]["id"].as_str().unwrap().parse().unwrap();

    // 创建空间和文档
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 发送空 operations
    let patch_resp = router
        .oneshot(
            Request::patch(format!("/documents/{}/content", doc_id))
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"operations": []}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    // 验证返回 400/1001
    assert_eq!(patch_resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(patch_resp.into_body(), 1024 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], 1001);
    assert!(body["message"].as_str().unwrap().contains("不能为空"));
}

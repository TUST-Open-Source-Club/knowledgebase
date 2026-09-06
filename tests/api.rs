//! HTTP API 层集成测试。
//!
//! 与 repository 层测试互补：这里验证的是**契约**——路由、统一响应壳
//! （code/message/data）、业务错误码、JWT 鉴权、operations 事务语义。
//! 用 tower::ServiceExt::oneshot 在内存中打完整请求，不起端口。

mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use knowledgebase::{api, models::Visibility};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::util::ServiceExt;
use uuid::Uuid;

const TEST_SECRET: &str = "test-secret-do-not-use-in-prod";

fn app(db: PgPool) -> Router {
    api::router(api::AppState::new(db, TEST_SECRET.to_owned(), 7 * 24 * 3600))
}

async fn post_json(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("请求应得到响应");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get_auth(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::get(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("请求应得到响应");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn patch_json_auth(app: &Router, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::patch(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("请求应得到响应");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// 注册一个用户，返回 (user_id, token)。
async fn register_user(app: &Router, username: &str, email: &str) -> (Uuid, String) {
    let (status, body) = post_json(
        app,
        "/auth/register",
        json!({ "username": username, "email": email, "password": "password-123" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "注册失败: {body}");
    let id = Uuid::parse_str(body["data"]["user"]["id"].as_str().unwrap()).unwrap();
    let token = body["data"]["token"].as_str().unwrap().to_owned();
    (id, token)
}

// ============================================================
// 注册
// ============================================================

#[sqlx::test]
async fn 注册成功返回用户与token(db: PgPool) {
    let app = app(db);
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({ "username": "alice", "email": "alice@e.com", "password": "password-123" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    assert_eq!(body["message"], "success");
    assert_eq!(body["data"]["user"]["username"], "alice");
    assert_eq!(body["data"]["user"]["email"], "alice@e.com");
    // 对外视图不得泄漏内部字段。
    assert!(body["data"]["user"].get("password_hash").is_none());
    assert!(body["data"]["user"].get("status").is_none());
    assert!(body["data"]["user"].get("role").is_none());
    assert!(body["data"]["token"].as_str().unwrap().len() > 20);
}

#[sqlx::test]
async fn 注册用户名过短返回1001(db: PgPool) {
    let app = app(db);
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({ "username": "ab", "email": "ab@e.com", "password": "password-123" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
}

#[sqlx::test]
async fn 注册密码过短返回1001(db: PgPool) {
    let app = app(db);
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({ "username": "alice", "email": "alice@e.com", "password": "short" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
}

#[sqlx::test]
async fn 注册邮箱格式非法返回1001(db: PgPool) {
    let app = app(db);
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({ "username": "alice", "email": "not-an-email", "password": "password-123" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
}

#[sqlx::test]
async fn 重复用户名返回1005(db: PgPool) {
    let app = app(db);
    register_user(&app, "alice", "alice@e.com").await;
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({ "username": "alice", "email": "alice2@e.com", "password": "password-123" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], 1005);
    assert_eq!(body["message"], "用户名已被占用");
}

#[sqlx::test]
async fn 重复邮箱返回1005(db: PgPool) {
    let app = app(db);
    register_user(&app, "alice", "alice@e.com").await;
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({ "username": "bob", "email": "alice@e.com", "password": "password-123" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], 1005);
    assert_eq!(body["message"], "邮箱已被注册");
}

// ============================================================
// 登录
// ============================================================

#[sqlx::test]
async fn 用户名和邮箱都能登录(db: PgPool) {
    let app = app(db);
    register_user(&app, "alice", "alice@e.com").await;

    for account in ["alice", "alice@e.com"] {
        let (status, body) = post_json(
            &app,
            "/auth/login",
            json!({ "account": account, "password": "password-123" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "account={account}: {body}");
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["user"]["username"], "alice");
    }
}

#[sqlx::test]
async fn 密码错误与用户不存在返回同一文案(db: PgPool) {
    let app = app(db);
    register_user(&app, "alice", "alice@e.com").await;

    for account in ["alice", "no-such-user"] {
        let (status, body) = post_json(
            &app,
            "/auth/login",
            json!({ "account": account, "password": "wrong-password" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "account={account}");
        assert_eq!(body["code"], 2001, "account={account}");
        assert_eq!(body["message"], "用户名或密码错误", "account={account}");
    }
}

#[sqlx::test]
async fn 被封禁用户登录返回统一文案(db: PgPool) {
    let app = app(db.clone());
    let (id, _) = register_user(&app, "alice", "alice@e.com").await;
    sqlx::query("UPDATE users SET status = 'banned' WHERE id = $1")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();

    let (status, body) = post_json(
        &app,
        "/auth/login",
        json!({ "account": "alice", "password": "password-123" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 2001);
    assert_eq!(body["message"], "用户名或密码错误");
}

#[sqlx::test]
async fn 登录缺字段返回1001(db: PgPool) {
    let app = app(db);
    let (status, body) = post_json(&app, "/auth/login", json!({ "account": "", "password": "" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
}

// ============================================================
// 鉴权与 users/me
// ============================================================

#[sqlx::test]
async fn 无token访问受保护接口返回1002(db: PgPool) {
    let resp = app(db)
        .oneshot(Request::get("/users/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], 1002);
}

#[sqlx::test]
async fn 伪造token返回1002(db: PgPool) {
    let (status, body) = get_auth(&app(db), "/users/me", "not.a.token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 1002);
}

#[sqlx::test]
async fn me返回当前用户(db: PgPool) {
    let app = app(db);
    let (id, token) = register_user(&app, "alice", "alice@e.com").await;

    let (status, body) = get_auth(&app, "/users/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["user"]["id"], id.to_string());
    assert_eq!(body["data"]["user"]["username"], "alice");
}

#[sqlx::test]
async fn logout带token返回成功(db: PgPool) {
    let app = app(db);
    let (_, token) = register_user(&app, "alice", "alice@e.com").await;

    let resp = app
        .oneshot(
            Request::post("/auth/logout")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[sqlx::test]
async fn logout不带token返回1002(db: PgPool) {
    let resp = app(db)
        .oneshot(Request::post("/auth/logout").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// 文档内容
// ============================================================

/// 建「owner + 私有空间 + 文档 + 两个分片」的最小场景。
async fn doc_scene(db: &PgPool, app: &Router, username: &str) -> (String, Uuid, Uuid, Uuid) {
    let (owner_id, token) = register_user(app, username, &format!("{username}@e.com")).await;
    let space = common::space(db, username, owner_id, Visibility::Private).await;
    let doc = common::document(db, space, "文档", None).await;
    let c1 = common::paragraph(db, doc, "第一段", 0).await;
    let c2 = common::paragraph(db, doc, "第二段", 1).await;
    (token, doc, c1, c2)
}

#[sqlx::test]
async fn 读文档返回全量分片(db: PgPool) {
    let app = app(db.clone());
    let (token, doc, c1, c2) = doc_scene(&db, &app, "owner").await;

    let (status, body) = get_auth(&app, &format!("/documents/{doc}/content"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["document_id"], doc.to_string());
    let chunks = body["data"]["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0]["id"], c1.to_string());
    assert_eq!(chunks[0]["position"], 0);
    assert_eq!(chunks[0]["content"]["text"], "第一段");
    assert_eq!(chunks[1]["id"], c2.to_string());
}

#[sqlx::test]
async fn 无权限用户读文档返回404不泄漏存在性(db: PgPool) {
    let app = app(db.clone());
    let (_, doc, _, _) = doc_scene(&db, &app, "owner").await;
    let (_, stranger_token) = register_user(&app, "stranger", "stranger@e.com").await;

    let (status, body) = get_auth(&app, &format!("/documents/{doc}/content"), &stranger_token).await;
    // 私有空间文档对外人不可见：404 而非 403，不泄漏文档存在。
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], 1004);
}

#[sqlx::test]
async fn patch单事务应用update_insert_delete(db: PgPool) {
    let app = app(db.clone());
    let (token, doc, c1, c2) = doc_scene(&db, &app, "owner").await;

    let (status, body) = patch_json_auth(
        &app,
        &format!("/documents/{doc}/content"),
        &token,
        json!({
            "operations": [
                { "op": "update", "chunk_id": c1, "content": { "text": "改过的第一段" } },
                { "op": "insert", "position": 1, "type": "paragraph", "content": { "text": "插进来的" } },
                { "op": "delete", "chunk_id": c2 }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["updated"], true);

    // 最终应为：[改过的第一段(0), 插进来的(1)]，位置连续。
    assert_eq!(common::positions(&db, doc).await, vec![0, 1]);
    let (status, body) = get_auth(&app, &format!("/documents/{doc}/content"), &token).await;
    assert_eq!(status, StatusCode::OK);
    let chunks = body["data"]["chunks"].as_array().unwrap();
    assert_eq!(chunks[0]["content"]["text"], "改过的第一段");
    assert_eq!(chunks[1]["content"]["text"], "插进来的");
}

#[sqlx::test]
async fn patch的update不传type时保留原类型(db: PgPool) {
    let app = app(db.clone());
    let (token, doc, c1, _) = doc_scene(&db, &app, "owner").await;

    let (status, _) = patch_json_auth(
        &app,
        &format!("/documents/{doc}/content"),
        &token,
        json!({ "operations": [ { "op": "update", "chunk_id": c1, "content": { "text": "x" } } ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = get_auth(&app, &format!("/documents/{doc}/content"), &token).await;
    assert_eq!(body["data"]["chunks"][0]["type"], "paragraph");
}

#[sqlx::test]
async fn patch空operations返回1001(db: PgPool) {
    let app = app(db.clone());
    let (token, doc, _, _) = doc_scene(&db, &app, "owner").await;

    let (status, body) = patch_json_auth(
        &app,
        &format!("/documents/{doc}/content"),
        &token,
        json!({ "operations": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
}

#[sqlx::test]
async fn patch操作其他文档的分片返回404且整体回滚(db: PgPool) {
    let app = app(db.clone());
    let (token, doc, c1, _) = doc_scene(&db, &app, "owner").await;
    // 第二个文档（同 owner），其分片不属于第一个文档。
    let owner_doc2 = common::document(&db, {
        sqlx::query_scalar!("SELECT space_id FROM documents WHERE id = $1", doc)
            .fetch_one(&db)
            .await
            .unwrap()
    }, "另一文档", None).await;
    let foreign_chunk = common::paragraph(&db, owner_doc2, "别家分片", 0).await;

    let (status, body) = patch_json_auth(
        &app,
        &format!("/documents/{doc}/content"),
        &token,
        json!({
            "operations": [
                { "op": "update", "chunk_id": c1, "content": { "text": "不应落库" } },
                { "op": "delete", "chunk_id": foreign_chunk }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], 1004);

    // 事务回滚：第一个分片内容未被改动。
    let (_, body) = get_auth(&app, &format!("/documents/{doc}/content"), &token).await;
    assert_eq!(body["data"]["chunks"][0]["content"]["text"], "第一段");
}

#[sqlx::test]
async fn 无写权限用户patch返回404(db: PgPool) {
    let app = app(db.clone());
    let (_, doc, c1, _) = doc_scene(&db, &app, "owner").await;
    let (_, stranger_token) = register_user(&app, "stranger", "stranger@e.com").await;

    let (status, _) = patch_json_auth(
        &app,
        &format!("/documents/{doc}/content"),
        &stranger_token,
        json!({ "operations": [ { "op": "update", "chunk_id": c1, "content": { "text": "x" } } ] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn 空间公开后外人可读但不可写(db: PgPool) {
    let app = app(db.clone());
    let (owner_id, _) = register_user(&app, "owner", "owner@e.com").await;
    let space = common::space(&db, "pub", owner_id, Visibility::Public).await;
    let doc = common::document(&db, space, "公开文档", None).await;
    common::paragraph(&db, doc, "大家好", 0).await;
    let (_, stranger_token) = register_user(&app, "stranger", "stranger@e.com").await;

    let (status, body) = get_auth(&app, &format!("/documents/{doc}/content"), &stranger_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["chunks"][0]["content"]["text"], "大家好");
}

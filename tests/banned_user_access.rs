//! 失权即时生效测试：banned/软删用户的已签发 JWT 立即失效。
//!
//! 测试场景：
//! 1. 用户登录拿到 token → 被封禁 → 旧 token 访问受保护接口返回 401/1002
//! 2. 用户登录拿到 token → 自助注销（软删除）→ 旧 token 返回 401/1002
//! 3. Active/Pending 用户行为不变

mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use knowledgebase::{api, models::Visibility};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
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

#[sqlx::test]
async fn banned用户的旧token访问文档被拒绝(db: PgPool) {
    let app = app(db.clone());

    // 1. 注册并激活用户
    let (status, body) = post_json(&app, "/auth/register",
        json!({"username": "alice", "email": "alice@e.com", "password": "password123"})).await;
    assert_eq!(status, StatusCode::OK);
    let user_id: Uuid = body["data"]["user"]["id"].as_str().unwrap().parse().unwrap();

    // 激活用户（pending → active）
    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await
        .unwrap();

    // 2. 登录获取 JWT token
    let (status, body) = post_json(&app, "/auth/login",
        json!({"account": "alice", "password": "password123"})).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["data"]["token"].as_str().unwrap();

    // 3. 创建测试空间和文档
    let space_id = common::space(&db, "test-space", user_id, Visibility::Private).await;
    let doc_id = common::document(&db, space_id, "Test Doc", None).await;

    // 4. 验证 token 当前可用（访问 /users/me）
    let (status, body) = get_auth(&app, "/users/me", token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["user"]["username"], "alice");

    // 5. 封禁用户
    sqlx::query("UPDATE users SET status = 'banned' WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await
        .unwrap();

    // 6. 旧 token 访问 /users/me 应该返回 401/1002
    let (status, body) = get_auth(&app, "/users/me", token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 1002);
    assert!(body["message"].as_str().unwrap().contains("失效"));

    // 7. 旧 token 访问文档内容应该返回 401/1002
    let (status, body) = get_auth(&app, &format!("/documents/{}/content", doc_id), token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 1002);
}

#[sqlx::test]
async fn 软删除用户的旧token访问被拒绝(db: PgPool) {
    let app = app(db.clone());

    // 1. 注册并激活用户
    let (status, body) = post_json(&app, "/auth/register",
        json!({"username": "bob", "email": "bob@e.com", "password": "password123"})).await;
    assert_eq!(status, StatusCode::OK);
    let user_id: Uuid = body["data"]["user"]["id"].as_str().unwrap().parse().unwrap();

    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await
        .unwrap();

    // 2. 登录获取 token
    let (status, body) = post_json(&app, "/auth/login",
        json!({"account": "bob", "password": "password123"})).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["data"]["token"].as_str().unwrap();

    // 3. 验证 token 当前可用
    let (status, body) = get_auth(&app, "/users/me", token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["user"]["username"], "bob");

    // 4. 软删除用户（模拟自助注销）
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await
        .unwrap();

    // 5. 旧 token 访问 /users/me 应该返回 401/1002
    let (status, body) = get_auth(&app, "/users/me", token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 1002);
}

#[sqlx::test]
async fn active用户行为不受影响(db: PgPool) {
    let app = app(db.clone());

    // 1. 注册并激活用户
    let (status, body) = post_json(&app, "/auth/register",
        json!({"username": "carol", "email": "carol@e.com", "password": "password123"})).await;
    assert_eq!(status, StatusCode::OK);
    let user_id: Uuid = body["data"]["user"]["id"].as_str().unwrap().parse().unwrap();

    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await
        .unwrap();

    // 2. 登录获取 token
    let (status, body) = post_json(&app, "/auth/login",
        json!({"account": "carol", "password": "password123"})).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["data"]["token"].as_str().unwrap();

    // 3. Active 用户访问 /users/me 应该成功
    let (status, body) = get_auth(&app, "/users/me", token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["user"]["username"], "carol");
}

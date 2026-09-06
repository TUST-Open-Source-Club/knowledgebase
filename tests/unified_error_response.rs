//! HTTP 层错误统一进响应壳测试。
//!
//! 测试场景：
//! 1. JSON 解析失败返回统一格式 400/1001
//! 2. UUID 路径参数格式非法返回 400/1001
//! 3. 未匹配路由返回 404/1004
//! 4. 业务层错误（如权限不足）正常进响应壳

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use knowledgebase::api;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

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

async fn get_with_token(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
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

async fn post_raw(app: &Router, uri: &str, body: &str) -> (StatusCode, Value) {
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

async fn get_raw(app: &Router, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .expect("请求应得到响应");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// 测试：JSON 解析失败返回统一格式 400/1001
#[sqlx::test]
async fn json解析失败返回统一格式(db: PgPool) {
    let app = app(db);

    // 发送非法 JSON
    let (status, body) = post_raw(&app, "/auth/register", "{invalid json}").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
    assert!(body["message"].as_str().unwrap().contains("JSON"));
    assert_eq!(body["data"], Value::Null);
}

/// 测试：UUID 路径参数格式非法返回 400/1001
#[sqlx::test]
async fn uuid路径参数非法返回统一格式(db: PgPool) {
    let app = app(db);

    // 注册并登录获取 token
    let (status, body) = post_json(
        &app,
        "/auth/register",
        json!({"username": "test", "email": "test@e.com", "password": "password123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["data"]["token"].as_str().unwrap();

    // 使用非法 UUID 访问文档
    let (status, body) = get_with_token(&app, "/documents/not-a-uuid/content", token).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 1001);
    assert!(body["message"].as_str().unwrap().contains("UUID") ||
            body["message"].as_str().unwrap().contains("参数"));
    assert_eq!(body["data"], Value::Null);
}

/// 测试：未匹配路由返回 404/1004
#[sqlx::test]
async fn 未匹配路由返回404(db: PgPool) {
    let app = app(db);

    let (status, body) = get_raw(&app, "/nonexistent/route").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], 1004);
    assert!(body["message"].as_str().unwrap().contains("不存在") ||
            body["message"].as_str().unwrap().contains("未找到"));
    assert_eq!(body["data"], Value::Null);
}

/// 测试：业务层错误正常进响应壳（登录失败示例）
#[sqlx::test]
async fn 业务错误正常进响应壳(db: PgPool) {
    let app = app(db);

    // 登录失败：密码错误（返回 2001 而非 1002，后者用于 JWT 鉴权失败）
    let (status, body) = post_json(
        &app,
        "/auth/login",
        json!({"account": "nonexistent", "password": "wrong"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 2001);  // 登录失败 code
    assert!(body["message"].as_str().unwrap().contains("用户名或密码"));
    assert_eq!(body["data"], Value::Null);
}

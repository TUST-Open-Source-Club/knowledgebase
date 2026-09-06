//! 服务入口。
//!
//! 装配顺序：配置 → 连接池 → 迁移 → 路由。路由分两层：
//! - `/health`：探活与数据库连通性，无鉴权、无业务数据。
//! - 业务路由（[`api::router]`]）：除 `/auth/register`、`/auth/login`
//!   外均由 `AuthUser` extractor 鉴权。
//!
//! CORS：开发环境全量放开；生产环境应收紧 `allow_origin`。

use std::net::SocketAddr;

use axum::{Json, Router, http::StatusCode, routing::get};
use knowledgebase::{Config, Db, api, db};
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "knowledgebase=debug,sqlx=warn".into()),
        )
        .init();

    let config = Config::from_env()?;
    tracing::info!(bind = %config.bind_addr, "启动中");

    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    tracing::info!("迁移已应用至最新版本");

    let state = api::AppState::new(
        pool.clone(),
        config.jwt_secret.clone(),
        config.jwt_ttl.as_secs(),
    );

    // 开发环境 CORS 全开；生产应收紧 allow_origin。
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health).with_state(pool.clone()))
        .merge(api::router(state))
        .layer(cors);

    let addr: SocketAddr = config.bind_addr.parse()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "开始监听");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn health(
    axum::extract::State(pool): axum::extract::State<Db>,
) -> (StatusCode, Json<serde_json::Value>) {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "status": "ok", "database": "up" })),
        ),
        Err(err) => {
            tracing::error!(error = %err, "数据库探活失败");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded", "database": "down" })),
            )
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("收到中断信号，正在关闭");
}

//! `knowledgebase-server`：类语雀知识库的后端 API 服务骨架。
//!
//! 当前版本提供工程骨架（配置、数据库连接、错误模型、路由与健康检查），
//! 业务模块（认证、空间、文档、版本）按 `docs/architecture.md` 的路线图逐步补齐。

pub mod app;
pub mod config;
pub mod db;
pub mod entities;
pub mod error;
pub mod routes;
pub mod state;
pub mod telemetry;
pub mod utils;

use anyhow::Context;

use crate::{config::AppConfig, state::AppState};

/// 启动服务：初始化日志 → 连接数据库 → 执行迁移 → 监听 HTTP。
pub async fn run(config: AppConfig) -> anyhow::Result<()> {
    telemetry::init(&config.log_filter);

    tracing::info!(
        host = %config.host,
        port = config.port,
        database = redact_database_url(&config.database_url),
        run_migrations = config.run_migrations,
        "starting knowledgebase-server"
    );

    let db = db::connect(&config.database_url)
        .await
        .context("连接数据库失败，请检查 DATABASE_URL")?;

    if config.run_migrations {
        db::migrate(&db).await.context("执行数据库迁移失败")?;
        tracing::info!("database migrations applied");
    }

    let state = AppState::new(db, config.clone());
    let router = app::build_router(state);
    let addr = config.bind_addr();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("监听地址 {addr} 失败"))?;

    tracing::info!(%addr, "listening on http://{addr}");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP 服务异常退出")?;

    Ok(())
}

/// 隐藏连接串里的密码，避免写进日志。
fn redact_database_url(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            let scheme = &url[..scheme_end + 3];
            format!("{scheme}***@{}", &url[at + 1..])
        }
        _ => url.to_owned(),
    }
}

/// 等待 Ctrl+C 或 SIGTERM，用于优雅停机。
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
        std::future::pending::<()>().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, shutting down gracefully");
}

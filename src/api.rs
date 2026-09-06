//! HTTP API 层。
//!
//! 本模块把 repository 层暴露为 REST 接口：
//!
//! | 路由 | 鉴权 | 说明 |
//! |------|------|------|
//! | `POST /auth/register` | 无 | 注册（pending），按「注册即登录」直接签发 JWT |
//! | `POST /auth/login` | 无 | 登录（username 或 email + 密码） |
//! | `POST /auth/logout` | Bearer | 无状态 JWT，服务端无操作，返回成功 |
//! | `GET  /users/me` | Bearer | 当前用户信息 |
//! | `GET  /documents/{id}/content` | Bearer | 全量分片（需读权限） |
//! | `PATCH /documents/{id}/content` | Bearer | 增量 operations（需写权限） |
//!
//! 设计要点：
//! - 统一响应壳 [`ApiResponse<T>`]（code/message/data），业务错误映射到
//!   HTTP 状态 + 业务 code（1001–1005 / 2001 / 5000）。
//! - JWT 为 HS256，载荷 `sub`(user_id)/`username`/`exp`/`iat`；
//!   鉴权通过 [`AuthUser`] extractor 完成，handler 签名里出现它即受保护。
//! - PATCH 的 `operations` 是强类型 [`Operation`] 枚举（含 update/insert/delete），
//!   一个事务内按序执行，全部成功才提交。
//! - 文档权限走现有六级判定（[`repository::permission`]）：GET 需
//!   `allows_read`，PATCH 需 `allows_write`；不可见按 404 处理，
//!   不泄漏文档存在性。

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::{FromRef, FromRequestParts, Path, State, Request},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::{
    db::{Db, Tx},
    error::{Error, Identity},
    models::{ChunkType, DocumentChunk, User},
    repository::{chunk as chunk_repo, permission as permission_repo, user as user_repo},
    repository::user::PasswordHasher,
};

/// 密码长度下限（≥ 8 字符）。
const MIN_PASSWORD_LEN: usize = 8;
/// username 长度约束（3–32 字符）。
const MIN_USERNAME_LEN: usize = 3;
const MAX_USERNAME_LEN: usize = 32;

// ============================================================
// 统一响应壳
// ============================================================

/// 统一响应结构。`code = 0` 为成功，`message` 固定 `"success"`。
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            code: 0,
            message: "success".to_owned(),
            data: Some(data),
        }
    }
}

/// 业务错误：HTTP 状态 + 业务 code + 文案，三者一体。
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: i32,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: i32, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, 1001, message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, 1002, message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, 1003, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, 1004, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, 1005, message)
    }

    /// 登录失败统一文案（不区分用户是否存在，防账号枚举）。
    fn bad_credentials() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, 2001, "用户名或密码错误")
    }

    fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, 5000, "服务器内部错误")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "code": self.code,
            "message": self.message,
            "data": null,
        }));
        (self.status, body).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        match err {
            Error::NotFound { resource, .. } => {
                ApiError::not_found(format!("{} 不存在", resource.as_str()))
            }
            Error::Conflict { identity } => ApiError::conflict(match identity {
                Identity::UserEmail => "邮箱已被注册".to_owned(),
                Identity::Username => "用户名已被占用".to_owned(),
                other => format!("{} 已被占用", other.as_str()),
            }),
            Error::RestoreConflict { identity, .. } => {
                ApiError::conflict(format!("{} 已被活跃记录占用", identity.as_str()))
            }
            Error::ForeignKey { .. } | Error::Check { .. } => {
                ApiError::bad_request("请求违反数据约束")
            }
            Error::InvalidToken => ApiError::bad_request("链接无效"),
            Error::ExpiredToken => ApiError::bad_request("链接已过期，请重新获取"),
            Error::WrongPassword => ApiError::bad_credentials(),
            Error::Unauthorized(message) => ApiError::unauthorized(message),
            Error::Forbidden(message) => ApiError::forbidden(message),
            Error::Invariant(message) => ApiError::forbidden(message),
            Error::BadRequest(message) => ApiError::bad_request(message),
            Error::Database(_) | Error::Migrate(_) => {
                tracing::error!(error = ?err, "内部错误");
                ApiError::internal()
            }
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        ApiError::from(Error::from(err))
    }
}

// ============================================================
// 自定义 extractors（把 axum rejection 统一进响应壳）
// ============================================================

use axum::extract::FromRequest;

/// JSON extractor 包装，将 axum rejection 转为 ApiError。
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => {
                tracing::debug!(error = ?rejection, "JSON 解析失败");
                Err(ApiError::bad_request("请求 JSON 格式错误"))
            }
        }
    }
}

/// Path extractor 包装，将 axum rejection 转为 ApiError。
/// 注意：UUID 解析失败返回 400/1001（而非 404）。
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(ApiPath(value)),
            Err(rejection) => {
                tracing::debug!(error = ?rejection, "路径参数解析失败");
                Err(ApiError::bad_request("路径参数格式错误"))
            }
        }
    }
}

// ============================================================
// JWT
// ============================================================

/// JWT 载荷：sub(user_id) / username / exp / iat。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub username: String,
    pub exp: u64,
    pub iat: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================
// 应用状态
// ============================================================

/// 共享给所有 handler 的状态。
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub hasher: PasswordHasher,
    jwt_secret: String,
    jwt_ttl_secs: u64,
}

impl AppState {
    pub fn new(db: Db, jwt_secret: String, jwt_ttl_secs: u64) -> Self {
        Self {
            db,
            hasher: PasswordHasher::new(),
            jwt_secret,
            jwt_ttl_secs,
        }
    }

    fn issue_token(&self, user: &User) -> Result<String, ApiError> {
        let iat = now_unix();
        let claims = Claims {
            sub: user.id,
            username: user.username.clone(),
            exp: iat + self.jwt_ttl_secs,
            iat,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .map_err(|err| {
            tracing::error!(error = ?err, "签发 JWT 失败");
            ApiError::internal()
        })
    }
}

impl FromRef<AppState> for Db {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

// ============================================================
// 鉴权 extractor
// ============================================================

/// 受保护 handler 的鉴权入口：从 `Authorization: Bearer <token>` 解出
/// 当前用户 id。解不出即 401（code 1002）。
pub struct AuthUser {
    pub user_id: Uuid,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)
            .ok_or_else(|| ApiError::unauthorized("未提供认证令牌"))?;

        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|err| {
            tracing::debug!(error = ?err, "JWT 校验失败");
            ApiError::unauthorized("令牌无效或已过期")
        })?;

        // 校验用户当前状态：banned/软删用户的 JWT 立即失效
        let user = sqlx::query!(
            r#"
            SELECT status AS "status: crate::models::UserStatus", deleted_at
            FROM users
            WHERE id = $1
            "#,
            data.claims.sub
        )
        .fetch_optional(&state.db)
        .await
        .map_err(|err| {
            tracing::error!(error = ?err, "查询用户状态失败");
            ApiError::internal()
        })?
        .ok_or_else(|| ApiError::unauthorized("令牌已失效"))?;

        use crate::models::UserStatus;
        if user.deleted_at.is_some() || matches!(user.status, UserStatus::Banned) {
            return Err(ApiError::unauthorized("令牌已失效"));
        }

        Ok(Self {
            user_id: data.claims.sub,
        })
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

// ============================================================
// 请求 / 响应 DTO
// ============================================================

/// 对外暴露的用户视图。刻意与内部 [`User`] 分离：
/// `password_hash`/`status`/`role`/`deleted_at` 不出现在任何响应中。
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UserView {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub nickname: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
}

impl From<&User> for UserView {
    fn from(user: &User) -> Self {
        Self {
            id: user.id,
            username: user.username.clone(),
            email: user.email.clone(),
            nickname: user.nickname.clone(),
            avatar_url: user.avatar.clone(),
            created_at: user.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub account: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthPayload {
    pub user: UserView,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct MePayload {
    pub user: UserView,
}

// ---- 文档域 ----

/// 对外暴露的分片视图：
/// `document_id`/`version`/时间戳不随单个 chunk 下发。
#[derive(Debug, Serialize)]
pub struct ChunkView {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub chunk_type: ChunkType,
    pub content: JsonValue,
    pub position: i32,
}

impl From<crate::models::DocumentChunk> for ChunkView {
    fn from(chunk: crate::models::DocumentChunk) -> Self {
        Self {
            id: chunk.id,
            chunk_type: chunk.chunk_type,
            content: chunk.content,
            position: chunk.position,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DocumentContentPayload {
    pub document_id: Uuid,
    pub owner_id: Uuid,
    pub chunks: Vec<ChunkView>,
}

/// 增量操作。`insert`/`delete` 与 `update` 一并落地，
/// 由 PATCH handler 在同一事务内按序执行。
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    Update {
        chunk_id: Uuid,
        content: JsonValue,
        #[serde(default, rename = "type")]
        chunk_type: Option<ChunkType>,
    },
    Insert {
        #[serde(default)]
        chunk_id: Option<Uuid>,
        position: i32,
        #[serde(rename = "type")]
        chunk_type: ChunkType,
        content: JsonValue,
    },
    Delete {
        chunk_id: Uuid,
    },
}

#[derive(Debug, Deserialize)]
pub struct UpdateContentRequest {
    pub operations: Vec<Operation>,
}

#[derive(Debug, Serialize)]
pub struct UpdatedPayload {
    pub updated: bool,
}

// ============================================================
// Handlers —— 鉴权
// ============================================================

/// `POST /auth/register`。
///
/// 按「注册即登录」：创建 pending 用户、签发邮箱验证令牌后，
/// 直接签发 JWT 返回。验证令牌由后续邮件层消费，当前打印到日志
/// （与 repository 层「一期无邮件层」的现状对齐）。
async fn register(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RegisterRequest>,
) -> Result<impl IntoResponse, ApiError> {
    validate_register(&req)?;

    let (user, raw_token) = user_repo::register(
        &state.db,
        &state.hasher,
        req.username.trim(),
        req.email.trim(),
        &req.password,
        None,
    )
    .await?;

    // 一期无邮件层：DEBUG 级别输出令牌指纹（禁止 INFO 级别输出原文）
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(raw_token.as_bytes());
    let fingerprint = hex::encode(&hash[..]);
    tracing::debug!(
        user_id = %user.id,
        token_fingerprint = %&fingerprint[..16],
        "邮箱验证令牌已签发（待邮件层投递）"
    );

    let token = state.issue_token(&user)?;
    Ok(Json(ApiResponse::success(AuthPayload {
        user: UserView::from(&user),
        token,
    })))
}

fn validate_register(req: &RegisterRequest) -> Result<(), ApiError> {
    let username_len = req.username.trim().chars().count();
    if !(MIN_USERNAME_LEN..=MAX_USERNAME_LEN).contains(&username_len) {
        return Err(ApiError::bad_request("用户名长度须为 3–32 字符"));
    }
    if req.password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request("密码至少 8 个字符"));
    }
    // 轻量邮箱格式校验：一个 @、两端非空、域含点。
    let email = req.email.trim();
    let mut parts = email.split('@');
    let (local, domain, extra) = (parts.next(), parts.next(), parts.next());
    let valid = matches!((local, domain, extra),
        (Some(l), Some(d), None) if !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'));
    if !valid {
        return Err(ApiError::bad_request("邮箱格式不合法"));
    }
    Ok(())
}

/// `POST /auth/login`。
async fn login(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<LoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.account.trim().is_empty() || req.password.is_empty() {
        return Err(ApiError::bad_request("账号与密码均必填"));
    }

    let user = user_repo::login(&state.db, &state.hasher, req.account.trim(), &req.password)
        .await
        .map_err(|err| match err {
            Error::WrongPassword => ApiError::bad_credentials(),
            other => ApiError::from(other),
        })?;

    let token = state.issue_token(&user)?;
    Ok(Json(ApiResponse::success(AuthPayload {
        user: UserView::from(&user),
        token,
    })))
}

/// `POST /auth/logout`。
///
/// 无状态 JWT：服务端无操作，返回成功即可——登出语义在前端清 token。
/// 挂 `AuthUser` 仅为了语义完整（无 token 调登出应得 401）。
async fn logout(_auth: AuthUser) -> impl IntoResponse {
    Json(ApiResponse::success(json!({})))
}

/// `GET /users/me`。
async fn me(State(state): State<AppState>, auth: AuthUser) -> Result<impl IntoResponse, ApiError> {
    let user = user_repo::get_user(&state.db, auth.user_id).await?;
    Ok(Json(ApiResponse::success(MePayload {
        user: UserView::from(&user),
    })))
}

// ============================================================
// Handlers —— 文档
// ============================================================

/// `GET /documents/{id}/content`。
///
/// 权限走六级判定：需 `allows_read`；不可见按 404 处理（不泄漏存在性）。
/// `owner_id` 取文档所属空间的 owner——多租户模型下「文档归属者」对应空间 owner。
async fn get_document_content(
    State(state): State<AppState>,
    auth: AuthUser,
    ApiPath(document_id): ApiPath<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    // 权限判定顺带取回 space owner，合并查询降低 RTT
    let info = permission_repo::require_read_with_info(&state.db, document_id, Some(auth.user_id)).await?;

    let chunks = chunk_repo::list_by_document(&state.db, document_id)
        .await?
        .into_iter()
        .map(ChunkView::from)
        .collect();

    Ok(Json(ApiResponse::success(DocumentContentPayload {
        document_id,
        owner_id: info.owner_id,
        chunks,
    })))
}

/// `PATCH /documents/{id}/content`。
///
/// 所有 operations 在**同一事务**内按序执行，末尾回写 word_count，
/// 任一失败整体回滚——不会留下「部分应用」的文档状态。
async fn patch_document_content(
    State(state): State<AppState>,
    auth: AuthUser,
    ApiPath(document_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateContentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // 单次 PATCH 的 operations 条数上限，防止超大请求拖垮事务
    const MAX_OPERATIONS: usize = 200;

    if req.operations.is_empty() {
        return Err(ApiError::bad_request("operations 不能为空"));
    }
    if req.operations.len() > MAX_OPERATIONS {
        return Err(ApiError::bad_request(format!(
            "operations 条数超限（最多 {}）",
            MAX_OPERATIONS
        )));
    }

    // 权限检查移入事务内，消除检查与写入之间的 TOCTOU 窗口
    let mut tx = state.db.begin().await.map_err(ApiError::from)?;
    permission_repo::require_write_tx(&mut tx, document_id, Some(auth.user_id)).await?;

    for op in &req.operations {
        match op {
            Operation::Update {
                chunk_id,
                content,
                chunk_type,
            } => {
                let existing = find_chunk_in_doc(&mut tx, document_id, *chunk_id).await?;
                // type 未传时保留原类型——前端乐观更新只发 content。
                let new_type = chunk_type.unwrap_or(existing.chunk_type);
                chunk_repo::update_content(&mut tx, *chunk_id, new_type, content).await?;
            }
            Operation::Insert {
                position,
                chunk_type,
                content,
                ..
            } => {
                chunk_repo::insert_at(&mut tx, document_id, *position, *chunk_type, content)
                    .await?;
            }
            Operation::Delete { chunk_id } => {
                find_chunk_in_doc(&mut tx, document_id, *chunk_id).await?;
                chunk_repo::delete(&mut tx, *chunk_id).await?;
            }
        }
    }

    chunk_repo::touch_document(&mut tx, document_id).await?;

    tx.commit().await.map_err(ApiError::from)?;

    Ok(Json(ApiResponse::success(UpdatedPayload { updated: true })))
}

/// 在事务内确认 chunk 属于该文档并取回——防止跨文档越权写/删。
async fn find_chunk_in_doc(
    tx: &mut Tx<'_>,
    document_id: Uuid,
    chunk_id: Uuid,
) -> Result<DocumentChunk, ApiError> {
    let chunk = sqlx::query_as!(
        DocumentChunk,
        r#"
        SELECT
            id, document_id,
            type    AS "chunk_type: ChunkType",
            content AS "content: JsonValue",
            position, version, created_at, updated_at
        FROM document_chunks
        WHERE id = $1
        "#,
        chunk_id
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("分片不存在"))?;

    if chunk.document_id != document_id {
        // 分片存在但不属于该文档——按不存在处理，不泄漏跨文档信息。
        return Err(ApiError::not_found("分片不存在"));
    }
    Ok(chunk)
}

async fn document_space_id(db: &Db, document_id: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar!(
        "SELECT space_id FROM documents WHERE id = $1 AND deleted_at IS NULL",
        document_id
    )
    .fetch_optional(db)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("文档不存在"))
}

async fn space_owner_id(db: &Db, space_id: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar!(
        "SELECT owner_id FROM spaces WHERE id = $1 AND deleted_at IS NULL",
        space_id
    )
    .fetch_optional(db)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| {
        ApiError::not_found("空间不存在")
    })
}

// ============================================================
// 路由
// ============================================================

/// 业务路由树。`/health` 在 main.rs 单独挂载（无需状态）。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/users/me", get(me))
        .route(
            "/documents/{document_id}/content",
            get(get_document_content).patch(patch_document_content),
        )
        .fallback(handler_404)
        .with_state(state)
}

/// 未匹配路由的兜底处理器
async fn handler_404() -> ApiError {
    ApiError::not_found("请求的资源不存在")
}

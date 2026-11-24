//! 统一错误模型：业务错误 → HTTP 状态码 + 稳定的错误码。
//!
//! 响应体固定为：
//! ```json
//! { "error": { "code": "not_found", "message": "文档不存在" } }
//! ```
//! 前端与 SDK 依赖 `code` 做判断，`message` 仅用于展示与日志。
//! 字段校验错误额外包含‘error.fields'数组

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

/// 应用层统一错误类型。

#[derive(Debug, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum AppError {
    /// 请求参数非法（400）。
    #[error("{0}")]
    BadRequest(String),
    ///请求字段(用户输入）校验失败（400）。
    #[error("请求参数校验失败")]
    Validation(Vec<FieldError>),
    /// 未认证或凭证失效（401）。
    #[error("{0}")]
    Unauthorized(String),
    /// 已认证但无权限（403）。
    #[error("{0}")]
    Forbidden(String),
    /// 资源不存在（404）。
    #[error("{0}")]
    NotFound(String),
    /// 资源冲突，例如标题重名、乐观锁版本不一致（409）。
    #[error("{0}")]
    Conflict(String),
    /// 服务内部错误（500），细节只写日志，不返回给客户端。
    #[error("服务器内部错误")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    /// 稳定的机器可读错误码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Internal(_) => "internal_error",
            Self::Validation(_) => "validation_failed",
        }
    }

    /// 对应的 HTTP 状态码。
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Validation(_) => StatusCode::BAD_REQUEST,
        }
    }
}

/// 错误响应体。
#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<Vec<FieldError>>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();
        let message = self.to_string();

        if status.is_server_error() {
            tracing::error!(error = ?self, status = status.as_u16(), "request failed");
        } else {
            tracing::debug!(status = status.as_u16(), code, "request rejected");
        }

        let fields = match self {
            Self::Validation(fields) => Some(fields),
            _ => None,
        };

        let body = Json(ErrorResponse {
            error: ErrorBody {
                code,
                message,
                fields,
            },
        });

        (status, body).into_response()
    }
}

impl From<sea_orm::DbErr> for AppError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

/// 业务处理函数的返回类型别名。
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_errors_keep_status_and_code() {
        let error = AppError::NotFound("文档不存在".to_owned());
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
        assert_eq!(error.code(), "not_found");
    }

    #[test]
    fn internal_errors_do_not_leak_details() {
        let error = AppError::Internal(anyhow::anyhow!("连接池耗尽，密码是 hunter2"));
        assert_eq!(error.to_string(), "服务器内部错误");
        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]

    fn validation_errors_should_return_400() {
        let error = AppError::Validation(vec![FieldError {
            field: "email".to_owned(),
            message: "邮箱输错了".to_owned(),
        }]);
        assert_eq!(error.to_string(), "请求参数校验失败")
    }

    #[tokio::test]
    async fn existing_errors_keep_response_shape() {
        let cases = [
            (
                AppError::BadRequest("参数不正确".to_owned()),
                StatusCode::BAD_REQUEST,
                "bad_request",
                "参数不正确",
            ),
            (
                AppError::Unauthorized("请先登录".to_owned()),
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "请先登录",
            ),
            (
                AppError::Forbidden("没有权限".to_owned()),
                StatusCode::FORBIDDEN,
                "forbidden",
                "没有权限",
            ),
            (
                AppError::NotFound("文档不存在".to_owned()),
                StatusCode::NOT_FOUND,
                "not_found",
                "文档不存在",
            ),
            (
                AppError::Conflict("文档已存在".to_owned()),
                StatusCode::CONFLICT,
                "conflict",
                "文档已存在",
            ),
            (
                AppError::Internal(anyhow::anyhow!("服务器内部错误")),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "服务器内部错误",
            ),
        ];

        for (error, expected_status, expected_code, expected_message) in cases {
            assert_eq!(error.status(), expected_status);
            assert_eq!(error.code(), expected_code);
            let response = error.into_response();
            assert_eq!(response.status(), expected_status);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("响应体应该可以读取");

            let actual: serde_json::Value =
                serde_json::from_slice(&bytes).expect("响应体应该是合法的 JSON");

            assert_eq!(
                actual,
                serde_json::json!({
                    "error":{
                        "code":expected_code,
                        "message":expected_message,
                    }
                }),
                "旧错误响应发生变化：{expected_code}"
            );
        }
    }

    #[tokio::test]
    async fn validation_errors_include_fields() {
        let error = AppError::Validation(vec![
            FieldError {
                field: "email".to_owned(),
                message: "邮箱输错了".to_owned(),
            },
            FieldError {
                field: "password".to_owned(),
                message: "密码输错了".to_owned(),
            },
        ]);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.code(), "validation_failed");

        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("响应体应该可以读取");

        let actual: serde_json::Value =
            serde_json::from_slice(&bytes).expect("响应体应该是合法的 JSON");

        assert_eq!(
            actual,
            serde_json::json!({
                "error":{
                    "code":"validation_failed",
                    "message":"请求参数校验失败",
                    "fields":[
                        {
                            "field":"email",
                            "message":"邮箱输错了"
                        },
                        {
                            "field":"password",
                            "message":"密码输错了"
                        }
                    ]
                }
            })
        );
    }
}

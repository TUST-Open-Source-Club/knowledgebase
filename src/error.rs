//! 领域错误，以及 sqlx 错误 → 领域错误的映射。
//!
//! 这里是一个深模块：全部调用方只看到 `Error` 这一个枚举，
//! PostgreSQL 的 SQLSTATE 与约束名（`uniq_users_email_alive` 之类）
//! 不向外泄漏。约束名一旦在迁移里改动，只需改本文件。
//!
//! 关键职责是把唯一约束冲突（SQLSTATE 23505）按约束名分辨成不同语义：
//! 「邮箱已被占用」和「恢复时撞上活跃记录」在数据库层是同一个错误码，
//! 但对调用方而言是两件事。

use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;

/// 唯一约束冲突涉及的资源标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    /// `users.email`
    UserEmail,
    /// `users.username`（登录名）
    Username,
    /// `spaces.slug`
    SpaceSlug,
    /// `documents.slug`，同一空间内唯一
    DocumentSlug,
    /// `space_members` 每空间至多一个 owner
    SpaceOwner,
    /// `document_chunks` 同一文档内 position 唯一
    ChunkPosition,
    /// `refresh_tokens.token_hash`
    TokenHash,
    /// `user_tokens` 同用户同类型至多一条令牌
    UserTokenType,
    /// `user_tokens.token_hash`
    UserTokenHash,
}

impl Identity {
    /// 迁移中定义的约束/索引名 → 资源标识。
    ///
    /// 未列出的约束名归为 `None`，交由 `Error::Database` 兜底，
    /// 避免误把新加的约束当成已知语义。
    fn from_constraint(name: &str) -> Option<Self> {
        match name {
            "uniq_users_email_alive" => Some(Self::UserEmail),
            "uniq_users_username_alive" => Some(Self::Username),
            "uniq_spaces_slug_alive" => Some(Self::SpaceSlug),
            "uniq_documents_space_slug_alive" => Some(Self::DocumentSlug),
            "uniq_space_members_owner" => Some(Self::SpaceOwner),
            "uniq_chunks_doc_position" => Some(Self::ChunkPosition),
            "refresh_tokens_token_hash_key" => Some(Self::TokenHash),
            "uniq_user_tokens_user_type" => Some(Self::UserTokenType),
            "user_tokens_token_hash_key" => Some(Self::UserTokenHash),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserEmail => "user.email",
            Self::Username => "user.username",
            Self::SpaceSlug => "space.slug",
            Self::DocumentSlug => "document.slug",
            Self::SpaceOwner => "space.owner",
            Self::ChunkPosition => "chunk.position",
            Self::TokenHash => "refresh_token.token_hash",
            Self::UserTokenType => "user_token.user_type",
            Self::UserTokenHash => "user_token.token_hash",
        }
    }
}

/// 被操作的资源种类，用于 NotFound 与恢复冲突。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    User,
    Space,
    Document,
    Chunk,
    MediaAsset,
    SpaceMember,
    DocumentPermission,
    RefreshToken,
    UserToken,
}

impl Resource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Space => "space",
            Self::Document => "document",
            Self::Chunk => "chunk",
            Self::MediaAsset => "media_asset",
            Self::SpaceMember => "space_member",
            Self::DocumentPermission => "document_permission",
            Self::RefreshToken => "refresh_token",
            Self::UserToken => "user_token",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{} {id} 不存在", resource.as_str())]
    NotFound { resource: Resource, id: Uuid },

    /// 创建或更新时标识已被占用。
    #[error("{} 已被占用", identity.as_str())]
    Conflict { identity: Identity },

    /// 恢复软删记录时，其标识已被保留期内新建的活跃记录占用。
    ///
    /// 策略：拒绝恢复，由调用方先变更冲突方的标识。
    /// 与 `Conflict` 分开是因为处置方式不同——前者要用户改名，
    /// 后者要用户决定放弃恢复还是重命名活跃记录。
    #[error("恢复 {} 失败：{} 已被活跃记录占用", resource.as_str(), identity.as_str())]
    RestoreConflict {
        resource: Resource,
        identity: Identity,
    },

    /// 违反外键：引用了不存在的父记录，或删除了仍被 RESTRICT 引用的父记录。
    #[error("违反引用完整性：{constraint}")]
    ForeignKey { constraint: String },

    /// 违反 CHECK 约束，如 parent_id = id、position < 0。
    #[error("违反数据约束：{constraint}")]
    Check { constraint: String },

    /// 一次性令牌无效：user_tokens 中查无对应哈希（从未发出、已被使用、
    /// 或被重发覆盖）。刻意不带资源 id——令牌场景下调用方手里只有令牌串，
    /// 且 HTTP 层不应借此泄漏用户是否存在。
    #[error("令牌无效")]
    InvalidToken,

    /// 一次性令牌存在但已过期。与 [`Error::InvalidToken`] 分开：
    /// 两者的用户提示不同（「链接无效」vs「链接已过期，请重新发送」）。
    #[error("令牌已过期，请重新获取")]
    ExpiredToken,

    /// 自助改密码时提供的旧密码不正确。哈希格式损坏也归入此错误——
    /// 对调用方而言两者是同一件事：「旧密码不对」。
    #[error("旧密码不正确")]
    WrongPassword,

    /// 应用层不变量，数据库无法表达（跨空间 parent、深层循环、最后一个 owner）。
    #[error("{0}")]
    Invariant(String),

    /// 客户端输入参数错误（格式/范围/边界），如 position 越界、operations 超限。
    #[error("{0}")]
    BadRequest(String),

    /// 未认证：JWT 缺失、无效或过期（API 映射为 code 1002）。
    #[error("未认证：{0}")]
    Unauthorized(String),

    /// 已认证但无权限（API 映射为 code 1003）。
    #[error("无权限：{0}")]
    Forbidden(String),

    #[error("数据库错误")]
    Database(#[source] sqlx::Error),

    #[error("迁移失败")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

impl Error {
    pub fn invariant(msg: impl Into<String>) -> Self {
        Self::Invariant(msg.into())
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    /// 把 sqlx 错误按约束名翻译成领域错误。
    ///
    /// `restoring` 为 true 时，唯一冲突被解释为恢复冲突——
    /// 同一个 23505 在「新建」和「恢复」两个上下文里语义不同。
    pub fn from_sqlx(err: sqlx::Error, restoring: Option<Resource>) -> Self {
        let Some(db_err) = err.as_database_error() else {
            return Self::Database(err);
        };

        let code = db_err.code().unwrap_or_default().to_string();
        let constraint = db_err.constraint().map(str::to_owned);

        match code.as_str() {
            // unique_violation
            "23505" => match (
                constraint.as_deref().and_then(Identity::from_constraint),
                restoring,
            ) {
                (Some(identity), Some(resource)) => Self::RestoreConflict { resource, identity },
                (Some(identity), None) => Self::Conflict { identity },
                (None, _) => Self::Database(err),
            },
            // foreign_key_violation
            "23503" => match constraint {
                Some(constraint) => Self::ForeignKey { constraint },
                None => Self::Database(err),
            },
            // check_violation
            "23514" => match constraint {
                Some(constraint) => Self::Check { constraint },
                None => Self::Database(err),
            },
            _ => Self::Database(err),
        }
    }
}

impl From<sqlx::Error> for Error {
    /// 默认按「非恢复上下文」翻译。恢复路径须显式调用
    /// [`Error::from_sqlx`] 并传入 `Some(resource)`。
    fn from(err: sqlx::Error) -> Self {
        Self::from_sqlx(err, None)
    }
}

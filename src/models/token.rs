use chrono::{DateTime, Utc};
use ipnetwork::IpNetwork;
use serde::Serialize;
use uuid::Uuid;

/// 刷新令牌。每设备一行，支持多设备同时在线与服务端踢下线。
///
/// 刻意不实现 `Deserialize`：本结构只由数据库产生，不从请求体构造。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    /// SHA-256(refresh_token)。绝不存明文，故不对外序列化。
    #[serde(skip)]
    pub token_hash: String,
    pub device_info: Option<String>,
    /// 列类型为 INET，映射为 `IpNetwork`（单地址即 /32 或 /128）。
    pub ip: Option<IpNetwork>,
    pub expires_at: DateTime<Utc>,
    /// NULL = 有效。
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl RefreshToken {
    pub const COLUMNS: &'static str = "id, user_id, token_hash, device_info, ip, \
         expires_at, revoked_at, created_at";

    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
}

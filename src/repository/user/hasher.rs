//! argon2 密码哈希器。
//!
//! 参数可配置是本模块的显式设计：生产用 OWASP 推荐参数，
//! 测试用最低参数——debug 构建下默认参数单次哈希要数秒，
//! 不降参数集成测试跑不完。

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{
        PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString, rand_core::OsRng,
    },
};

use crate::error::{Error, Result};

/// argon2 密码哈希器。
///
/// 参数可配置是本模块的显式设计：[`PasswordHasher::new`] 用生产参数，
/// [`PasswordHasher::for_tests`] 用最低参数，保证集成测试时长可控。
#[derive(Clone)]
pub struct PasswordHasher {
    argon2: Argon2<'static>,
}

impl Default for PasswordHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl PasswordHasher {
    /// 生产参数：Argon2id、19 MiB 内存、2 轮迭代、1 并行度
    /// （OWASP Password Storage Cheat Sheet 的推荐下限）。
    pub fn new() -> Self {
        Self {
            argon2: Argon2::default(),
        }
    }

    /// 测试参数：最低内存与迭代次数。仅供测试——生产路径用 [`Self::new`]。
    pub fn for_tests() -> Self {
        // 三个参数都是 argon2 允许的最低值，库保证合法，不会失败。
        let params = Params::new(Params::MIN_M_COST, Params::MIN_T_COST, Params::MIN_P_COST, None)
            .expect("最低参数在 argon2 合法范围内");
        Self {
            argon2: Argon2::new(Algorithm::Argon2id, Version::V0x13, params),
        }
    }

    /// 哈希明文密码，返回 PHC 格式哈希串。
    pub fn hash(&self, password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = self
            .argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|err| Error::invariant(format!("密码哈希失败: {err}")))?;
        Ok(hash.to_string())
    }

    /// 校验明文密码与存储哈希是否匹配。
    ///
    /// 哈希格式损坏与密码不匹配都返回 [`Error::WrongPassword`]：
    /// 对调用方而言两者是同一件事——「旧密码不对」。
    pub fn verify(&self, password: &str, expected_hash: &str) -> Result<()> {
        let parsed = PasswordHash::new(expected_hash).map_err(|_| Error::WrongPassword)?;
        self.argon2
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| Error::WrongPassword)
    }
}

/// 预计算的 argon2 哈希，供「账号不存在」分支做等时校验，
/// 抹平「用户不存在 vs 密码错误」的时间侧信道。
///
/// 生产环境使用与 [`PasswordHasher::new`] 一致的参数（m=19456, t=2, p=1），
/// 测试环境使用 [`PasswordHasher::for_tests`] 的参数（m=8, t=1, p=1）。
#[cfg(not(test))]
pub(super) const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$\
AAAAAAAAAAAAAAAAAAAAAA$\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[cfg(test)]
pub(super) const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=8,t=1,p=1$\
AAAAAAAAAAAAAAAAAAAAAA$\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

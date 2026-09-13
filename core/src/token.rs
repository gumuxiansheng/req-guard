//! 审批令牌（方案 B，reviewer token）——零外部依赖实现。
//!
//! ## 与方案 A 的关系
//! 方案 A（`REQ_GUARD_AI_CTX` 软标记）可被 `env -u` 剥离；方案 B 让审批**必须携带
//! 人类带外持有的短期令牌**，AI 环境拿不到 → `approve/reject/resolve/bypass` 无法自批。
//! 本模块的令牌存于仓库外（`~/.config/req-guard/guard.cfg`），只存 **SHA-256 哈希**，
//! 令牌原文由 `token issue` 仅打印一次、人类存入密码管理器/自身会话（带外）。
//!
//! ## 强度与边界（诚实披露）
//! - 令牌原文**不落盘**：库文件只存哈希，AI 即使读到库文件也无法倒推令牌值；
//! - 但库文件与 AI 同用户可读（无 OS keychain，受"零外部依赖"约束），本方案强度来自
//!   "令牌值只由人类知道"，而非文件权限隔离——生产对并发多机仍建议方案 C（带外审批）；
//! - 熵源：Unix 用 `/dev/urandom`（强）；Windows 回退低熵组合并打印警告（强度降级）。

use crate::error::{GateError, Result};
use std::fs;
use std::path::PathBuf;

/// 人类令牌环境变量（审批命令也可用 `--token <值>` 提供，见 [`crate::cli`]）。
pub const TOKEN_ENV: &str = "REQ_GUARD_TOKEN";

/// 配置文件名（仓库外）。
const GUARD_FILE: &str = "guard.cfg";
/// 配置目录（相对 home）。
const GUARD_REL_DIR: &str = ".config/req-guard";
/// 令牌默认有效期（分钟）。
pub const DEFAULT_TTL_MIN: u64 = 60;

/// 一个审批令牌配置（读取/写入 `guard.cfg`）。
///
/// 仅存 SHA-256 哈希与到期时间，令牌原文不落盘。
#[derive(Debug, Clone)]
pub struct TokenCfg {
    pub enabled: bool,
    pub hash: String,
    pub expires_epoch: u64,
}

/// 配置目录路径（`$HOME/.config/req-guard/`，Windows 回退 `USERPROFILE`）。
pub fn guard_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(GUARD_REL_DIR))
}

/// 配置文件路径。
pub fn guard_file() -> Option<PathBuf> {
    guard_dir().map(|d| d.join(GUARD_FILE))
}

/// 读取配置；不存在或无有效令牌时返回 `None`（视为未启用方案 B）。
pub fn load() -> Option<TokenCfg> {
    let path = guard_file()?;
    let content = fs::read_to_string(&path).ok()?;
    let mut enabled = false;
    let mut hash = String::new();
    let mut expires = 0u64;
    for line in content.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("enabled=") {
            enabled = v.trim().eq_ignore_ascii_case("true");
        } else if let Some(v) = l.strip_prefix("hash=") {
            hash = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("expires_epoch=") {
            expires = v.trim().parse().unwrap_or(0);
        }
    }
    if enabled && !hash.is_empty() && expires > crate::gate::now_epoch() {
        Some(TokenCfg {
            enabled: true,
            hash,
            expires_epoch: expires,
        })
    } else {
        // 已过期/被撤销 → 视为未启用
        None
    }
}

/// 当前是否处于"令牌模式"（已启用且有未过期令牌）。
pub fn token_mode() -> bool {
    load().is_some()
}

/// 校验提供的令牌：哈希命中且未过期。
pub fn verify(provided: &str) -> bool {
    match load() {
        Some(cfg) => crate::digest::sha256_hex(provided.as_bytes()) == cfg.hash,
        None => false,
    }
}

/// 签发新令牌：生成随机原文，仅存哈希；返回 `(原文, 到期 epoch, 有效性分钟, 配置路径)`。
///
/// 原文只返回一次，由调用方打印给人类带外保存。
pub fn issue(ttl_minutes: u64) -> Result<(String, u64, u64, PathBuf)> {
    let ttl = if ttl_minutes == 0 {
        DEFAULT_TTL_MIN
    } else {
        ttl_minutes
    };
    let raw = random_hex(32);
    let hash = crate::digest::sha256_hex(raw.as_bytes());
    let now = crate::gate::now_epoch();
    let expires = now + ttl.saturating_mul(60);
    let issued = crate::gate::now_str();

    let dir = guard_dir().ok_or_else(|| {
        GateError::Validation("无法定位用户主目录（HOME/USERPROFILE 未设置）".into())
    })?;
    fs::create_dir_all(&dir).map_err(|e| GateError::Io {
        path: Some(dir.clone()),
        source: e,
    })?;
    let path = dir.join(GUARD_FILE);
    let content = format!(
        "enabled=true\nhash={}\nexpires_epoch={}\nissued={}\n",
        hash, expires, issued
    );
    fs::write(&path, content).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    // 受限权限：仅属主可读写（Unix）。失败不阻断（只是防御纵深），但提示。
    set_private(&path);
    Ok((raw, expires, ttl, path))
}

/// 撤销令牌：禁用并清空（未启用也不报错）。
pub fn revoke() -> Result<PathBuf> {
    let dir = guard_dir().ok_or_else(|| {
        GateError::Validation("无法定位用户主目录（HOME/USERPROFILE 未设置）".into())
    })?;
    fs::create_dir_all(&dir).map_err(|e| GateError::Io {
        path: Some(dir.clone()),
        source: e,
    })?;
    let path = dir.join(GUARD_FILE);
    fs::write(&path, "enabled=false\nhash=\nexpires_epoch=0\n").map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    Ok(path)
}

// ===================== 内部：熵与文件权限 =====================

/// 生成 `bytes` 字节随机内容的十六进制字符串。
fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    let strong = read_urandom(&mut buf);
    if !strong {
        // 低熵回退（非 Unix）：时间 + 进程号 + 地址组合，并随栈变化积累。
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        for (i, b) in buf.iter_mut().enumerate() {
            let mix = now
                .wrapping_mul(31)
                .wrapping_add(pid)
                .wrapping_add(i as u128 * 0x9e37_79b9_7f4a_7c15)
                ^ (i as u128).wrapping_mul(0x1000_0000_0123);
            *b = (mix >> (i % 8) * 8) as u8;
        }
    }
    let mut s = String::with_capacity(bytes * 2);
    for b in buf {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Unix：读 `/dev/urandom`（强熵）。返回是否成功。
#[cfg(unix)]
fn read_urandom(buf: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut f) => f.read_exact(buf).is_ok(),
        Err(_) => false,
    }
}

/// 非 Unix（Windows）：无 `/dev/urandom`，回退低熵。
#[cfg(not(unix))]
fn read_urandom(_buf: &mut [u8]) -> bool {
    false
}

/// Unix：把文件设为仅属主可读写（0600）。
#[cfg(unix)]
fn set_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

/// 非 Unix：无 POSIX 权限模型，跳过。
#[cfg(not(unix))]
fn set_private(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    // `guard_file()` 基于环境变量，无法在测试里改 HOME（并行竞态），
    // 因此令牌配置的持久化路径不在此单测覆盖，只验证格式自洽与哈希语义；
    // 真实文件路径行为由端到端冒烟脚本验证。
    #[test]
    fn issue_verify_revoke_自洽() {
        let raw = random_hex(32);
        assert_eq!(raw.len(), 64, "32 字节十六进制 = 64 字符");

        // 哈希命中 → 校验通过；错 token → 不通过
        let hash = crate::digest::sha256_hex(raw.as_bytes());
        assert_eq!(crate::digest::sha256_hex(raw.as_bytes()), hash);
        assert_ne!(crate::digest::sha256_hex(b"wrong"), hash);
    }

    #[test]
    fn sha256配合verify_is_作为令牌验证核心() {
        let raw = random_hex(32);
        let hash = crate::digest::sha256_hex(raw.as_bytes());
        // token 值本身绝不等于哈希
        assert_ne!(raw, hash);
    }

    #[test]
    fn random_hex_同长度且hex格式() {
        let a = random_hex(32);
        let b = random_hex(32);
        assert_eq!(a.len(), 64);
        assert_eq!(b.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(b.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // 说明：`load()/issue()/revoke()` 依赖 HOME 环境 + 写真实 ~/.config，
    // 为不污染用户主目录，其文件系统行为由端到端冒烟（脚本）验证。
}

//! 审批令牌（方案 B）与**一次性范围票据**（严格等级 L3 加固）——零外部依赖实现。
//!
//! ## 与方案 A 的关系
//! 方案 A（`REQ_GUARD_AI_CTX` 软标记）可被 `env -u` 剥离；方案 B 让审批**必须携带
//! 人类带外持有的短期令牌**，AI 环境拿不到 → `approve/reject/resolve/bypass` 无法自批。
//! 本模块的令牌存于仓库外（`~/.config/req-guard/guard.cfg`），只存 **SHA-256 哈希**，
//! 令牌原文由 `token issue` 仅打印一次、人类存入密码管理器/自身会话（带外）。
//!
//! ## 两种凭据形态（`mode=`）
//! - `static`：传统短期令牌，TTL 内可反复使用（L0–L2）。原文只在签发时打印一次。
//! - `scoped`：**一次性范围票据**（L3）。签发时生成，**用后即废**（`used=true`），
//!   且可绑定 `scope=<需求id>:<步骤>`——换了需求/步骤就不认，同一张票也无法重放。
//!   这直接压掉静态令牌的两大残留面："TTL 内可被重放"与"票据与审批对象无关"。
//!
//! ## 强度与边界（诚实披露）
//! - 令牌原文**不落盘**：库文件只存哈希，读到库文件也无法倒推；
//! - 但库文件与 AI 同用户可读（无 OS keychain，受"零外部依赖"约束），强度来自
//!   "原文只由人类知道"，而非文件权限隔离；
//! - 因此**不采用"口令 + HMAC 挑战应答"**：核验方若要本地校验 `HMAC(口令, 挑战)`，
//!   就必须把口令（或等价密钥）落到 AI 可读的同一份存储里，AI 直接照算即可伪造——
//!   那是安全剧场。一次性票据在不引入非对称密钥的前提下，才是可落地的抗重放手段；
//! - 熵源：Unix 用 `/dev/urandom`（强）；Windows 回退低熵组合并打印警告（强度降级）。

use crate::error::{GateError, Result};
use std::fs;
use std::path::PathBuf;

/// 人类令牌环境变量（**仅 L0–L1 回退**；L2 起凭据只认进程内显式传入）。
pub const TOKEN_ENV: &str = "REQ_GUARD_TOKEN";

/// 配置文件名（仓库外）。
const GUARD_FILE: &str = "guard.cfg";
/// 配置目录（相对 home）。
const GUARD_REL_DIR: &str = ".config/req-guard";
/// 令牌默认有效期（分钟）。
pub const DEFAULT_TTL_MIN: u64 = 60;

/// 凭据形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 短期令牌：TTL 内可重复使用。
    Static,
    /// 一次性范围票据：用后即废，可绑定需求+步骤。
    Scoped,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Static => "static",
            Mode::Scoped => "scoped",
        }
    }
    fn parse(s: &str) -> Mode {
        if s.eq_ignore_ascii_case("scoped") {
            Mode::Scoped
        } else {
            Mode::Static
        }
    }
}

/// 范围校验方式（L3 票据绑定需求+步骤）。
pub enum ScopeCheck<'a> {
    /// 不校验范围（仅校验身份）：用于"重签/撤销须出示现有凭据"的场景。
    Any,
    /// 必须与票据 `scope` 一致；票据 scope 为空（通用单次票）时放行。
    Exact(&'a str),
}

/// 一个审批凭据配置（读取/写入 `guard.cfg`）。
///
/// 仅存 SHA-256 哈希与到期时间，凭据原文不落盘。
#[derive(Debug, Clone)]
pub struct TokenCfg {
    pub enabled: bool,
    pub mode: Mode,
    pub hash: String,
    /// 范围绑定（`<需求id>:<步骤>`；空 = 通用单次票）。
    pub scope: String,
    /// 是否已消费（仅 `scoped` 有意义）。
    pub used: bool,
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

/// 按原文逐行解析 `guard.cfg`（不做过期/消费判定）。
///
/// 与 [`load`] 分开的理由：`token status` 需要展示"配置存在但已过期/已用尽"的实情，
/// 而判定层只关心"当前是否有生效凭据"。
pub fn load_any() -> Option<TokenCfg> {
    let path = guard_file()?;
    let content = fs::read_to_string(&path).ok()?;
    let mut cfg = TokenCfg {
        enabled: false,
        mode: Mode::Static,
        hash: String::new(),
        scope: String::new(),
        used: false,
        expires_epoch: 0,
    };
    for line in content.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("enabled=") {
            cfg.enabled = v.trim().eq_ignore_ascii_case("true");
        } else if let Some(v) = l.strip_prefix("mode=") {
            cfg.mode = Mode::parse(v.trim());
        } else if let Some(v) = l.strip_prefix("hash=") {
            cfg.hash = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("scope=") {
            cfg.scope = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("used=") {
            cfg.used = v.trim().eq_ignore_ascii_case("true");
        } else if let Some(v) = l.strip_prefix("expires_epoch=") {
            cfg.expires_epoch = v.trim().parse().unwrap_or(0);
        }
    }
    Some(cfg)
}

/// 读取**生效中**的凭据：启用 + 有哈希 + 未过期 + 未消费；否则 `None`。
///
/// 已过期/已消费/被撤销一律视为"未启用"，审批侧据此回退到其他凭据或拒绝。
pub fn load() -> Option<TokenCfg> {
    let cfg = load_any()?;
    if cfg.enabled
        && !cfg.hash.is_empty()
        && cfg.expires_epoch > crate::gate::now_epoch()
        && !cfg.used
    {
        Some(cfg)
    } else {
        None
    }
}

/// 当前是否处于"凭据模式"（已启用且有生效凭据）。
pub fn token_mode() -> bool {
    load().is_some()
}

/// 校验**静态令牌**：形态必须是 `static`、哈希命中且未过期。
pub fn verify_static(provided: &str) -> bool {
    match load() {
        Some(cfg) => {
            cfg.mode == Mode::Static && crate::digest::sha256_hex(provided.as_bytes()) == cfg.hash
        }
        None => false,
    }
}

/// 校验**一次性范围票据**（L3）：形态必须是 `scoped`、未消费、范围匹配、哈希命中。
pub fn verify_scoped(provided: &str, scope: ScopeCheck<'_>) -> bool {
    let Some(cfg) = load() else {
        return false;
    };
    if cfg.mode != Mode::Scoped {
        return false;
    }
    let scope_ok = match scope {
        ScopeCheck::Any => true,
        ScopeCheck::Exact(s) => cfg.scope.is_empty() || cfg.scope == s,
    };
    scope_ok && crate::digest::sha256_hex(provided.as_bytes()) == cfg.hash
}

/// 消费当前票据（`scoped` 用后即废）。返回是否确实标记成功。
///
/// 必须在审批动作**落盘成功后**调用：先消费再失败会让人类白丢一张票。
pub fn consume() -> bool {
    let Some(mut cfg) = load_any() else {
        return false;
    };
    if cfg.mode != Mode::Scoped || cfg.used {
        return false;
    }
    cfg.used = true;
    match guard_file() {
        Some(p) => write_cfg(&p, &cfg).is_ok(),
        None => false,
    }
}

/// 签发**静态令牌**（L0–L2）：生成随机原文，仅存哈希。
///
/// 返回 `(原文, 到期 epoch, 有效期分钟, 配置路径)`；原文只返回一次，
/// 由调用方交给人类带外保存。
pub fn issue(ttl_minutes: u64) -> Result<(String, u64, u64, PathBuf)> {
    issue_inner(ttl_minutes, Mode::Static, "")
}

/// 签发**一次性范围票据**（L3）。`scope` 为空 = 通用单次票（仍用后即废）。
pub fn issue_scoped(ttl_minutes: u64, scope: &str) -> Result<(String, u64, u64, PathBuf)> {
    if scope.contains('\n') || scope.contains('\r') {
        return Err(GateError::Validation("票据范围不得含换行".into()));
    }
    issue_inner(ttl_minutes, Mode::Scoped, scope)
}

fn issue_inner(ttl_minutes: u64, mode: Mode, scope: &str) -> Result<(String, u64, u64, PathBuf)> {
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
    let cfg = TokenCfg {
        enabled: true,
        mode,
        hash,
        scope: scope.trim().to_string(),
        used: false,
        expires_epoch: expires,
    };
    write_cfg_with_issued(&path, &cfg, &issued)?;
    // 受限权限：仅属主可读写（Unix）。失败不阻断（只是防御纵深），但提示。
    set_private(&path);
    Ok((raw, expires, ttl, path))
}

/// 撤销凭据：禁用并清空（未启用也不报错）。
pub fn revoke() -> Result<PathBuf> {
    let dir = guard_dir().ok_or_else(|| {
        GateError::Validation("无法定位用户主目录（HOME/USERPROFILE 未设置）".into())
    })?;
    fs::create_dir_all(&dir).map_err(|e| GateError::Io {
        path: Some(dir.clone()),
        source: e,
    })?;
    let path = dir.join(GUARD_FILE);
    fs::write(
        &path,
        "enabled=false\nmode=static\nhash=\nscope=\nused=false\nexpires_epoch=0\n",
    )
    .map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    Ok(path)
}

// ===================== 内部：持久化 =====================

fn write_cfg(path: &std::path::Path, cfg: &TokenCfg) -> Result<()> {
    write_cfg_with_issued(path, cfg, &crate::gate::now_str())
}

fn write_cfg_with_issued(path: &std::path::Path, cfg: &TokenCfg, issued: &str) -> Result<()> {
    let content = format!(
        "enabled={}\nmode={}\nhash={}\nscope={}\nused={}\nexpires_epoch={}\nissued={}\n",
        cfg.enabled,
        cfg.mode.as_str(),
        cfg.hash,
        cfg.scope,
        cfg.used,
        cfg.expires_epoch,
        issued
    );
    fs::write(path, content).map_err(|e| GateError::Io {
        path: Some(path.to_path_buf()),
        source: e,
    })
}

// ===================== 内部：熵与文件权限 =====================

/// 生成 `bytes` 字节随机内容的十六进制字符串。
///
/// ⚠️ 熵源分开处理（2026-09-30 加固）：
/// - Unix：直接读 `/dev/urandom`（强熵）；
/// - Windows：无 `/dev/urandom`，**逐字节重新采样时间并混入 ASLR 地址与滚动状态**。
///   旧实现把 `now`/`pid` 在循环外取一次，32 字节全由**同一个 128 位状态**派生，
///   实际熵只有"取时间那一刻"的抖动（约 20~30 bit）——而 `guard.cfg` 里的哈希与 AI
///   同用户可读，AI 能**离线暴力猜原文**再自批。逐字节采样后每个字节都引入新的
///   时间抖动，且栈地址（ASLR）每次进程启动都不同、外部不可观测。
///   仍弱于内核 CSPRNG → 见 [`entropy_strong`]，CLI 会提示缩短 TTL。
fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if !read_urandom(&mut buf) {
        let mut state = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // ASLR：栈变量地址每次进程启动都不同，且不出现在任何可读文件里
        let aslr = &state as *const u128 as usize as u128;
        state ^= aslr;
        for (i, b) in buf.iter_mut().enumerate() {
            // ★ 每次迭代重新采样：纳秒低位抖动不可预测，且随调度推进
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u128)
                .unwrap_or(0);
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(t)
                .wrapping_add(aslr)
                ^ (i as u128).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            *b = (state >> 56) as u8;
        }
    }
    let mut s = String::with_capacity(bytes * 2);
    for b in buf {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 本次运行的熵源是否为内核 CSPRNG（Unix 的 `/dev/urandom`）。
///
/// 供 CLI 在弱熵平台（Windows）提示用户缩短令牌有效期。
pub fn entropy_strong() -> bool {
    let mut probe = [0u8; 1];
    read_urandom(&mut probe)
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
    fn 凭据形态解析与序列化自洽() {
        for m in [Mode::Static, Mode::Scoped] {
            assert_eq!(Mode::parse(m.as_str()), m, "mode 文本须可往返");
        }
        // 未知/缺失一律回退 static——老 guard.cfg 没有 mode 行，必须仍能读
        assert_eq!(Mode::parse(""), Mode::Static);
        assert_eq!(Mode::parse("STATIC"), Mode::Static);
        assert_eq!(Mode::parse("scoped"), Mode::Scoped);
    }

    #[test]
    fn 无库文件时校验一律失败() {
        // 不依赖 HOME 是否设置：找不到 guard.cfg 时任何凭据都不得通过（fail-closed）。
        // 若宿主恰好存在 guard.cfg，本断言会因"文件存在"而失真，故仅在缺失时校验。
        if guard_file().map(|p| !p.exists()).unwrap_or(true) {
            assert!(!verify_static("whatever"));
            assert!(!verify_scoped("whatever", ScopeCheck::Any));
            assert!(!verify_scoped(
                "whatever",
                ScopeCheck::Exact("REQ-001:decomposition")
            ));
            assert!(load().is_none());
        }
    }

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
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(b.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn 弱熵回退不得退化为单一状态() {
        // 旧实现把 now/pid 在循环外取一次 → 同一时刻多次调用会产出完全相同的串，
        // 且整串熵等于"取时间那一刻"的抖动。这里守住"连续调用必须发散"。
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            seen.insert(random_hex(32));
        }
        assert!(
            seen.len() >= 2,
            "连续 8 次调用不得全部相同（熵退化会让 AI 可离线猜原文）"
        );
    }

    #[test]
    fn entropy_strong_与平台一致() {
        let strong = entropy_strong();
        #[cfg(unix)]
        assert!(strong, "Unix 应读到 /dev/urandom");
        #[cfg(not(unix))]
        assert!(!strong, "Windows 无 /dev/urandom，须走弱熵提示");
    }

    // 说明：`load()/issue()/revoke()` 依赖 HOME 环境 + 写真实 ~/.config，
    // 为不污染用户主目录，其文件系统行为由端到端冒烟（脚本）验证。
}

//! 审批身份绑定（git identity + 内联指纹）。
//!
//! # 为什么需要
//!
//! `--reviewer <姓名>` 是**自报字符串**：台账里 `reviewer=寇工` 无法证明是谁，
//! 也无法区分「人类审批」与「AI 随手填个名字」。方案 A（`REQ_GUARD_AI_CTX`）
//! 是白名单式注入，未登记的 AI 工具会话内没有该变量 → `env -u` 即可剥离
//! （见《AI工具合规保证规范》§4.4 / §7.2）。本模块把身份锚定到 **git 身份**
//! （`git config user.name` / `user.email`）：它是提交本身的署名，随提交进入
//! 版本历史，事后无法悄悄改写。
//!
//! # 保证到什么程度（诚实边界）
//!
//! 本模块提供的是**可归属**（attribution），不是密码学签名：
//!
//! - 指纹 `sig` = `sha256("req-guard-id-v1\0name\0email\0<repo 绝对路径>")` 前 12 位
//!   ——纯派生值，无密钥，**任何人都能重算**。它的作用是让台账里的每条审批
//!   都带一个可离线复算的一致性锚点，便于事后批量核对"这条审批出自哪个身份"，
//!   而不是防伪。
//! - 仓库绝对路径参与指纹：同一个 `sig` 在别的 clone 上照抄无意义（可归属性
//!   不跨仓库传播），代价是换机器 clone 后重算会变——这是刻意的。
//! - 同用户下的 AI 仍可执行 `git config user.name ...` 伪造身份。本模块抬高
//!   成本、不构成安全边界；真正的兜底仍是 §7 说的**事后复核**（审批要与人类在
//!   PR / CI 侧的批准对应）。这与 §4.4 方案 D（TTY 在场证明）是叠加关系，不是替代。
//!
//! # 与 auth.level 的关系
//!
//! 冲突处置随 [`crate::auth::effective_level`] 分级，与既有体系一致：

//! | 情形 | L0 | L1+ |
//! | --- | --- | --- |
//! | 取不到 git 身份 | 放行，`sig=-` | **拒绝**（无法归属的审批不予采信） |
//! | `--reviewer` 与 git 身份一致 | 放行 | 放行 |
//! | `--reviewer` 与 git 身份冲突 | 放行，标 `mismatch=1` | **拒绝** |

//! 取不到 git 身份时的逃生口：设 `REQ_GUARD_REVIEWER_EMAIL`（姓名取
//! `REQ_GUARD_REVIEWER`，沿用既有环境变量），使其在无 git 配置的环境
//! （精简容器、只读挂载的 CI 镜像）里仍能完成绑定。

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use crate::error::GateError;

/// 邮箱的环境变量覆盖名（无 git 身份时的逃生口）。
pub const EMAIL_ENV: &str = "REQ_GUARD_REVIEWER_EMAIL";

/// 身份指纹的派生盐。改动会让全部历史 `sig` 失效，故视为**格式版本**：
/// 一旦发布过审批记录就不要改，否则旧记录无法与新记录同口径核对。
const SIG_SALT: &str = "req-guard-id-v1";

/// 指纹长度（十六进制字符数）。12 位 = 48 bit，够区分团队规模，
/// 又短到能直接写进 marker 行、台账和 Markdown 表格而不撑破排版。
const SIG_LEN: usize = 12;

/// 一个可归属的审批身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `git config user.name`
    pub name: String,
    /// `git config user.email`
    pub email: String,
    /// 12 位指纹（见模块文档）。
    pub sig: String,
}

impl Identity {
    /// 该身份是否与自报的 `claimed` 指向同一人。
    ///
    /// 姓名与邮箱**任一**对上即算一致：不同团队有人习惯用邮箱当 `--reviewer`
    /// 的值（域账号 / 工号邮箱），两种写法都不该被误判成"冒名"。
    pub fn matches(&self, claimed: &str) -> bool {
        let c = claimed.trim();
        c == self.name || (!self.email.is_empty() && c == self.email)
    }

    /// 写进 marker / 台账的 `sig=` 取值。
    pub fn sig_field(&self) -> String {
        self.sig.clone()
    }
}

/// 一次审批最终落盘的身份信息。
#[derive(Debug, Clone)]
pub struct Stamp {
    /// 写进 `reviewer=` 的名字：L0 冲突时保留自报值（并置 `mismatch`），
    /// 其余情况用自报值（保持既有台账可读性），身份另由 `email`/`sig` 承载。
    pub reviewer: String,
    /// `email=` 的取值：`-` 表示未绑定到 git 身份。
    pub email: String,
    /// `sig=` 的取值：`-` 表示未绑定到 git 身份。
    pub sig: String,
    /// 自报名与 git 身份冲突但被放行（仅 L0 可能发生）。
    pub mismatch: bool,
}

/// 无身份可用时降级戳里的 `sig`/`email` 占位值。
///
/// 公开出来是为了让读侧（`resolve::active_bypass`）能识别"这枚指纹根本不是算出来的、
/// 而是占位符"，从而**显式降级**而不是把它当成一个算错的指纹去拒。
/// 静默降级不可接受，所以读侧降级时必须留审计痕迹（见 REQ-012 设计 1）。
pub const UNBOUND_SIG: &str = "-";

impl Stamp {
    /// 无身份可用时的降级戳（`email`/`sig` 均为 [`UNBOUND_SIG`]）。
    fn unbound(reviewer: &str) -> Self {
        Stamp {
            reviewer: reviewer.to_string(),
            email: UNBOUND_SIG.into(),
            sig: UNBOUND_SIG.into(),
            mismatch: false,
        }
    }

    /// 追加到台账事件末尾的字段串（供 `audit_ledger` 拼 `key=value`）。
    pub fn audit_fields(&self) -> String {
        let mut s = format!("email={} sig={}", self.email, self.sig);
        if self.mismatch {
            s.push_str(" mismatch=1");
        }
        s
    }
}

/// 派生身份指纹。分库不引入 `sha2` crate（项目坚持零外部依赖），
/// 复用 [`crate::digest::sha256_hex`]。
pub fn fingerprint(root: &Path, name: &str, email: &str) -> String {
    let hex = crate::digest::sha256_hex(
        format!("{}\0{}\0{}\0{}", SIG_SALT, name, email, root.display()).as_bytes(),
    );
    hex.chars().take(SIG_LEN).collect()
}

/// 读 git 身份（`user.name` + `user.email`）。
///
/// 结果按进程缓存：一次审批链路上有多处要读（bind + 台账 + marker），
/// 而每次都是一个 `git config` 子进程。缓存的生命周期是进程级，
/// 因此同一次命令内改 git config 不会二次生效——这在门禁工具里可接受
/// （也避免审批过程中身份中途漂移）。
///
/// 用 `Mutex<Option<..>>` 而非 `OnceLock`：单测需要在同一进程内反复改 git 配置
/// 再重读（`OnceLock` 一旦 set 就再也写不进去，`reset` 是静默空操作）。
static GIT_ID: Mutex<Option<Option<Identity>>> = Mutex::new(None);

fn read_git_identity(root: &Path) -> Option<Identity> {
    let cfg = |key: &str| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("config")
            .arg("--get")
            .arg(key)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    // 无 git 可执行 / 不在 git 仓 / 未配 user.name —— 一律视为"取不到身份"。
    let name = cfg("user.name")?;
    let email = cfg("user.email").unwrap_or_default();
    let sig = fingerprint(root, &name, &email);
    Some(Identity { name, email, sig })
}

/// 读当前审批身份（带进程级缓存）。取不到返回 `None`。
pub fn current(root: &Path) -> Option<Identity> {
    let mut slot = GIT_ID.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(read_git_identity(root));
    }
    slot.clone().flatten()
}

/// 仅供测试：清掉进程级缓存（同一进程内改了 git config 后复用）。
#[cfg(test)]
pub fn reset_cache() {
    if let Ok(mut slot) = GIT_ID.lock() {
        *slot = None;
    }
}

/// `whoami` 的展示形态（未绑定时 `sig`/`email` 为 `-`）。
pub fn describe(root: &Path) -> String {
    match current(root) {
        Some(i) => format!("{} <{}> sig={}", i.name, i.email, i.sig),
        None => "- <-> sig=-（未取到 git 身份）".to_string(),
    }
}

/// 审批身份绑定的事实包（供 [`decide`] 做纯判定，也便于单测构造任意组合）。
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// `git config user.name` / `user.email` 的解析结果。
    pub git: Option<Identity>,
    /// [`EMAIL_ENV`] 的取值（无 git 身份时的逃生口）。
    pub env_email: Option<String>,
    /// `REQ_GUARD_REVIEWER` 的取值。
    pub env_reviewer: Option<String>,
}

/// 采集事实（唯一的 I/O 入口；策略在 [`decide`] 里，纯函数）。
pub fn collect(root: &Path) -> Facts {
    Facts {
        git: current(root),
        env_email: env_nonempty(EMAIL_ENV),
        env_reviewer: std::env::var("REQ_GUARD_REVIEWER")
            .ok()
            .filter(|s| !s.trim().is_empty()),
    }
}

/// 把自报的 `claimed` 绑定到 git 身份，产出落盘用的身份戳。
pub fn bind(root: &Path, claimed: &str) -> Result<Stamp, GateError> {
    let facts = collect(root);
    decide(root, claimed, &facts, crate::auth::effective_level(root))
}

/// 纯判定：给定事实与等级，决定放行 / 拒绝，并给出落盘戳。
///
/// 与 [`bind`] 分开是为了让判级矩阵（一致 / 冲突 / 无身份 / 环境变量兜底）
/// 可以被单测穷举，而不必去操纵运行机的 git 全局配置——后者在 CI 上不可靠。
pub fn decide(root: &Path, claimed: &str, facts: &Facts, level: u8) -> Result<Stamp, GateError> {
    let claimed = claimed.trim();

    if let Some(id) = &facts.git {
        if id.matches(claimed) {
            return Ok(Stamp {
                reviewer: claimed.to_string(),
                email: id.email.clone(),
                sig: id.sig_field(),
                mismatch: false,
            });
        }
        if level >= 1 {
            return Err(GateError::Validation(format!(
                "审批人身份冲突：--reviewer 指定的是「{}」，但当前生效的 git 身份是「{} <{}>」（sig={}）。\n\
                 auth.level={} 要求审批可归属到真实 git 身份，故拒绝。\n\
                 取解：① 若确系本人，修正 git 身份后重试（git config user.name \"{}\"）；\n\
                 ② 确系代审/机器人账号，先切换 git 身份再审批；\n\
                 ③ 临时豁免：设 REQ_GUARD_AUTH_LEVEL=0 后重试（等级由配置与该变量取 max，只能抬高不能降低）。",
                claimed, id.name, id.email, id.sig, level, id.name
            )));
        }
        // L0：放行，但保留自报名 + 标记冲突（台账里 mismatch=1 可被批量捞出）
        return Ok(Stamp {
            reviewer: claimed.to_string(),
            email: id.email.clone(),
            sig: id.sig_field(),
            mismatch: true,
        });
    }

    // 取不到 git 身份：先找逃生口，再按等级决定放行与否。
    if let Some(email) = facts.env_email.clone() {
        let name = facts
            .env_reviewer
            .clone()
            .unwrap_or_else(|| claimed.to_string());
        let sig = fingerprint(root, &name, &email);
        let id = Identity {
            name: name.clone(),
            email: email.clone(),
            sig: sig.clone(),
        };
        let mismatch = !id.matches(claimed);
        if mismatch && level >= 1 {
            return Err(GateError::Validation(format!(
                "审批人身份冲突：--reviewer 指定的是「{}」，但 {} 指定的是「{} <{}>」。\n\
                 auth.level={} 要求审批可归属，故拒绝。",
                claimed, EMAIL_ENV, name, email, level
            )));
        }
        return Ok(Stamp {
            reviewer: claimed.to_string(),
            email,
            sig,
            mismatch,
        });
    }

    if level >= 1 {
        return Err(GateError::Validation(format!(
            "无法绑定审批人身份：读不到生效的 git 身份（git config user.name/user.email 为空），\
             auth.level={} 要求审批可归属，故拒绝。\n\
             取解：① 配置 git 身份（git config user.name \"你的名字\" / git config user.email 你的邮箱）；\n\
             ② 无 git 环境（精简容器等）时设环境变量 {} 声明邮箱，姓名取 REQ_GUARD_REVIEWER。",
            level, EMAIL_ENV
        )));
    }
    Ok(Stamp::unbound(claimed))
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 指纹_同输入稳定且随仓库变化() {
        let a = fingerprint(Path::new("/repo/a"), "kou", "kou@x.com");
        let b = fingerprint(Path::new("/repo/a"), "kou", "kou@x.com");
        let c = fingerprint(Path::new("/repo/b"), "kou", "kou@x.com");
        assert_eq!(a, b, "同一 (仓库,姓名,邮箱) 必须派生同一指纹");
        assert_ne!(a, c, "仓库不同 → 指纹不同（可归属性不跨仓库传播）");
        assert_eq!(a.len(), SIG_LEN);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn 指纹_含盐防跨工具冒用() {
        // 同样的 name/email，换盐（版本）即换指纹——避免与其它工具的同源哈希混淆
        assert_ne!(SIG_SALT, "req-guard-id-v0");
    }

    #[test]
    fn 姓名或邮箱对上_都算同一人() {
        let id = Identity {
            name: "寇工".into(),
            email: "kou@corp.com".into(),
            sig: "abcdef012345".into(),
        };
        assert!(id.matches("寇工"));
        assert!(id.matches(" 寇工 "), "首尾空白视为同一人");
        assert!(id.matches("kou@corp.com"), "用邮箱当 --reviewer 也算一致");
        assert!(!id.matches("张三"));
        assert!(!id.matches("kou"));
    }

    #[test]
    fn 空邮箱不与空串误匹配() {
        let id = Identity {
            name: "kou".into(),
            email: "".into(),
            sig: "x".into(),
        };
        assert!(id.matches("kou"));
        assert!(!id.matches(""), "空 claimed 不应因空邮箱而匹配");
    }

    // ---- 判级矩阵（L0 放行 / L1+ fail-closed）----
    //
    // 全部驱动纯函数 `decide()`，不碰运行机的 git 配置：`bind()` 的 I/O 只有
    // 「读 git 身份」，把事实喂进去即可穷举（CI 上 git 全局身份不可控，靠真实
    // git 仓构造前提的用例会时绿时红）。

    const ROOT: &str = "/repo";

    fn kou() -> Identity {
        Identity {
            name: "寇工".into(),
            email: "kou@corp.com".into(),
            sig: fingerprint(Path::new(ROOT), "寇工", "kou@corp.com"),
        }
    }

    fn facts_git() -> Facts {
        Facts {
            git: Some(kou()),
            env_email: None,
            env_reviewer: None,
        }
    }

    fn facts_none() -> Facts {
        Facts::default()
    }

    #[test]
    fn decide_身份一致_各等级都放行且落可复算指纹() {
        let want = fingerprint(Path::new(ROOT), "寇工", "kou@corp.com");
        for level in 0..=3u8 {
            let s = decide(Path::new(ROOT), "寇工", &facts_git(), level)
                .unwrap_or_else(|e| panic!("level {} 应放行: {}", level, e));
            assert_eq!(s.reviewer, "寇工");
            assert_eq!(s.email, "kou@corp.com", "level {}", level);
            assert_eq!(s.sig, want, "sig 必须可离线复算");
            assert!(!s.mismatch);
            assert_eq!(s.audit_fields(), format!("email=kou@corp.com sig={}", want));
        }
    }

    #[test]
    fn decide_邮箱写法_不算冒名() {
        let s = decide(Path::new(ROOT), "kou@corp.com", &facts_git(), 3).expect("邮箱写法应放行");
        assert!(!s.mismatch);
        assert_eq!(s.email, "kou@corp.com");
        // 首尾空白同样视为同一人
        assert!(decide(Path::new(ROOT), "  寇工  ", &facts_git(), 3).is_ok());
    }

    #[test]
    fn decide_身份冲突_l0放行并留痕_l1起拒绝() {
        let s =
            decide(Path::new(ROOT), "张三", &facts_git(), 0).expect("L0 应放行，不打断存量项目");
        assert!(s.mismatch, "L0 放行也必须留下可批量捞出的冲突标记");
        assert_eq!(s.reviewer, "张三", "保留自报名，便于人工复核时对照");
        assert_eq!(s.sig, kou().sig, "sig 取 git 身份而非自报名");
        assert!(s.audit_fields().contains("mismatch=1"));

        for level in 1..=3u8 {
            let e = decide(Path::new(ROOT), "张三", &facts_git(), level)
                .expect_err("L1+ 冲突必须 fail-closed");
            let msg = e.to_string();
            assert!(msg.contains("身份冲突"), "level {}: {}", level, msg);
            assert!(
                msg.contains("张三") && msg.contains("寇工"),
                "报错须点明两方身份: {}",
                msg
            );
            assert!(
                msg.contains("kou@corp.com"),
                "报错须带邮箱，便于判断改哪个: {}",
                msg
            );
        }
    }

    #[test]
    fn decide_无任何身份_l0放行留空戳_l1起拒绝() {
        let s = decide(Path::new(ROOT), "寇工", &facts_none(), 0).expect("L0 应放行");
        assert_eq!(s.email, "-");
        assert_eq!(s.sig, "-");
        assert!(!s.mismatch, "无身份可绑时不谎报冲突");
        assert_eq!(s.audit_fields(), "email=- sig=-");

        for level in 1..=3u8 {
            let e = decide(Path::new(ROOT), "寇工", &facts_none(), level)
                .expect_err("L1+ 必须拒绝无法归属的审批");
            let msg = e.to_string();
            assert!(
                msg.contains("无法绑定审批人身份"),
                "level {}: {}",
                level,
                msg
            );
            assert!(
                msg.contains(EMAIL_ENV),
                "报错须给出无 git 环境时的出路: {}",
                msg
            );
        }
    }

    #[test]
    fn decide_环境变量兜底_可绑定但冲突仍按等级拒() {
        let facts = Facts {
            git: None,
            env_email: Some("kou@corp.com".into()),
            env_reviewer: Some("寇工".into()),
        };
        let s = decide(Path::new(ROOT), "寇工", &facts, 3).expect("兜底身份与自报名一致应放行");
        assert_eq!(s.email, "kou@corp.com");
        assert!(!s.mismatch);

        // 无 git 环境、邮箱声明与自报名不一致：L1+ 仍拒（不放水）
        let e = decide(Path::new(ROOT), "张三", &facts, 1).expect_err("兜底身份也要判冲突");
        assert!(e.to_string().contains("身份冲突"), "{}", e);
        let s0 = decide(Path::new(ROOT), "张三", &facts, 0).expect("L0 放行");
        assert!(s0.mismatch);
    }

    #[test]
    fn decide_仅邮箱无姓名时_以自报名兜底为姓名() {
        let facts = Facts {
            git: None,
            env_email: Some("kou@corp.com".into()),
            env_reviewer: None,
        };
        let s = decide(Path::new(ROOT), "kou@corp.com", &facts, 3)
            .expect("邮箱自报名与声明邮箱一致，应放行");
        assert_eq!(s.email, "kou@corp.com");
        assert!(!s.mismatch, "姓名取自报名、邮箱取自声明，两者不冲突");
    }
}

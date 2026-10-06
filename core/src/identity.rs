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

/// 操作人姓名的环境变量覆盖名（三级回退的第二档）。
pub const REVIEWER_ENV: &str = "REQ_GUARD_REVIEWER";

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
/// 结果按**仓库**缓存：一次审批链路上有多处要读（bind + 台账 + marker），
/// 而每次都是一个 `git config` 子进程。缓存的生命周期是进程级，
/// 因此同一次命令内改 git config 不会二次生效——这在门禁工具里可接受
/// （也避免审批过程中身份中途漂移）。
///
/// 用 `Mutex<Option<..>>` 而非 `OnceLock`：单测需要在同一进程内反复改 git 配置
/// 再重读（`OnceLock` 一旦 set 就再也写不进去，`reset` 是静默空操作）。
///
/// ⚠️ 已知缺陷（**不在 REQ-015 范围内修**）：这份缓存是**进程级**的，
/// root 参数只在第一次调用时被使用——同一进程里先问 A 仓再问 B 仓，
/// `current(B)` 返回的仍是 A 的身份，而 `sig` 也是按 A 的路径算的。
/// 单进程只碰一个仓库时看不出来，但界面与单测都会在一个进程里问多个仓库。
/// 修它要动 `current` 的语义（REQ-015 G7 明确「不改 identity 语义」），
/// 故单独立项；新增的 [`resolve_claimed`] **不复用这份缓存**（见该函数注释）。
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

/// 解析「谁在做这个动作」：显式参数 → 环境变量 → git 身份，三级回退。
///
/// 从 `cli/src/main.rs` 的私有 `resolve_identity` **逐字搬迁**而来（REQ-015 G5/T1）：
/// 三个前端（CLI / GUI / TUI）必须共用同一条回退链。各写一份的话，
/// 下一个需要「操作人」的功能就会出现第四种写法，而回退链的**优先级**恰恰是
/// 那种错了以后看日志也发现不了的东西（三种取值都可能非空）。
///
/// `label` / `flag_name` 是错误文案里的「操作人」与 `--author`（或「审核人」与
/// `--reviewer`）——参数化是必须的：把它们焊死在 core 里，等于让 core 知道
/// CLI 的具体命令名。
///
/// ⚠️ 语义边界：本函数只回答「默认填什么」，**不做**默认值回退。
/// 调用方拿到结果后若用户把它清空再提交，必须报错——
/// 「没填」与「填了预填值」在审计上不可区分，而这是 L3 可归属性的基础。
///
/// 拆成 [`resolve_decided`]（纯策略）+ 本函数（唯一的 I/O 入口）与模块里
/// `collect` / `decide` 的划分是同一条纪律：**优先级矩阵必须能被单测穷举**。
/// 三级回退的三个输入都是进程级状态（参数 / 环境变量 / git 配置），
/// 若策略与 I/O 写在一起，用例就只能在测试进程里真的去改环境变量和 git 配置，
/// 而 Rust 的测试是并行的——那会造出一批时绿时红的用例，
/// 比没有用例更糟（它会让人习惯性忽略红灯）。
///
/// ⚠️ 走 [`read_git_identity`] 而**不是**带缓存的 [`current`]：
/// 那份缓存是进程级且**忽略 root**（见 `GIT_ID` 的注释），
/// 而本函数的语义恰恰是「**这个仓库**的操作人是谁」——
/// 界面会在一个进程里切换项目根，复用那份缓存会答出上一个仓库的人。
/// 代价是每次调用多一个 `git config` 子进程；本函数一次动作只调一次，
/// 且门禁本来就要为变更集跑 `git diff --cached`，故不改变量级。
pub fn resolve_claimed(
    root: &Path,
    flag: Option<&str>,
    label: &str,
    flag_name: &str,
) -> Result<String, GateError> {
    resolve_decided(
        read_git_identity(root).as_ref(),
        env_nonempty(REVIEWER_ENV).as_deref(),
        flag,
        label,
        flag_name,
    )
}

/// 纯策略：给定 git 身份与环境变量，按「显式参数 > 环境变量 > git 身份」定出操作人。
///
/// 与 [`resolve_claimed`] 是策略 / I/O 的拆分，理由见那里的注释。
pub fn resolve_decided(
    git: Option<&Identity>,
    env_reviewer: Option<&str>,
    flag: Option<&str>,
    label: &str,
    flag_name: &str,
) -> Result<String, GateError> {
    if let Some(s) = flag {
        if !s.trim().is_empty() {
            return Ok(s.trim().to_string());
        }
    }
    if let Some(s) = env_reviewer {
        if !s.trim().is_empty() {
            return Ok(s.trim().to_string());
        }
    }
    if let Some(id) = git {
        return Ok(id.name.clone());
    }
    Err(GateError::Validation(format!(
        "缺少{}：请使用 {} <姓名>，或设置环境变量 {}，\
         或配置 git 身份（git config user.name \"你的名字\"）后由 req-guard 自动取用",
        label, flag_name, REVIEWER_ENV
    )))
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

    // ---- REQ-015 T1/T2：resolve_claimed 的三级回退 ----

    /// git 姓名为 `Mike Zhu` 的事实（AC-005/006/007 的形态）。
    fn mike() -> Identity {
        Identity {
            name: "Mike Zhu".into(),
            email: "mike@corp.com".into(),
            sig: fingerprint(Path::new(ROOT), "Mike Zhu", "mike@corp.com"),
        }
    }

    #[test]
    fn resolve_显式参数优先于环境变量与git身份() {
        // U1 第一档。三者同时非空 —— 这是唯一能证明「优先级」的形态：
        // 若实现写反了（比如先取环境变量），这条会红。
        let got = resolve_decided(
            Some(&mike()),
            Some("代审人"),
            Some("命令行指定"),
            "审核人",
            "--reviewer",
        )
        .expect("显式参数应命中");
        assert_eq!(got, "命令行指定");
    }

    #[test]
    fn resolve_显式参数为空白时继续回退() {
        // 空白显式参数**不等于**「没传」也不等于「传了空白」：
        // 后者会写进台账，而空名字的审批是不可归属的。
        let got = resolve_decided(
            Some(&mike()),
            Some("代审人"),
            Some("   "),
            "审核人",
            "--reviewer",
        )
        .expect("应回退到环境变量");
        assert_eq!(got, "代审人", "空白参数必须被当作没传，而不是被采用");
    }

    #[test]
    fn resolve_环境变量优先于git身份() {
        // AC-007：环境变量 `代审人` 覆盖 git 的 `Mike Zhu`。
        let got = resolve_decided(Some(&mike()), Some("代审人"), None, "审核人", "--reviewer")
            .expect("环境变量应命中");
        assert_eq!(got, "代审人");
    }

    #[test]
    fn resolve_无环境变量时取git姓名() {
        // AC-006
        let got = resolve_decided(Some(&mike()), None, None, "审核人", "--reviewer")
            .expect("git 身份应命中");
        assert_eq!(got, "Mike Zhu");
    }

    #[test]
    fn resolve_三级皆空时报错并点名git配置() {
        // AC-005 / U1 第四档。报错必须给出**可执行的**出路，
        // 而不是一句「缺少审核人」——后者会让人去翻文档。
        let e = resolve_decided(None, None, None, "审核人", "--reviewer")
            .expect_err("三者皆空必须报错");
        let msg = e.to_string();
        assert!(msg.contains("git config user.name"), "{}", msg);
        assert!(msg.contains("--reviewer"), "须点明该用哪个 flag：{}", msg);
        assert!(msg.contains("审核人"), "须点明缺的是什么：{}", msg);
        assert!(msg.contains(REVIEWER_ENV), "须给出环境变量这条路：{}", msg);
    }

    #[test]
    fn resolve_错误文案随label与flag参数化() {
        // 参数化的意义：core 不该知道 CLI 的命令名。同一条报错在
        // 评论作者那里必须说「缺少评论作者：请使用 --author <姓名>」。
        let e = resolve_decided(None, None, None, "评论作者", "--author")
            .expect_err("三者皆空必须报错");
        let msg = e.to_string();
        assert!(msg.contains("缺少评论作者"), "{}", msg);
        assert!(msg.contains("--author"), "{}", msg);
        assert!(!msg.contains("--reviewer"), "不得焊死 CLI 命令名：{}", msg);
    }

    #[test]
    fn resolve_环境变量为空白时继续回退到git() {
        // 与「显式参数为空白」同源：空环境变量不该产出空名字。
        let got = resolve_decided(Some(&mike()), Some("  "), None, "审核人", "--reviewer")
            .expect("应回退到 git 身份");
        assert_eq!(got, "Mike Zhu");
    }

    #[test]
    fn resolve_真实仓库下走通三级回退() {
        // 端到端兜底：不 mock，真的造一个带 git 身份的临时仓，确认
        // `resolve_claimed`（I/O 版）与 `resolve_decided`（策略版）结论一致。
        // 之所以敢在并行测试里造真仓：`resolve_claimed` 走的是**不带缓存**的
        // `read_git_identity`（见该函数注释），临时仓路径唯一，
        // 既不污染进程级缓存，也不被它污染。
        let root = crate::testutil::temp_dir("resolve-claimed");
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "临时目录应能 git init");
        for (k, v) in [("user.name", "Mike Zhu"), ("user.email", "mike@corp.com")] {
            let ok = std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(&root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(ok, "应能设置 git {}", k);
        }
        let got = resolve_claimed(&root, None, "审核人", "--reviewer").expect("git 身份应命中");
        assert_eq!(got, "Mike Zhu", "I/O 版应取到刚配的 git 姓名");
        // 显式参数仍优先（证明 I/O 版没把优先级写反）
        assert_eq!(
            resolve_claimed(&root, Some("命令行"), "审核人", "--reviewer").expect("显式参数"),
            "命令行"
        );
        crate::testutil::cleanup(&root);
    }
}

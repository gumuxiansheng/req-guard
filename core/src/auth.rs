//! 审批动作鉴权（见《AI工具合规保证规范.md》§4.4）。
//!
//! ## 四种凭据来源（正交叠加）
//! - **方案 A**（软标记）：`req-guard` 给支持会话 env 的工具（Claude Code/CodeBuddy）
//!   注入 `REQ_GUARD_AI_CTX=1`；审批动作检测到即拒。
//!   ⚠️ **白名单式**，且可被 `env -u` 剥离：未登记的工具（如 WorkBuddy）会话内没有该标记，
//!   "没标记 = 人类"即 **fail-open** —— 2026-09-29 Firedit 事故根因。只作开发期加固。
//! - **方案 B**（静态令牌）：`req-guard token issue` 签发短期令牌，原文由人类带外持有。
//!   库文件只存 SHA-256 哈希于 `~/.config/req-guard/guard.cfg`。
//! - **方案 D**（人类在场证明）：在**真实终端**打印一次性挑战码并要求回读。AI 的执行通道
//!   是管道（实测 stdin/stdout 均非 TTY），既看不到交互提示也无法回读 → 天然被拒。
//! - **L3 一次性范围票据**（2026-09-30 新增）：见 [`crate::token`]，绑定需求+步骤、用后即废。
//!
//! ## 审批严格等级（`auth.level`，0–3）——本模块的判定主轴
//! 逐级包含，越高越严；由 [`.gates/req-guard.yaml`](crate::gate::auth_level) 配置，
//! 环境变量 [`AUTH_LEVEL_ENV`] 只能**抬高**不能降低（否则等于给 AI 留了一个降级开关）：
//!
//! | 等级 | 签发（`token issue/revoke`） | 审批（approve/reject/resolve/done/archive/bypass） |
//! | --- | --- | --- |
//! | **L0** | 仅方案 A 软标记 | 启用令牌模式才要求令牌，否则放行（旧行为） |
//! | **L1** | **必须人类在真实终端**（TTY 挑战码） | 无凭据即拒（令牌或 TTY 挑战二选一） |
//! | **L2** | 同 L1；GUI/TUI 走**进程内签发** | L1 + 凭据只认**进程内显式传入**，不读 `REQ_GUARD_TOKEN` 环境变量 |
//! | **L3** | 同 L2，且签发的是一次性票据 | L2 + 凭据必须是**一次性范围票据**（静态令牌不受理） |
//!
//! **为什么 L1 要卡签发**：令牌是方案 B 的信任根。若 `token issue` 无人看守，AI 自签
//! 令牌再自批，方案 B 一招即破（2026-09-29 发现的设计缺陷）。
//! **为什么 L2 要断环境变量**：`export REQ_GUARD_TOKEN=…` 会被同一 shell 会话里的
//! AI 子进程**继承**，等于把令牌交给 AI；改为进程内传入后，凭据不出人类进程。
//! **为什么 L3 用一次性票据**：静态令牌在 TTL 内可被重放、且与审批对象无关。
//!
//! 鉴权点放在 core（而非 CLI），TUI / GUI 与 CLI 自动获得同一约束。

use crate::error::{GateError, Result};
use crate::token::ScopeCheck;
use std::cell::RefCell;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// AI 执行上下文标记环境变量名（方案 A）。
pub const AI_CTX_ENV: &str = "REQ_GUARD_AI_CTX";

/// 带外审批声明环境变量名（方案 C）：进程声明本次审批来自带外渠道。
pub const OOB_DECL_ENV: &str = "REQ_GUARD_OOB";

/// 仅带外审批开关环境变量名（方案 C）：置非空则强制审批必须带 `--oob`/`oob …`。
pub const OOB_ONLY_ENV: &str = "REQ_GUARD_OOB_ONLY";

/// 严格审批模式开关（兼容旧配置）：置非空等价于 `auth.level >= 1`。
///
/// 保留它是因为存量脚本/文档在用它；新项目请直接用 `auth.level`。
pub const STRICT_ENV: &str = "REQ_GUARD_STRICT_AUTH";

/// 审批严格等级覆盖（0–3）。**只能抬高**：取 `max(yaml, env)`，
/// 这样即使 AI 设置了它也无法把项目的严格等级降下来。
pub const AUTH_LEVEL_ENV: &str = "REQ_GUARD_AUTH_LEVEL";

thread_local! {
    /// 进程内审批凭据（L2 起唯一受认可的来源）。
    ///
    /// 为什么不直接读环境变量：环境变量会被同一 shell 会话中的 AI 子进程**继承**，
    /// 人类一旦 `export REQ_GUARD_TOKEN`，AI 便与人类同权。改为由前端在自己进程内
    /// 显式设置（CLI 从 `--token` 参数、GUI/TUI 从内存字段），凭据不出人类进程。
    static CREDENTIAL: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// 设置本次审批使用的凭据（进程内，空串视为清除）。
pub fn set_credential(v: Option<String>) {
    CREDENTIAL.with(|c| *c.borrow_mut() = v.filter(|s| !s.trim().is_empty()));
}

/// 清除进程内凭据（GUI 每次动作后调用：L3 票据是一次性的，用完即弃）。
pub fn clear_credential() {
    set_credential(None);
}

/// 当前进程内凭据（**不含**环境变量回退）。
pub fn credential() -> Option<String> {
    CREDENTIAL.with(|c| c.borrow().clone())
}

/// 本次审批可用的凭据：优先进程内，L0–L1 才允许回退到 [`crate::token::TOKEN_ENV`]。
fn provided_credential(level: u8) -> Option<String> {
    if let Some(v) = credential() {
        return Some(v);
    }
    if level < 2 {
        return std::env::var(crate::token::TOKEN_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty());
    }
    None
}

/// 按给定标记取值判断是否处于 AI 执行上下文（存在且非空即视为是）。
fn is_ai_ctx_value(v: Option<&str>) -> bool {
    v.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// 当前进程是否处于 AI 执行上下文（方案 A 判定）。
pub fn is_ai_context() -> bool {
    is_ai_ctx_value(std::env::var(AI_CTX_ENV).ok().as_deref())
}

/// 当前进程是否处于**真实交互终端**（方案 D 的物理前提）。
///
/// 判 stdin **与** stdout：AI/脚本执行时两者都是管道，回读挑战码必然落空；
/// 只判 stdout 会让 `echo 码 | req-guard approve` 这类"喂答案"通过。
pub fn is_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// 本次生效的审批严格等级（0–3，见模块文档）。
///
/// 取值规则：`yaml auth.level`（兼容旧 `auth.strict` → 1），再与
/// [`STRICT_ENV`]（→ 至少 1）、[`AUTH_LEVEL_ENV`] 取 **max**。
/// 环境变量只能抬高，避免成为 AI 的降级开关。
pub fn effective_level(root: &Path) -> u8 {
    let mut lvl = crate::gate::auth_level(root);
    if std::env::var(STRICT_ENV)
        .ok()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        lvl = lvl.max(1);
    }
    if let Ok(v) = std::env::var(AUTH_LEVEL_ENV) {
        if let Ok(n) = v.trim().parse::<u8>() {
            lvl = lvl.max(n.min(crate::gate::AUTH_LEVEL_MAX));
        }
    }
    lvl
}

/// 本次进程的审批环境事实（可注入，便于单测不碰进程环境）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthFacts {
    /// 方案 A：是否处于 AI 执行上下文。
    pub ai_ctx: bool,
    /// 凭据库中是否存在**生效中**的凭据（决定 L0 是否强制凭据）。
    pub token_mode: bool,
    /// 本次生效的严格等级（0–3）。
    pub level: u8,
    /// 是否在真实终端（L1+ 的备用人类证明）。
    pub tty: bool,
}

impl AuthFacts {
    /// 从进程环境采集（生产路径）。
    pub fn capture(root: &Path) -> Self {
        Self {
            ai_ctx: is_ai_context(),
            token_mode: crate::token::token_mode(),
            level: effective_level(root),
            tty: is_tty(),
        }
    }
}

/// 审批类动作的入口守卫。
///
/// 供 [`crate::requirement::review`]（approve/reject）、[`crate::comment::resolve`]、
/// [`crate::requirement::done`]、[`crate::requirement::archive_due`]、
/// [`crate::gate::bypass`] 在入口调用；`action` 用于错误提示点名动作，
/// `scope` 用于 L3 票据的范围绑定校验（见 [`crate::token::ScopeCheck`]）。
pub fn ensure_human(action: &str, root: &Path, scope: ScopeCheck<'_>) -> Result<()> {
    let f = AuthFacts::capture(root);
    authorize(
        &f,
        action,
        || credential_ok(f.level, scope),
        || prove_presence(action),
    )?;
    check_oob_forced(action)
}

/// 凭据是否有效（形态随等级收紧：L3 只认一次性范围票据）。
fn credential_ok(level: u8, scope: ScopeCheck<'_>) -> bool {
    let Some(cred) = provided_credential(level) else {
        return false;
    };
    match crate::token::load() {
        // L3 起静态令牌不受理——TTL 内可重放、且不绑定审批对象
        Some(cfg) if cfg.mode == crate::token::Mode::Static => {
            level < 3 && crate::token::verify_static(&cred)
        }
        Some(_) => crate::token::verify_scoped(&cred, scope),
        None => false,
    }
}

/// 凭据**管理**（重签/撤销）时"出示当前凭据"的校验：只看身份，**不看形态与范围**。
///
/// 与 [`credential_ok`] 分开的理由：L3 起审批只认一次性票据，但库中可能还躺着一枚
/// 升级前签发的 `static` 凭据。若管理通道也按"L3 只认票据"来判，人类就**再也拿不出**
/// 这枚凭据、`token issue/revoke` 全部锁死、没有任何出口。持有当前凭据本身就是授权，
/// 与它是什么形态无关，故这里两种形态都接受。
fn admin_credential_ok(level: u8) -> bool {
    let Some(cred) = provided_credential(level) else {
        return false;
    };
    match crate::token::load() {
        Some(cfg) if cfg.mode == crate::token::Mode::Static => crate::token::verify_static(&cred),
        Some(_) => crate::token::verify_scoped(&cred, ScopeCheck::Any),
        None => false,
    }
}

/// 审批动作成功后**消费**一次性票据（L3）。
///
/// 返回是否确实消费了票据。只在库中存的是 `scoped` 票据时生效；
/// 必须在审批结果**落盘成功后**调用，否则人类会白丢一张票。
pub fn consume_credential_if_scoped() -> bool {
    if crate::token::load()
        .map(|c| c.mode == crate::token::Mode::Scoped)
        .unwrap_or(false)
    {
        return crate::token::consume();
    }
    false
}

/// 交互式界面（GUI / TUI）签发凭据的默认有效期（分钟）。
///
/// 界面凭据是"点一下、用一次"的短命凭据，无需长 TTL；留一点余量只是为了让
/// 同一次点击里的多个判定共用它。
const UI_CRED_TTL_MIN: u64 = 10;

/// 交互式界面的**进程内签发**：以"人类亲手操作界面"作为在场证明。
///
/// 这是方案 2 的核心：GUI / TUI 是人类亲手启动并点击的界面进程，AI 既无法凭空
/// 拉起一个带人类交互的界面，也无法点击按钮——因此界面里的确认动作本身就是
/// 在场证明，签发与持有**全程只在人类进程内存里**，不经 stdout、不经环境变量、
/// 不落剪贴板，AI 没有任何命令可以"执行一下然后读到它"。
///
/// - **L0**：不做任何事（返回 `false`），保持旧行为；
/// - **L1–L2**：进程内签发短 TTL 静态凭据并设为本次凭据；
/// - **L3**：进程内签发一次性范围票据（`scope` 绑定需求+步骤）。
///
/// ⚠️ 会覆盖库中现有凭据——界面与 CLI 同属"人类信任级别"（都能做出人类决策），
/// 而 AI 会话在入口就被 [`is_ai_context`] 拒掉，不构成自签通道。
/// 调用方应在动作结束后调用 [`clear_credential`]。
pub fn ui_issue_credential(root: &Path, scope: &str) -> Result<bool> {
    let level = effective_level(root);
    if is_ai_context() {
        return Err(ai_ctx_err("界面审批（签发凭据）"));
    }
    if level == 0 {
        return Ok(false);
    }
    let raw = if level >= 3 {
        crate::token::issue_scoped(UI_CRED_TTL_MIN, scope)?.0
    } else {
        crate::token::issue(UI_CRED_TTL_MIN)?.0
    };
    set_credential(Some(raw));
    Ok(true)
}

/// 判级核心（事实可注入，供单测使用）。
///
/// `verify_cred` 在校验凭据时才被调用；`prove` 只在"L1+ 且真实终端"时才被调用，
/// 避免非交互场景下空等输入（CI / AI 会话里读 stdin 会立刻 EOF）。
pub fn authorize(
    f: &AuthFacts,
    action: &str,
    verify_cred: impl FnOnce() -> bool,
    prove: impl FnOnce() -> bool,
) -> Result<()> {
    // 1) 方案 A：AI 上下文直拒（最廉价的已知场景，任何等级都生效）
    if f.ai_ctx {
        return Err(ai_ctx_err(action));
    }
    // 2) L0：保持旧行为——仅在启用凭据模式时才强制凭据
    if f.level == 0 {
        if f.token_mode {
            return if verify_cred() {
                Ok(())
            } else {
                Err(token_err(action))
            };
        }
        return Ok(());
    }
    // 3) L1+：无凭据即拒（把 fail-open 反转成 fail-closed）
    if verify_cred() {
        return Ok(());
    }
    // 4) L1+ 备用：人类在真实终端输入一次性挑战码
    if f.tty && prove() {
        return Ok(());
    }
    Err(level_err(action, f))
}

/// 凭据管理动作（`token issue` / `token revoke`）的入口守卫。
///
/// 为什么单独一条：凭据是方案 B 的信任根，若 `token issue` 无人看守，AI 可自签凭据
/// 再自批，方案 B 一招即破（2026-09-29 发现的设计缺陷）。规则：
/// - AI 上下文 → 拒；
/// - 本机已有生效凭据 → **必须出示当前凭据**才能重签/撤销（阻止静默替换）；
/// - 尚无生效凭据且 `level >= 1` → **必须人类在场**（TTY 挑战），管道通道一律签不出来。
///
/// 注：GUI / TUI 的进程内签发**不走本函数**——它们是人类亲手启动的界面进程，
/// 由界面里的确认弹窗充当在场证明（见 [`crate::token::issue_scoped`] 的调用方）。
pub fn ensure_token_admin(action: &str, root: &Path) -> Result<()> {
    let level = effective_level(root);
    if is_ai_context() {
        return Err(ai_ctx_err(action));
    }
    if crate::token::token_mode() {
        return if admin_credential_ok(level) {
            Ok(())
        } else {
            Err(GateError::Validation(format!(
                "{}：本机已存在生效审批凭据，重签/撤销必须出示当前凭据（--token <当前凭据>）。\n\
                 该限制用于阻止 AI 会话静默替换凭据后自批。",
                action
            )))
        };
    }
    if level >= 1 {
        return if is_tty() && prove_presence(action) {
            Ok(())
        } else {
            Err(admin_err(action, level, is_tty()))
        };
    }
    Ok(())
}

/// L1+ 下"签发/撤销凭据"被拒的提示（与审批被拒区分开：这里的解法是换到真实终端）。
fn admin_err(action: &str, level: u8, tty: bool) -> GateError {
    if tty {
        return GateError::Validation(format!(
            "{}：挑战码不匹配，无法确认操作者是人类，已拒绝。",
            action
        ));
    }
    GateError::Validation(format!(
        "{}：本机审批严格等级 L{}，签发/撤销审批凭据必须由人类在**真实终端**执行\
         （终端会要求输入一次性挑战码）。\n\
         当前通道不是交互终端（AI 会话 / 脚本 / CI 均属此类），已拒绝——\
         这正是阻断\"AI 自签凭据→自批\"的关键一环。\n\
         若你是人类：请在本机终端里重跑该命令；若项目已开 GUI，也可在界面里直接审批。",
        action, level
    ))
}

/// 生成 6 位挑战码（人类可读，方案 D）。
fn challenge_code() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let mix = nanos
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(pid)
        .wrapping_add(1_442_695_040_888_963_407);
    format!("{:06}", mix % 1_000_000)
}

/// 人类在场证明（方案 D）：在终端打印挑战码并回读比对。
///
/// 非 TTY 直接 false（AI 通道读不到输入，`read_line` 会立即 EOF；
/// 显式短路避免任何阻塞）。
pub fn prove_presence(action: &str) -> bool {
    if !is_tty() {
        return false;
    }
    let code = challenge_code();
    println!(
        "{} 属于审批类动作，需要确认操作者是人类（方案 D 在场证明）",
        action
    );
    println!("   挑战码：{}", code);
    print!("   请在终端输入上面的挑战码后回车：");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(_) => line.trim() == code,
        Err(_) => false,
    }
}

/// 审计上下文指纹：让事后能判定审批发生在**什么通道**。
///
/// 事故复盘的关键：仅有 `channel=interactive` 无法区分"人类终端"与"AI 会话"，
/// 追加 `tty=`/`ai=` 两位指纹后，AI 代跑（`tty=0`）在台账里一目了然。
pub fn audit_ctx() -> String {
    format!(
        "tty={} ai={}",
        if is_tty() { 1 } else { 0 },
        if is_ai_context() { 1 } else { 0 }
    )
}

/// 方案 A 拒绝信息。
fn ai_ctx_err(action: &str) -> GateError {
    GateError::Validation(format!(
        "{} 属于审批类动作，禁止在 AI 执行上下文内执行（检测到 {} 非空）。\n\
         审批须由真实审核人在 AI 会话之外的独立终端操作。\n\
         若你是被误拦的人类：请在不带该环境变量的终端重试，或先 unset {}。",
        action, AI_CTX_ENV, AI_CTX_ENV
    ))
}

/// L0 凭据模式拒绝信息。
fn token_err(action: &str) -> GateError {
    GateError::Validation(format!(
        "{} 属于审批类动作：本机已启用审批凭据，必须携带有效凭据。\n\
         请在命令中追加 --token <凭据>，或设置环境变量 {}；\n\
         凭据由真实审核人带外持有（req-guard token issue 签发），AI 上下文拿不到。",
        action,
        crate::token::TOKEN_ENV
    ))
}

/// L1+ 拒绝信息。`tty` 为真说明挑战码输错，为假说明通道本身不是终端。
fn level_err(action: &str, f: &AuthFacts) -> GateError {
    if f.level >= 3 {
        return GateError::Validation(format!(
            "{} 属于审批类动作：本机审批严格等级 L{}（auth.level），必须出示**一次性范围票据**。\n\
             获取票据：在真实终端执行 req-guard token issue（可加 --req <需求ID> --step <步骤> 绑定对象），\n\
             再把打印出的原文用 --token <票据> 提交；同票据用后即废，不可重放。\n\
             也可直接在 GUI 管理台里审批（界面进程内签发并内存持有票据）。",
            action, f.level
        ));
    }
    if f.tty {
        return GateError::Validation(format!(
            "{}：挑战码不匹配，无法确认操作者是人类，已拒绝。",
            action
        ));
    }
    GateError::Validation(format!(
        "{} 属于审批类动作：本机审批严格等级 L{}（auth.level / {} / {}），\
         当前通道不是交互终端（AI 会话 / 脚本 / CI 均属此类），无法证明操作者是人类。\n\
         请选择其一：\n\
         1) 在真实终端里执行（会要求输入一次性挑战码）；\n\
         2) 用审批凭据：req-guard token issue 后追加 --token <凭据>；\n\
         3) 用 GUI 管理台审批（界面进程内签发并内存持有凭据）。",
        action, f.level, STRICT_ENV, AUTH_LEVEL_ENV
    ))
}

/// 方案 C：当前进程是否**已声明带外渠道**（`--oob` / `req-guard oob …`）。
fn declared_oob() -> bool {
    std::env::var(OOB_DECL_ENV)
        .ok()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// 方案 C 判定的可测核心：`oob_only` 为"仅带外开关是否开启"，`declared` 为
/// "本次审批是否已声明带外渠道"。仅当开关开启且未声明才拒绝。
fn check_oob_forced_with(oob_only: bool, declared: bool, action: &str) -> Result<()> {
    if oob_only && !declared {
        return Err(GateError::Validation(format!(
            "{} 属于审批类动作：本机已开启\"仅带外审批\"（{}），\
             审批必须在带外渠道执行并显式声明。\n\
             请在命令中加 --oob，或改用它：req-guard oob <命令>。",
            action, OOB_ONLY_ENV
        )));
    }
    Ok(())
}

/// 方案 C 入口（读进程环境）。
fn check_oob_forced(action: &str) -> Result<()> {
    let oob_only = std::env::var(OOB_ONLY_ENV)
        .ok()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    check_oob_forced_with(oob_only, declared_oob(), action)
}

/// 本次审批的**渠道标注**（供审计台账 `channel=` 使用）：`oob` 或 `interactive`。
pub fn declared_channel() -> &'static str {
    if declared_oob() {
        "oob"
    } else {
        "interactive"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(ai: bool, token_mode: bool, level: u8, tty: bool) -> AuthFacts {
        AuthFacts {
            ai_ctx: ai,
            token_mode,
            level,
            tty,
        }
    }

    #[test]
    fn 方案a_ai上下文一律拒() {
        // 无论等级、是否 TTY，AI 上下文都直拒
        for level in 0..=3u8 {
            for tty in [false, true] {
                let e = authorize(&facts(true, false, level, tty), "approve", || true, || true)
                    .unwrap_err();
                assert!(
                    e.to_string().contains(AI_CTX_ENV),
                    "提示须给出变量名：{}",
                    e
                );
            }
        }
        assert!(
            authorize(
                &facts(false, false, 0, false),
                "approve",
                || false,
                || false
            )
            .is_ok(),
            "L0 且无凭据模式 → 旧行为放行"
        );
    }

    #[test]
    fn l0_启用凭据后无凭据即拒() {
        let e = authorize(&facts(false, true, 0, true), "approve", || false, || true).unwrap_err();
        assert!(e.to_string().contains(crate::token::TOKEN_ENV));
        assert!(
            authorize(&facts(false, true, 0, false), "resolve", || true, || false).is_ok(),
            "凭据是充分条件（无需 TTY）"
        );
    }

    #[test]
    fn l1起无凭据即拒_ai标记缺失也拦() {
        // 核心修复：未登记工具（无 AI 标记）+ 非 TTY → L1+ 必须拒；
        // 这正是 WorkBuddy 自批事故的情形（旧逻辑会放行）。
        for level in 1..=3u8 {
            let e = authorize(
                &facts(false, false, level, false),
                "approve",
                || false,
                || true,
            )
            .unwrap_err();
            let msg = e.to_string();
            assert!(msg.contains("approve"), "提示须点名动作：{}", msg);
            assert!(
                msg.contains(&format!("L{}", level)),
                "提示须给出等级：{}",
                msg
            );
            if level < 3 {
                assert!(msg.contains("交互终端"), "须说明拒绝原因：{}", msg);
            } else {
                assert!(msg.contains("一次性"), "L3 须说明凭据形态：{}", msg);
            }

            // 凭据有效 → 放行（哪怕非 TTY）
            assert!(authorize(
                &facts(false, false, level, false),
                "approve",
                || true,
                || false
            )
            .is_ok());
            // 无凭据 + TTY + 挑战通过 → 放行
            assert!(authorize(
                &facts(false, false, level, true),
                "approve",
                || false,
                || true
            )
            .is_ok());
            // 无凭据 + TTY + 挑战失败 → 拒
            assert!(authorize(
                &facts(false, false, level, true),
                "approve",
                || false,
                || false
            )
            .is_err());
        }
    }

    #[test]
    fn prove_只在需要时被调用() {
        // 非 TTY 场景：prove 不得被调用（否则会在 CI / AI 会话里阻塞读 stdin）
        let mut called = false;
        let _ = authorize(
            &facts(false, false, 3, false),
            "approve",
            || false,
            || {
                called = true;
                true
            },
        );
        assert!(!called, "非交互通道不得触发挑战码输入");
        // 凭据已通过时也不得再问挑战码（人类只答一次）
        let mut called2 = false;
        let _ = authorize(
            &facts(false, false, 3, true),
            "approve",
            || true,
            || {
                called2 = true;
                true
            },
        );
        assert!(!called2, "凭据有效时不应重复要求在场证明");
    }

    #[test]
    fn 挑战码格式稳定() {
        let a = challenge_code();
        assert_eq!(a.len(), 6, "6 位挑战码：{}", a);
        assert!(a.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn is_ai_context_读进程环境() {
        // 不改写进程环境变量（并行测试有竞态），只验证缺省路径的稳定性
        if std::env::var(AI_CTX_ENV).is_err() {
            assert!(!is_ai_context());
        }
    }

    #[test]
    fn 审计指纹标记通道() {
        let ctx = audit_ctx();
        assert!(ctx.starts_with("tty="), "指纹须含 tty：{}", ctx);
        assert!(ctx.contains("ai="), "指纹须含 ai：{}", ctx);
        assert_eq!(ctx.len(), "tty=0 ai=0".len());
    }

    #[test]
    fn 进程内凭据可设置与清除() {
        assert!(credential().is_none(), "初始不得有凭据");
        set_credential(Some("  ".into()));
        assert!(credential().is_none(), "空白串视为清除");
        set_credential(Some("abc".into()));
        assert_eq!(credential().as_deref(), Some("abc"));
        clear_credential();
        assert!(credential().is_none());
    }

    #[test]
    fn l2起不再读环境变量() {
        // 直接验证取凭据的口径：L2+ 时即使环境变量有值也不采纳
        // （用进程内凭据做对照，避免改写进程环境变量引发并行竞态）
        set_credential(Some("from-tls".into()));
        assert_eq!(provided_credential(2).as_deref(), Some("from-tls"));
        clear_credential();
        assert!(provided_credential(2).is_none(), "L2 无进程内凭据即无凭据");
        assert!(provided_credential(3).is_none());
    }

    #[test]
    fn ui签发在l0为无操作() {
        // 无 .gates/req-guard.yaml → L0：不得开凭据通道，更不得写 guard.cfg
        let root = crate::testutil::temp_dir("ui-cred-l0");
        assert!(!ui_issue_credential(&root, "REQ-001:decomposition").unwrap());
        assert!(credential().is_none(), "L0 不得留下进程内凭据");
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn 仅带外模式未声明即拒() {
        let e = check_oob_forced_with(true, false, "approve").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("approve"));
        assert!(msg.contains(OOB_ONLY_ENV));
        assert!(msg.contains("--oob"));

        assert!(check_oob_forced_with(true, true, "approve").is_ok());
        assert!(check_oob_forced_with(false, false, "approve").is_ok());
        assert!(check_oob_forced_with(false, true, "approve").is_ok());
    }

    #[test]
    fn 动作名透传各审批入口() {
        for act in ["approve", "reject", "resolve", "bypass", "done"] {
            let e = authorize(&facts(true, false, 3, false), act, || false, || false).unwrap_err();
            assert!(e.to_string().contains(act), "{} 须点名动作", act);
        }
        for act in ["approve", "resolve", "bypass"] {
            let e = authorize(&facts(false, false, 3, false), act, || false, || false).unwrap_err();
            assert!(e.to_string().contains(act), "{} 须点名动作", act);
        }
    }
}

//! 审批动作鉴权（见《AI工具合规保证规范.md》§4.4）。
//!
//! 两个强度层级，逐级增强：
//! - **方案 A**（软标记）：`req-guard` 给支持会话 env 的工具（Claude Code/CodeBuddy）
//!   注入 `REQ_GUARD_AI_CTX=1`；`approve/reject/resolve/bypass` 检测到即拒。
//!   可被 `env -u` 剥离，只作开发期加固。
//! - **方案 B**（reviewer token，本模块主导）：`req-guard token issue` 签发短期令牌，
//!   原文由人类带外持有（存入密码管理器/自身会话）。库文件只存 SHA-256 哈希于
//!   `~/.config/req-guard/guard.cfg`。一旦**启用令牌**（[`crate::token::load()`] 命中），
//!   审批必须携带有效令牌——AI 不知道令牌值，无法自批。
//! - **方案 C**（带外审批，流程化）：审批必须**显式声明来自带外渠道**（`--oob` /
//!   `req-guard oob …`）。可全局关闭 `REQ_GUARD_OOB_ONLY=1` 强制"仅接受带外审批"。
//!   零依赖下不引入非对称签名；其隔离与留痕通过"渠道声明 + 审计台账 `channel=` 标注"实现。
//!
//! ## 判级（三者正交叠加）
//! [`ensure_human`] 依次要求：
//! 1. 令牌或非 AI 上下文可通过（B / A）；
//! 2. 若 `REQ_GUARD_OOB_ONLY=1` → 必须已声明带外渠道（C 强制）。
//!
//! 鉴权点放在 core（而非 CLI），TUI / GUI 与 CLI 自动获得同一约束。

use crate::error::{GateError, Result};

/// AI 执行上下文标记环境变量名（方案 A）。
pub const AI_CTX_ENV: &str = "REQ_GUARD_AI_CTX";

/// 带外审批声明环境变量名（方案 C）：进程声明本次审批来自带外渠道。
pub const OOB_DECL_ENV: &str = "REQ_GUARD_OOB";

/// 仅带外审批开关环境变量名（方案 C）：置非空则强制审批必须带 `--oob`/`oob …`。
pub const OOB_ONLY_ENV: &str = "REQ_GUARD_OOB_ONLY";

/// 按给定标记取值判断是否处于 AI 执行上下文（存在且非空即视为是）。
fn is_ai_ctx_value(v: Option<&str>) -> bool {
    v.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// 当前进程是否处于 AI 执行上下文（方案 A 判定）。
pub fn is_ai_context() -> bool {
    is_ai_ctx_value(std::env::var(AI_CTX_ENV).ok().as_deref())
}

/// 审批类动作的入口守卫。
///
/// 供 [`crate::requirement::review`]（approve/reject）、[`crate::comment::resolve`]、
/// [`crate::gate::bypass`] 在入口调用；`action` 用于错误提示点名动作。
/// 依次要求：主鉴权（令牌 B 或方案 A），再由方案 C 判定（若仅带外开关开启）。
pub fn ensure_human(action: &str) -> Result<()> {
    ensure_human_gate(action)?;
    check_oob_forced(action)
}

/// 主鉴权：令牌模式优先，未启用则回退方案 A。
fn ensure_human_gate(action: &str) -> Result<()> {
    if crate::token::token_mode() {
        return guard_token(action);
    }
    // 方案 A：软标记拒绝
    ensure_human_with(std::env::var(AI_CTX_ENV).ok().as_deref(), action)
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

/// 令牌模式守卫：必须携带有效 `REQ_GUARD_TOKEN`，否则拒绝（fail-closed）。
fn guard_token(action: &str) -> Result<()> {
    let provided = std::env::var(crate::token::TOKEN_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    guard_token_with(provided, action)
}

/// [`guard_token`] 的可测形式：令牌取值由参数注入，避免测试竞改进程环境变量。
fn guard_token_with(provided: Option<String>, action: &str) -> Result<()> {
    match provided {
        Some(t) if crate::token::verify(&t) => Ok(()),
        Some(_) => Err(GateError::Validation(format!(
            "{}：提供的审批令牌无效或已过期（{}），请重新签发：req-guard token issue",
            action,
            crate::token::TOKEN_ENV
        ))),
        None => Err(GateError::Validation(format!(
            "{} 属于审批类动作：本机已启用审批令牌（方案 B），必须携带有效令牌。\n\
             请在命令中追加 --token <令牌>，或设置环境变量 {}；\n\
             令牌由真实审核人带外持有（req-guard token issue 签发），AI 上下文拿不到。",
            action,
            crate::token::TOKEN_ENV
        ))),
    }
}

/// [`ensure_human`] 方案 A 的可测形式：标记取值由参数注入，避免测试竞改进程环境变量。
fn ensure_human_with(ctx: Option<&str>, action: &str) -> Result<()> {
    if is_ai_ctx_value(ctx) {
        return Err(GateError::Validation(format!(
            "{} 属于审批类动作，禁止在 AI 执行上下文内执行（检测到 {} 非空）。\n\
             审批须由真实审核人在 AI 会话之外的独立终端操作。\n\
             若你是被误拦的人类：请在不带该环境变量的终端重试，或先 unset {}。",
            action, AI_CTX_ENV, AI_CTX_ENV
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_human_按标记取值裁决() {
        // 无标记 → 放行（人类终端的常态）
        assert!(ensure_human_with(None, "approve").is_ok());
        // 空串 → 不构成 AI 上下文
        assert!(ensure_human_with(Some(""), "approve").is_ok());
        assert!(ensure_human_with(Some("   "), "approve").is_ok());

        // 标记非空 → 拒绝（AI 经 Shell 自批被堵）
        for v in ["1", "true", "claude-code"] {
            let e = ensure_human_with(Some(v), "approve").unwrap_err();
            let msg = e.to_string();
            assert!(msg.contains("approve"), "提示须点名动作：{}", msg);
            assert!(msg.contains(AI_CTX_ENV), "提示须给出变量名：{}", msg);
            assert!(msg.contains("独立终端"), "提示须指明人类操作路径：{}", msg);
        }

        // 动作名透传（bypass / resolve / reject 同受约束）
        assert!(ensure_human_with(Some("1"), "bypass")
            .unwrap_err()
            .to_string()
            .contains("bypass"));
        assert!(ensure_human_with(Some("1"), "resolve")
            .unwrap_err()
            .to_string()
            .contains("resolve"));
    }

    #[test]
    fn is_ai_context_读进程环境() {
        // 不改写进程环境变量（并行测试有竞态），只验证缺省路径的稳定性：
        // 开发机/CI 正常不应设置该标记；若人为设置，此处断言即应失败以提醒。
        if std::env::var(AI_CTX_ENV).is_err() {
            assert!(!is_ai_context());
        }
    }

    #[test]
    fn 令牌模式下无token即拒() {
        // 无令牌 → 提示必须签发/携带（fail-closed）
        let e = guard_token_with(None, "approve").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("approve"));
        assert!(msg.contains(crate::token::TOKEN_ENV));
        assert!(msg.contains("方案 B"));

        // 提供了错误令牌 → 报无效/过期（真实 verify 依赖 HOME，此处只验错误提示形态）
        let e = guard_token_with(Some("deadbeef".to_string()), "resolve").unwrap_err();
        assert!(e.to_string().contains("RESOLVE") || e.to_string().contains("resolve"));
    }

    #[test]
    fn 仅带外模式未声明即拒() {
        // 开关开启 + 未声明 → 拒绝并提示带外渠道
        let e = check_oob_forced_with(true, false, "approve").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("approve"));
        assert!(msg.contains(OOB_ONLY_ENV));
        assert!(msg.contains("--oob"));

        // 开关开启 + 已声明 → 通过
        assert!(check_oob_forced_with(true, true, "approve").is_ok());
        // 开关关闭 → 无论是否声明都通过
        assert!(check_oob_forced_with(false, false, "approve").is_ok());
        assert!(check_oob_forced_with(false, true, "approve").is_ok());
    }
}

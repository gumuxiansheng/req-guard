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
//!
//! ## 判级
//! [`ensure_human`] 行为：
//! - 已启用令牌（[`crate::token::token_mode()`]）→ 必须校验 `REQ_GUARD_TOKEN`；
//! - 未启用令牌 → 回退方案 A（`REQ_GUARD_AI_CTX` 软标记拒绝）。
//!
//! 鉴权点放在 core（而非 CLI），TUI / GUI 与 CLI 自动获得同一约束。

use crate::error::{GateError, Result};

/// AI 执行上下文标记环境变量名（方案 A）。
pub const AI_CTX_ENV: &str = "REQ_GUARD_AI_CTX";

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
/// 令牌模式优先；未启用令牌时回退方案 A。
pub fn ensure_human(action: &str) -> Result<()> {
    if crate::token::token_mode() {
        return guard_token(action);
    }
    // 方案 A：软标记拒绝
    ensure_human_with(std::env::var(AI_CTX_ENV).ok().as_deref(), action)
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
}

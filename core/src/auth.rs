//! 审批动作鉴权：`REQ_GUARD_AI_CTX` 软标记（方案 A，见《AI工具合规保证规范.md》§4.4）。
//!
//! ## 为什么需要
//! `--reviewer` / `--author` 只是字符串，不构成身份保证：AI 经 Shell 自调
//! `req-guard approve ... --reviewer 寇工`（×3）即可完成**自批**，
//! 再用 `resolve --author 寇工` 关掉阻塞评论——L2/L3 两道墙只认状态，
//! 会被欺诈性 `approved` 照单全收。审批动作必须鉴权。
//!
//! ## 机制
//! [`crate::gate`] 注入 AI 工具配置时，为支持会话级环境变量的工具（如
//! Claude Code `settings.json` 的 `env` 段）写入 `"REQ_GUARD_AI_CTX": "1"`，
//! 使该工具会话内（含其 Shell 工具）都带着此标记；
//! `approve / reject / resolve / bypass` 检测到标记即拒绝执行。
//! 审核人须在 **AI 会话之外的独立终端** 完成审批。
//!
//! ## 边界（诚实披露）
//! 软标记可被 `env -u REQ_GUARD_AI_CTX` 剥离——方案 A 只提高门槛，
//! 不是密码学保证；生产环境应升级方案 B（reviewer token）/ C（带外审批）。
//!
//! 鉴权点放在 core（而非 CLI），TUI / GUI 与 CLI 自动获得同一约束。

use crate::error::{GateError, Result};

/// AI 执行上下文标记环境变量名。
pub const AI_CTX_ENV: &str = "REQ_GUARD_AI_CTX";

/// 按给定标记取值判断是否处于 AI 执行上下文（存在且非空即视为是）。
fn is_ai_ctx_value(v: Option<&str>) -> bool {
    v.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// 当前进程是否处于 AI 执行上下文。
pub fn is_ai_context() -> bool {
    is_ai_ctx_value(std::env::var(AI_CTX_ENV).ok().as_deref())
}

/// 审批类动作的入口守卫：AI 上下文内直接拒绝。
///
/// 供 [`crate::requirement::review`]（approve/reject）、[`crate::comment::resolve`]、
/// [`crate::gate::bypass`] 在入口调用；`action` 用于错误提示点名动作。
pub fn ensure_human(action: &str) -> Result<()> {
    ensure_human_with(std::env::var(AI_CTX_ENV).ok().as_deref(), action)
}

/// [`ensure_human`] 的可测形式：标记取值由参数注入，避免测试竞改进程环境变量。
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
}

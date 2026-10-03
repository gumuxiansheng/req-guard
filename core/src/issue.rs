//! 通用问题模型：严重级。
//!
//! **为什么从 `idcheck` 迁出来**：编号检查（`ids --check`）、AC 校验（`ac check`）、
//! 变更范围校验（`touch-check`）都要产出「一堆带严重级的问题」，而 `Severity` 的语义
//! （Error 使退出码非 0、仅 Warn 放行）是**跨三类检查共用的同一套约定**。
//! 各模块各复制一份 enum，就等于允许"同一个词在两处含义不同" —— 对门禁工具不可接受。
//!
//! **只放 `Severity`，不放泛型 `Issue { kind: String }`**：kind 必须在各检查内保持
//! 枚举类型（`AcIssueKind` / `TouchIssueKind` / `IdIssueKind`），否则调用方要写字符串比较，
//! 编译器不再能兜住拼写错误。宁可三处各有一个 `XxxIssue` 结构体，也不引入丢失类型安全的泛型。

/// 问题严重级：`Error` 使所属检查的退出码非 0；`Warn` 仅提示不阻断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Warn => "警告",
            Severity::Error => "错误",
        }
    }

    pub fn is_error(self) -> bool {
        matches!(self, Severity::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::Severity;

    #[test]
    fn severity_错误为_error_且中文标签固定() {
        assert!(Severity::Error.is_error());
        assert_eq!(Severity::Error.as_str(), "错误");
        assert!(!Severity::Warn.is_error());
        assert_eq!(Severity::Warn.as_str(), "警告");
    }
}

//! 统一错误类型与结果别名。
//!
//! 设计原则：所有可失败路径都收敛到 [`GateError`]，由 `main` 统一打印并以非零退出码结束。
//! 错误信息使用中文，面向最终用户（测试经理 / 研发），要求"结论先行 + 可操作提示"。

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

#[derive(Debug)]
pub enum GateError {
    /// 文件系统 IO 错误，携带涉及路径。
    Io {
        path: Option<PathBuf>,
        source: io::Error,
    },
    /// 参数 / 语义校验失败（用户侧可修复）。
    Validation(String),
    /// 外部命令（拦截脚本）失败。
    External {
        command: String,
        status: Option<ExitStatus>,
        stderr: String,
    },
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateError::Io { path, source } => match path {
                Some(p) => write!(f, "IO 错误 ({}): {}", p.display(), source),
                None => write!(f, "IO 错误: {}", source),
            },
            GateError::Validation(m) => write!(f, "参数校验失败: {}", m),
            GateError::External {
                command,
                status,
                stderr,
            } => {
                let st = match status {
                    Some(s) => format!(" 退出码 {}", s),
                    None => String::new(),
                };
                if stderr.is_empty() {
                    write!(f, "外部命令失败 [{}]{}", command, st)
                } else {
                    write!(f, "外部命令失败 [{}]{}:\n{}", command, st, stderr)
                }
            }
        }
    }
}

impl std::error::Error for GateError {}

pub type Result<T> = std::result::Result<T, GateError>;

//! req-guard 终端界面（TUI）。
//!
//! 定位：**纯门禁管理台**——列需求、看三段状态、批准/打回、查审计。
//! 所有状态读写都走 `req-guard-core`，与 CLI 完全等价（core 是唯一真相）。
//!
//! 刻意取舍：
//! - **正文只读**：清单正文由 AI/编辑器维护，TUI 只做审核决策，职责清晰；
//! - **无鼠标依赖**：全部键盘可达，SSH 场景同样可用；
//! - 退出与 panic 都必须恢复终端（否则会把用户的终端搞成乱码）。

pub mod app;
pub mod ui;

pub use app::App;

use req_guard_core::error::{GateError, Result};
use std::path::Path;

/// 启动 TUI，阻塞直到用户退出。
pub fn run(root: &Path) -> Result<()> {
    app::run(root)
}

/// 终端初始化/恢复失败的统一错误。
pub(crate) fn term_err(what: &str, e: std::io::Error) -> GateError {
    GateError::Validation(format!("终端{}失败：{}", what, e))
}

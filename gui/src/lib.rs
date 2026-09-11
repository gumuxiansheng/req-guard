//! req-guard 桌面门禁管理台（egui / eframe）。
//!
//! 定位：**纯门禁管理台**——列需求、看三段状态、执行审核、查审计。
//! 与 CLI / TUI 共用同一套 `req-guard-core`：状态读写与门禁判定**只有 core 一份实现**，
//! 这里只负责渲染，杜绝"界面里放行、CLI 却拦截"的漂移。
//!
//! 刻意取舍：
//! - 正文**只读**（与 TUI 一致）：清单正文由 AI/编辑器维护，界面只做审核决策；
//! - 中文字体必须**内嵌**：egui 默认字体不含 CJK，不内嵌就是满屏豆腐块；
//! - 启动失败要能优雅降级（由 cli 回退到 TUI，见 `core::ui_mode`）。

pub mod app;
pub mod fonts;

pub use app::App;

use eframe::egui;
use req_guard_core::error::{GateError, Result};
use std::path::Path;

/// 启动 GUI 窗口，阻塞直到用户关闭。
pub fn run(root: &Path) -> Result<()> {
    let root = root.to_path_buf();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_title("req-guard 门禁管理台"),
        ..Default::default()
    };
    eframe::run_native(
        "req-guard",
        native,
        Box::new(move |cc| {
            fonts::setup(cc);
            Ok(Box::new(App::new(&root)))
        }),
    )
    .map_err(|e| GateError::Validation(format!("GUI 启动失败：{}", e)))
}

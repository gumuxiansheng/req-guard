//! req-guard core —— AI 需求门禁的**全部业务逻辑**，零外部依赖。
//!
//! 三个前端共用本 crate：`cli`（文本）、`tui`（终端界面）、`gui`（桌面管理台）。
//! **唯一真相在这里**：门禁判定与清单状态读写只实现一次，前端只渲染不判定，
//! 杜绝"GUI 与 CLI 行为不一致"——对门禁工具而言那是致命的。

pub mod auth;
pub mod comment;
pub mod digest;
pub mod error;
pub mod gate;
pub mod requirement;
pub mod status;
pub mod token;
pub mod ui_mode;

#[cfg(test)]
mod testutil;

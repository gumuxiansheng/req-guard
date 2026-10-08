//! req-guard core —— AI 需求门禁的**全部业务逻辑**，零外部依赖。
//!
//! 三个前端共用本 crate：`cli`（文本）、`tui`（终端界面）、`gui`（桌面管理台）。
//! **唯一真相在这里**：门禁判定与清单状态读写只实现一次，前端只渲染不判定，
//! 杜绝"GUI 与 CLI 行为不一致"——对门禁工具而言那是致命的。

pub mod ac;
pub mod auth;
pub mod comment;
pub mod digest;
pub mod error;
pub mod gate;
/// 需求编号防冲突检查（`ids --check` 三类检测 + `create --id` 写法 lint）。
pub mod idcheck;
/// 审批身份绑定（git identity + 内联指纹 `sig=`）。
pub mod identity;
/// 通用问题模型（`Severity`：编号检查 / AC 校验 / 变更范围校验共用）。
pub mod issue;
/// AI 工具 hook payload 解析（PreToolUse 的 stdin JSON）。
pub mod json;
pub mod requirement;
/// 多需求并行的门禁裁决：裁决对象是「本次变更集」而非「某一份清单」（REQ-006）。
pub mod resolve;
pub mod section;
pub mod specmeta;
pub mod status;
/// 分级门禁：档位模型 + `classify` 纯函数（REQ-019 G1/G3）。
pub mod tier;
/// 有效改动行统计：`git diff -U0` 解析与双侧注释行号对齐（REQ-019 T2）。
pub mod tierdiff;
pub mod token;
pub mod touch;
pub mod ui_mode;

#[cfg(test)]
mod testutil;

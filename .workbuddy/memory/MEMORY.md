# req-guard 项目长期备忘

## 项目定位
gates-toolkit 家族的**流程门禁**（管"AI 该不该写"），与 sql-guard/java-guard（管"写得对不对"）互补。
核心：需求分解 → 技术方案 → 测试计划，三段逐段批准 + 阻塞评论未 resolve 即硬拦截。

## 约定
- **零外部依赖**是立身之本，core 永远不加 `[dependencies]`；UI 走 feature 门控的 workspace（core/cli/tui/gui）。
- Rust stable（rust-toolchain.toml 固定）。
- 中文注释与提示，结论先行。
- 目录：`.gates/`（入库：yaml 声明 + 清单 + 评论 + 拦截脚本）/`gates-tools/`（不入库：二进制与渲染产物）。
- 拦截判定**唯一真相**在 `.gates/hooks/req-guard-check.{sh,ps1}` 脚本，`req-guard check` 只透传退出码。
- req-guard 是安全机制 → **fail-closed**（区别于 sql-guard/java-guard 的 fail-open）。

## 已完成（截至 2026-09-12）
- **workspace 已拆分**：`core`（零依赖 lib）/ `cli`（唯一 bin）/ `tui`（lib）/ `gui`（lib）；
  `default-members = ["core","cli"]` → 根目录 `cargo build` 仍零依赖秒级。
- CLI 核心（13 命令，含 `ui`）+ **TUI 与 GUI 门禁管理台均可用**；**44 单测**（core 39 / tui 5）+ 10 脚本场景全绿；
  `clippy --workspace -D warnings` 与 `fmt --check` 零告警；GUI 真机截图验证通过（中文无豆腐块）。
- core 结构化 API：`status::{ReqStatus,StepStatus,ReqState,req_get,req_list}`、`gate::{GateVerdict,gate_check,audit_tail}`、
  `ui_mode::{UiMode,EnvFacts,detect}`；渲染分别在 `cli::render` / `tui::ui` / `gui::app`。
- `ui` 子命令自动探测 + `--gui/--tui` 强制 + **GUI 启动失败回退 TUI**（P4 已落地）。
- GUI 中文字体：Noto Sans SC 子集 1.51MB 内嵌（`gui/assets/`，SIL OFL），
  生成脚本 `scripts/make_font_subset.py`。
- 命名统一 `req-guard-*`（常量 `gate::HOOK_SH_REL`/`HOOK_PS1_REL` 为唯一真相）；ps1 强制 UTF-8 BOM；
  `strict_order` 真实生效；install 幂等追加 `.gitignore`；评论/绕过事件入审计；AI 只能 reply 不得 resolve。
- 已 git init + 6 次提交（最新见 git log）+ GitHub Actions 三平台矩阵 + CNB 流水线 + `.gitattributes`(sh=LF)。

## 关键坑（复用）
- `cargo test` 在 workspace 根**只跑 default-members**，TUI 测试要用 `cargo test --workspace`。
- TUI 测试摊平 TestBackend 缓冲必须按 `unicode-width` 跳格，否则 CJK 占位空格导致断言误判。
- **egui 0.36 API 大改**（详见 2026-09-11.md）：`App::update` → `logic`+`ui`；
  `TopBottomPanel/SidePanel` → `egui::Panel::{top,bottom,left}`（show 接收 &mut Ui）；
  `CollapsingHeader::open(Option<bool>)` 按值。核对 API 直接读 registry 源码最快。
- 字体源：noto-cjk 仓库结构已变，正确路径 `Sans/SubsetOTF/SC/`（jsdelivr 分发）。
- 本机构建见 2026-09-11.md：Git Bash 下需前置 MSVC bin 到 PATH 并设 LIB，否则 GNU `link` 抢先。

## 已知未完成
- **P5 三平台产物矩阵**（CI 构建全绿，产物上传/发布未做）。
- 未配置远端 remote（无 push）。

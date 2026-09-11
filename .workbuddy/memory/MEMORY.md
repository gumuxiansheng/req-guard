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

## 已完成（截至 2026-09-11）
- CLI 核心（12 命令）可用；命名统一 `req-guard-*`，脚本名常量 `gate::HOOK_SH_REL`/`HOOK_PS1_REL` 为唯一真相。
- 26 单测 + 10 脚本场景全绿；`clippy --all-targets -- -D warnings` 与 `fmt --check` 零告警。
- ps1 落盘强制 UTF-8 BOM（`gate::ps1_with_bom`），否则 PowerShell 5.1 中文乱码 → 假拦截。
- `strict_order` 从 `.gates/req-guard.yaml` 真实读取（缺省 fail-closed）；install 幂等追加 `.gitignore`。
- 评论事件入 `audit/gate-audit.log`；`reply=C00N` 落盘；AI 只能 reply、不得新开评论或 resolve。
- 已 git init（首次提交 `ce4aa74`）+ GitHub Actions 三平台矩阵 + CNB 流水线 + `.gitattributes`(sh=LF)。

## 已知未完成
- **UI 线（P0–P5）全部未启动**：workspace 拆分、core API 结构化（`print_status` 仍直接 println）、
  `ui` 子命令、TUI/GUI 均无。
- 动手前需先拍板《UI架构细化方案》§9 与《GUI跨平台方案选型》§8 共 10 个决策点。
- 未配置远端 remote（无 push）。

## 本机构建
见 2026-09-11.md：Git Bash 下需前置 MSVC bin 到 PATH 并设置 LIB，否则 GNU `link` 抢先导致构建失败。
`cargo` 不在 PATH，真身 `C:/Users/win11/.cargo/bin/cargo.exe`。

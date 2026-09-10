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

## 已知未完成（截至 2026-09-11）
- UI 线（P0 workspace 化 → P1 core API 结构化 → P2 TUI → P3 GUI → P4 合一 → P5 CI 矩阵）全部未启动。
- 未 git init、无 CI、无单元测试。
- 待修：命名断裂（ai-gate-* vs req-guard-*）、ps1 缺 BOM、评论文件污染 list、--reply 不落盘、yaml 死配置。

## 本机构建
见 2026-09-11.md：Git Bash 下需前置 MSVC bin 到 PATH 并设置 LIB，否则 GNU `link` 抢先导致构建失败。

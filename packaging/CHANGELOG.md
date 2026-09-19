# 变更记录

> 版本号规则：Release tag = `v` + `Cargo.toml` 的 `[workspace.package] version`。
> `req-guard -V` 输出编译期写入的版本，可用于核对手中的二进制属于哪一版。
> 完整设计文档见源码仓库 `docs/`（`需求/` `设计/` `规范/` `提案/`）。

---

## 未发布

**新增**

- 需求编号防冲突（规范《docs/规范/需求编号防冲突命名规范.md》）：`req-guard ids --check`
  三类检测（同 id 多文件 / 自动编号污染 / 前缀歧义，硬伤退出码 1，可挂 CI / pre-push）；
  裸 `ids` 输出 `<id>\t<文件名>` 机器可读清单；`create --id` 写法 lint（stderr 提示，不阻断）。
- CI 样例（GitHub Actions `templates/ci/req-guard-ci.yml`、GitLab
  `packaging/templates/ci/req-guard-ci.yml.gitlab`）同步加入 `ids --check` 步骤；
  两条自举流水线（GitHub `gate-selfcheck`、CNB `门禁自举`）补全绿/伪造双 id 应红两条断言。

---

## v0.1.3（2026-09-18）

**新增**

- 完整发布包：多平台预编译二进制 + 安装/卸载/校验/自检脚本 + 安装指南与用户手册。
- `docs/提案/AI协同审核GUI自动弹出技术方案.md`：GUI 自动弹出的前瞻方案与 P0–P5 落地路径。
- `docs/需求/需求分析报告.md` 补状态校注；`docs/README.md` 文档索引（文档统一归拢到 `docs/`）。

**改进**

- TUI：焦点管理增强、正文滚动体验优化。
- GUI：审核（批准/打回）后自动前进到下一个未通过段；修正折叠标题点击被忽略的问题。

---

## v0.1.2（2026-09-16）

**修复（重要）**

- **L2 静默失效**：拦截脚本由工具写入后缺执行位，git 在 Unix 上静默跳过钩子，
  表现为"看着在拦、其实没拦"。现由 `install` 统一补 `chmod`，`install --verify` 单独检查执行位。
- **绕过谎报**：hook payload 改用 `req-guard hook-check` 由 Rust **真解析** JSON，
  修掉"用正则抠字段"在 Unicode 转义路径（`.gates\u002f…`）上的漏判；
  二进制不在 PATH 时退回正则粗判（保底，不完备）。
- **跨平台假红**：hook"是否已接入"的判定改为按脚本名词干（`req-guard-check` / `req-guard-deny`），
  不再带 `.sh`/`.ps1` 后缀，避免 Windows↔Linux 互装互验误报。
- Windows CI：`verify_gate.py` 在 cp1252 代码页下中文输出崩溃的问题。

**改进**

- 可靠性专项：门禁三层闭环、审计留痕、失败策略统一（req-guard 是安全机制，一律 fail-closed）。
- 命令扩展设计：gates-toolkit GUI 审核流程的扩展点梳理。

---

## v0.1.1（2026-09-13）

**新增**

- **方案 B 审批令牌**：`token issue / status / revoke`，启用后审批必须携带令牌，AI 无法自批。
- **方案 C 带外审批**：`--oob` / `REQ_GUARD_OOB`，可强制"仅接受带外审批"，台账按渠道留痕。
- **各 AI 工具原生 hook schema**：claude / codebuddy / codex / cursor 各自注入原生配置；
  Codex/Cursor 走 deny 包装（exit 2）适配其拦截语义。
- CI 门禁样例 `templates/ci/req-guard-ci.yml` + CNB 流水线自举（`install --verify` + `check`）。

**修复**

- hook 脚本解释器跨平台选错（Windows 调 sh / Linux 调 PowerShell）。
- clippy 1.98 新增 lint；CNB 流水线排除 `req-guard-gui`（Linux 无图形栈，winit 编译失败）。

---

## v0.1.0（2026-09-11）

首个可用版本。

- 三段清单审核（需求分解 → 技术方案 → 测试计划）+ 强顺序 + 硬拦截。
- 审核评论（步骤级、行号锚定、`--blocking`），AI 只能 reply、禁止 resolve。
- 应急绕过（时效 + 原因 + 强制审计）、审计日志与 `audit-digest` 摘要入库。
- CLI + TUI 门禁管理台（`req-guard ui`）。
- 多平台 Release 流水线（CNB tag_push → 交叉编译 → 附件上传）。

---

## 已修复的历史缺陷（供升级判断）

| 缺陷 | 影响 | 修复版本 |
| --- | --- | --- |
| 命名断裂（`ai-gate-*` vs `req-guard-*`） | init 后永久拦截 | v0.1.0 |
| ps1 缺 UTF-8 BOM | Windows 上恒拦截 + 中文乱码 | v0.1.0 |
| `*.comments.md` 污染需求列表 | 出现幽灵需求 | v0.1.0 |
| 拦截脚本缺执行位 | L2 静默失效 | v0.1.2 |
| 正则抠 JSON 字段漏判 | 看着在拦其实没拦 | v0.1.2 |
| hook marker 带平台后缀 | 跨平台校验假红 | v0.1.2 |
| Windows CI cp1252 编码崩溃 | 流水线误报失败 | v0.1.2 |

# 变更记录

> 版本号规则：Release tag = `v` + `Cargo.toml` 的 `[workspace.package] version`。
> `req-guard -V` 输出编译期写入的版本，可用于核对手中的二进制属于哪一版。
> 完整设计文档见源码仓库 `docs/`（`需求/` `设计/` `规范/` `提案/`）。

---

## 未发布

**多需求并行（REQ-006）—— 门禁裁决对象从「抽一份清单」改为「本次变更集」**

- 修复**漏拦**：待审清单与已批清单并存时，抽中已批那份即放行 —— AI 可以在零审批的需求上写代码，
  而审计台账记的是另一份清单（`PASS <另一份>`）。现按各清单技术方案段的 `GATE:TOUCH` 声明
  反查归属，只判相关的那几份。
- 修复**误锁**：已批清单的开发不再被无关的未批清单阻断。
- `req-guard check` 新增 `--staged` / `--base <ref>` / `--stdin` / `--req <需求ID>`；
  三个变更集来源互斥。**裸 `check` 在多份未归档清单下会报歧义并拦截**（无变更集无从归因）。
- 主门禁脚本收缩为「只取参与渲染」，判定全部下沉 core；`req-guard` 不在 PATH 时 fail-closed。
  `verify_install` 新增缺口检测（脚本是否仍自带 `sort -r` / `GATE:STEP` / `verify-content`、
  CI 模板是否含 `check --base`）。
- `touch --scope strict` 重定义：只比「本次改动归属的那一份」，候选 ≠1 时报 `AmbiguousScope`
  （旧语义「文件名逆序第一份」已废除）。
- `touch --declare` 无 `--id` 且仓库有多份未归档清单时**报错**（此前会往错的那份写声明）。
- 新增 `.gates/req-guard.yaml` 的 `multi.mode`（`resolve` 默认 / `all` 保守档）与
  `multi.bind_branch`（分支名消歧，默认关闭）。

**草稿通道契约硬化（REQ-010）**

- `apply` 一次只接受一段草稿（多段会被拒绝，不再静默丢失其余段）。
- 草稿里出现 GATE 块标记即报错并指出行号（此前被静默忽略、导致块错位）。
- 草稿短于原段即拒绝（此前静默丢弃原段尾部散文）；长出的行会在成功信息里报出落在段末。
- 写盘前重读草稿逐字比对：校验期间草稿被改动则拒绝落盘（按内容而非 mtime/size）。
- 草稿契约写进三处可查位置：`--help`、`.gates/README.md`、草稿文件头部。

## v0.1.5（2026-09-30）

**新增**

- **审批严格等级 `auth.level`（0–3，逐级包含）**：把此前散落的审批加固收拢成一个可配置等级，
  写进 `.gates/req-guard.yaml`（随仓库版本化，`init` 模板默认 **L3**）；环境变量
  `REQ_GUARD_AUTH_LEVEL` / `REQ_GUARD_STRICT_AUTH` **只能抬高**等级，不能降低。
  - **L1**：`token issue/revoke` 必须人类在真实终端完成（TTY 挑战码）——掐死"AI 执行
    `token issue` 自签凭据再自批"（方案 B 的信任根此前无人看守）；
  - **L2**：凭据只认**进程内显式传入**，不再读 `REQ_GUARD_TOKEN` 环境变量
    （该变量会被同 shell 会话里的 AI 子进程继承，等于把凭据交给 AI）；
  - **L3**：凭据升级为**一次性范围票据**（`mode=scoped`，可绑定 `<需求ID>:<步骤>`，
    用后即废、不可重放），静态令牌在 L3 不再受理。
  兼容旧配置：仅写 `auth.strict: true` 的项目按 L1 计，不降级。
- **GUI / TUI 进程内签发凭据**：界面由人类亲手启动并点击，界面进程内签发并**只在内存持有**凭据
  （不经 stdout / 环境变量 / 剪贴板）。同时解决"AI 拿得到凭据"与"GUI 非 TTY、拿不出挑战码
  导致审批被 fail-closed 卡死"两个问题，不必再先 `export REQ_GUARD_TOKEN` 再启动界面。
- `token issue [<需求ID>] [--step <步骤>]`：L3 下签发一次性票据，可用 `--step` 把票绑定到
  具体需求+步骤；不绑定时为"通用单次票"（可审任意一个对象，仍用后即废）。
- `token status` 现在显示严格等级、凭据形态（static/scoped）、状态（生效中/已消费/已过期）与绑定范围。
- `requirement::section_of`（core）：按二级标题截取清单某一段，**TUI 与 GUI 共用同一条切段规则**。

**修复（重要）**

- **TUI 上下键"没反应"**：`Focus` 状态机实现了却**从未被赋值**（只有测试改过），
  于是 `↑↓` 永远作用在需求列表、`Focus::Steps` 分支是空的；换段只能靠 `←→`，
  而"切段"与"移焦点"被混用。现 `↑↓` 在**当前聚焦面板内**选择、`←→/Tab`
  （`Shift+Tab` 反向）只移焦点，默认聚焦「三段」——进来按 `↑↓` 即可换段；
  聚焦面板标题加 `▶` 与青色边框，让"现在动的是谁"可见。
- **GUI 审核任一段都显示整篇清单**：切段逻辑原本只存在于 TUI crate，GUI 无从复用，
  折叠面板里塞的是全文。现统一走 `requirement::section_of`，展开哪一段只显示那一段
  （需求分解 / 技术方案 / 测试计划），避免审核人自己在全文里找对应节、看错段点错批准。

**改进**

- 版本号统一到 `0.1.5`：根 `Cargo.toml` 的 `[workspace.package] version` 与
  core / cli / tui / gui 四个成员 crate 对齐；两份 CI 接入样例的版本占位同步更新。
- `done` 成功后的到期自动清扫改走 `archive_due_authorized`：不再对同一次人类授权
  重复鉴权（否则 L1+ 下会冒出一条无意义的"archive 缺凭据"告警）。

---

## v0.1.4（2026-09-21）

**新增**

- 需求编号防冲突（规范《docs/规范/需求编号防冲突命名规范.md》）：`req-guard ids --check`
  三类检测（同 id 多文件 / 自动编号污染 / 前缀歧义，硬伤退出码 1，可挂 CI / pre-push）；
  裸 `ids` 输出 `<id>\t<文件名>` 机器可读清单；`create --id` 写法 lint（stderr 提示，不阻断）。
- CI 样例（GitHub Actions `templates/ci/req-guard-ci.yml`、GitLab
  `packaging/templates/ci/req-guard-ci.yml.gitlab`）同步加入 `ids --check` 步骤；
  两条自举流水线（GitHub `gate-selfcheck`、CNB `门禁自举`）补全绿/伪造双 id 应红两条断言。

**改进**

- 版本号统一到 `0.1.4`：根 `Cargo.toml` 的 `[workspace.package] version` 与
  core / cli / tui / gui 四个成员 crate 对齐；两份 CI 接入样例的版本占位同步更新，
  避免下游按旧版本号拉取不存在的 Release 附件。
  发布包的 `-V` 输出 / `VERSION` / 归档名三处自证仍由构建脚本从 `Cargo.toml` 注入。

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

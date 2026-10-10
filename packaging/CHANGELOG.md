# 变更记录

> 版本号规则：Release tag = `v` + `Cargo.toml` 的 `[workspace.package] version`。
> `req-guard -V` 输出编译期写入的版本，可用于核对手中的二进制属于哪一版。
> 完整设计文档见源码仓库 `docs/`（`需求/` `设计/` `规范/` `提案/`）。

---

## v0.1.7（2026-10-09）

**修复**

- **受管路径为空时不再误判歧义，需求清单可正常入库（REQ-020）**。
  `judge` 剥除 `touch.exempt` 后受管路径为空时，反查候选集必为空，多需求仓库一律
  `R8 → Block(Ambiguous)` —— 于是**提交一份新需求清单会被自己的门禁拦住**，
  `--no-verify` 也无效（CI 侧 `check --base` 同样算空集），门禁无法自举。
  新增 **R2b**：改动集整体落在豁免区时判定放行（位置在 R15 → R3 之后、候选推导之前），
  审计记 `PASS no-managed-path + NOTE`，与「三段已批准」明确区分。
  放行只在三条同时成立时生效：本次确实带变更集、paths 至少含一条非空路径（`--stdin`
  取不到路径时保持 fail-closed）、受管路径集为空。

**新增**

- **清单全生命周期效率与可自动化（REQ-019，已批准）**：面向 AI 的提交流程规范，
  明确「AI 不直接改 `GATE` 块标记」「只改需求分解段落正文」「改脚本须走 `req-guard approve`」，
  避免"顺手把门禁注释改掉"这类不可审计的变更。

**改进**

- GUI / TUI / CLI 代码结构重构（可读性与可维护性），行为不变。
- 版本号统一到 `0.1.7`：根 `Cargo.toml` 与 core / cli / tui / gui 四个成员 crate 对齐。

---

## v0.1.6（2026-10-05）

**界面：GUI / TUI 的 Markdown 渲染（REQ-013 / REQ-014 / REQ-018）**

- GUI 正文版心限宽居中 + 列表层级缩进；解析层换 `pulldown-cmark`，表格竖线按 GFM 判列、
  表宽与正文同宽；短表格不再占满整屏；切段后视口落到该段开头。
- GUI / TUI 审核任一段只显示那一段（统一走 `requirement::section_of`）。

### 新增：分级门禁（REQ-019）

- **四档自动定档**：`trivial` / `light` / `standard` / `critical`，档位由
  **有效改动行数 + 路径敏感度**派生，AI 不能自行选择。
  档位 = max(清单 frontmatter 的 `tier` 声明档, 派生档) —— **声明只能往上抬**；
  派生档更高时 L3 报 `TierEscalation`、退出码 1，要求按更高档重新批准。
- **有效改动行剔除注释与空行**：按**文件全文**重建注释状态机再按行号过滤
  （不是数 diff 行、也不是逐行看是不是 `//` 开头），故跨 hunk 的块注释、
  字符串里的 `//` 都判得对；「只改注释、只加空行」自然落进免审档。
- **新增命令** `req-guard tier check [--staged | --base <ref>]`：只读输出档位**与理由**
  （逐文件有效行、命中的 glob、声明档 vs 派生档）。退出码只表示「算出来了」，
  不表示通过门禁。
- **`approve --all-steps`**：轻档一条命令批三段。原子性（任一段不合规则一段都不批）、
  留痕不减（三条 `APPROVE`，各带 `sum=`，`channel=quick`）、凭据不放宽
  （L3 下需 `scope` 为 `<需求ID>:*` 的通配票据，一次性；精确票与通配票互不对冲）、
  实质正文仍强制。轻档 AC 选填，但写了必须全量合规（A2–A12 不放松）。
- **配置**：`.gates/req-guard.yaml` 新增 `tier` 段（`enabled` /
  `trivial_max_lines`（5）/ `light_max_lines`（80）/ `risky_paths`）。
  门禁自身源码（`core/src/**`、`templates/hooks/**`、`templates/ci/**`、
  `.gates/req-guard.yaml`）恒为 `critical` 且**不可配置** ——
  配置里写 `locked_paths` 键会被直接拒绝（不静默忽略）。
- **兼容与回滚**：`tier` 缺省或取历史值 `standard` 时三段语义**逐字不变**，
  存量清单零迁移；把 `tier` 段置空 / `enabled: false` 即完整回滚。
- **三层边界写进文档**：L1 `hook-check` 只能给**下界**（看不到整体 diff、无跨调用记忆），
  L2 看不见未暂存改动，L3（`check --base`）是**权威**层；豁免区改动**不参与定档**。

### ⚠️ 破坏性变更（升级前必读）

- **`bypass --ttl` 上限 240 分钟**：`bypass --ttl 0` 或 `> 240` 从「接受」变为「拒绝」。
  动机：`active_bypass` 只认 `expires_epoch`，故 `expires_epoch=99999999999` 等价于
  **永不过期**的全部门禁开关。加上限后该模式不可能出现。合法用法是分段重新开启
  （每次独立留痕）。参见 REQ-012。
- **应急绕过令牌迁出工作树**：从 `.gates/.bypass` 移到用户级状态目录
  （`$XDG_STATE_HOME/req-guard/<仓库路径哈希>/bypass`，未设则回落
  `~/Library/Application Support/...`（macOS）或 `~/.local/state/...`）。
  迁移期读旧路径并**自愈重写**，存量仓库无需手工处理。`.gitignore` 里保留旧条目。
- **绕过令牌须通过两道校验才生效**（见下）。手写 `expires_epoch` 的绕过方式已完全失效。
- **`touch.exempt` 默认集收窄**：不再默认豁免 `.gates/**` 通配，改为具名清单。
  `.gates/req-guard.yaml`（含 `auth.level` / `enforce.ci`）与 `.gates/audit/ledger.md`
  **不再豁免** —— 改它们须由清单的 `GATE:TOUCH` 声明。
  若你的流程依赖旧行为，在 `req-guard.yaml` 的 `touch.exempt` 里显式加回 `.gates/**`。

### 安全修复（REQ-012：门禁自身可被本地禁用与自改）

- **伪造应急绕过令牌不再生效**。原实现只读 `expires_epoch`，故一行
  `printf 'expires_epoch=99999999999\n' > .gates/.bypass` 即可解除全部门禁，
  且该文件被 gitignore、`audit_ledger.md` **零痕迹**。现在令牌必须：
  ① 通过身份自证（`sig` 等于**此刻**把 `actor` 绑到生效身份上得到的 `sig`）；
  ② 与**入库台账** `ledger.md` 里的 `BYPASS-OPEN` 事件对得上。
  被拒时写审计：`BYPASS-REJECT reason=missing-fields|sig-mismatch|no-ledger-entry`。
- **发布脚本自检恢复**。`init` 默认 `auth.level: 3`，而模板要求一次性范围票据，
  于是**任何脚本化 `approve` 都被拒** —— 表现为发布包 `selfcheck.sh` 7 PASS/7 FAIL、
  `.github/workflows/ci.yml` 自举 job 中止、`.cnb.yml` 主流程变红，
  即**门禁自己过不了自己的门禁**。修复分两处（第二个根因此前未被识别）：
  - 新增 `req-guard init --for-ci`：沙箱/CI 自检用（`auth.level: 0` + `enforce.ci: false`），
    **裸 `init` 的默认值不变（仍 L3）**；
  - `selfcheck` / CI 自举此前**从未填过三段实质正文**，而 `approve` 会跑段落实质性校验
    （REQ-003）拒绝模板占位 —— 这是与 `auth.level` 无关的**第二个**根因。
  - 结果：`selfcheck.sh` 由 7 PASS/7 FAIL 变为 **15 PASS/0 FAIL**。
- **L3 接入质量可被机械判定**。原 `verify_ci` 只做子串匹配（编排里含 `req-guard` 字样即算接入
  —— 本仓自己的 `ci.yml` 曾靠注释通过）。现在要求 `req-guard` 处于**命令位**且**紧跟子命令**，
  并逐条检出三种「接了但会静默失效」的接法：缺 `fetch-depth: 0`、`--base` 缺 ref、
  下载二进制缺版本自证。
- **发布的 CI 模板此前跑不通**：`actions/checkout@v4` 缺 `fetch-depth`，
  depth=1 下 `origin/main` 不存在，两条 `--base` 步骤以
  `fatal: ambiguous argument` 硬失败。现已补 `fetch-depth: 0`；版本改走仓库/CI 变量
  （未设即失败并说明怎么设）并加 `req-guard -V` 版本自证；
  GitLab 模板补上 `--base`（此前用裸 `check`，规范 §4.3 明令禁止）。
- **`install --verify` 不再说谎**。原判据是子串存在性检查，实测在脚本顶部插入 `exit 0`
  （保留后面的委托行）后报 **PASS**，而 hook 已永久放行。现在改为**实跑装好的脚本**验
  「未过审必拦 / 已过审必放行」两个方向（`--quick` 可退回快筛）；
  语义自检找不到可执行文件时**如实跳过**，不拿「找不到」当「判失败」。
  存在性检查降级为告警（`⚠️`），反向检查（禁止脚本自建判定）仍为 Error。
- **`token revoke` 不再锁死**。凭据启用后撤销需出示当前凭据，凭据丢失即无路可走
  （此前只能手工删 `~/.config/req-guard/guard.cfg`，且不留痕）。新增
  `token revoke --i-lost-it`：须真实终端在场（TTY 挑战码），并强制记入入库台账
  `REVOKE-FORCED`。
- **新增** `install --verify --base <ref>`：检出「改了 `.gates/req-guard.yaml` 但无清单背书」，
  把「AI 调松门禁」变成 PR 里必然被评审看到的一条失败。

### 已知残留（不要当成密码学保证）

- 绕过令牌的 `sig` 是**公开可复算**的派生值；第① 道校验挡随手伪造，第② 道（台账交叉核对）
  把伪造变成**PR 可见**而非不可伪造。同用户进程若肯重算 sig 并改台账，仍可伪造。
  真正的兜底是服务端必需检查 + `ledger.md` 挂 CODEOWNERS。
- 绕过窗口生效期间**不做内容篡改检测**（短路顺序本期未改）。
- `install --verify` 的实跑自检仍信任被测脚本本身；若 hook 与 core 同时被替换，实跑看不出。
- `verify_ci` 的「是否执行了 req-guard」仍是**启发式**（不解析 shell 语法），
  只挡已知的两种误接形态。
- **审批凭据无密码学保证**（本次未改变）：`~/.config/req-guard/guard.cfg` 是明文 key=value、
  无完整性保护；TTY 挑战码打在 stdout 由同一进程读回，`pty` 可满足。
  本产品防的是「顺手自批」与「格式塌方」，不是有动机的对抗。

---

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

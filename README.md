# req-guard —— AI 需求门禁

> **AI 在编写代码前，必须先走完「需求分解 → 技术方案 → 测试计划」三段审核，否则物理上无法写入文件。**
> gates-toolkit 家族成员（流程门禁：管"该不该写"；sql-guard/java-guard 管"写得对不对"）。

## 特性

- **三段清单审核**：强制顺序 `需求分解 → 技术方案 → 测试计划`，逐段批准
- **硬拦截**：AI 工具 `PreToolUse`（写不了文件）+ git `pre-commit`（fail-closed）+ CI `check`
- **审核评论**：审核人只评论不改正文；步骤级 + 行号锚定；`--blocking` 未 resolve 即拦截
- **证据不可篡改**：评论独立文件，AI 禁止直接写、禁止 resolve（只能 reply）
- **审计留痕**：拦截/放行/绕过/评论全部入 `audit/gate-audit.log`
- **分级门禁**：四档按**有效改动行数**与高危路径自动定档；只改注释/空行 → 免审档，
  轻档一条命令批三段（三段仍各留台账）；声明只能往上抬，CI 复算为权威
- **应急绕过**：有时效、必填原因；**不覆盖评论证据保护**
- **审批锁**：`approve/reject/amend/resolve/bypass/seal` 须出示人类凭据才放行——AI 会话标记（方案 A）、
  审批令牌（方案 B）、带外声明（方案 C）、终端挑战码（方案 D）；**严格模式下无凭据即拒**，
  不依赖"AI 工具是否已登记"，堵 AI 自批
- 零外部依赖（纯 Rust 标准库），开箱即 build

## 快速上手

```bash
# 0. 初始化（写入 .gates/ + 注入 AI 工具 hook + 追加 pre-commit）
req-guard init

# 1. 创建需求清单
req-guard create -t "用户登录改造"          # → REQ-001

# 2. AI 填写三段正文（编辑 .gates/requirements/REQ-001-*.md）

# 3. 审核人逐段批准（此时 AI 仍被拦截）
req-guard approve REQ-001 --step decomposition --reviewer 寇工
req-guard approve REQ-001 --step solution      --reviewer 寇工
req-guard approve REQ-001 --step testplan      --reviewer 寇工

# 4. 查看状态 / 评论
req-guard status REQ-001
req-guard comments REQ-001

# 5. 解锁后 AI 方可编写代码；提交时 pre-commit 二次校验

# 6. 需求完成（或中止）后归档——门禁随之跳过该清单
#    done 满 archive.after_days 天（默认 30，见 .gates/req-guard.yaml）后自动物理归档；
#    手动补扫：req-guard archive --author 寇工（--dry-run 先预览）
req-guard done REQ-001 --author 寇工

# （可选）查看归档历史
req-guard status --archived
```

## 到期归档：需求文档不再无限膨胀

全部需求平铺在 `.gates/requirements/` 会让 `status`/`list` 越滚越长。`req-guard done`
成功后自动扫描 done 满 `archive.after_days`（默认 30）天的清单，把**清单 + 评论**
成对搬入 `.gates/requirements/archive/<创建年份>/`（无日期段 → `misc/`）：

- 归档区是**只读历史**：拦截脚本、`list`、自动编号都不再看它；`status`/`comments`
  仍可按 id 查询（`req-guard status REQ-001`）；
- 归档**不复号**：自动编号跳过归档区已用过的 `REQ-NNN`（id 是永久主键）；
- AI 工具无法写入归档区（原样保留历史证据，`hook-check` 直接拦截）；

## 审核评论（审核人只评论、不修改）

```bash
# 纯评论（不改状态）
req-guard comment REQ-001 --step solution --author 寇工 --text "回滚方案需补充 DB 迁移回退"

# 锚定到原文 + 标记阻塞（未 resolve 时拦截编码）
req-guard comment REQ-001 --step solution --author 寇工 --quote "回滚方案" --blocking --text "..."

# AI 回复（只能 reply，不能 resolve）
# 不带 --reply 会被拒绝：AI 不得新开评论，只能回复审核人
req-guard comment REQ-001 --author ai --reply C001 --text "已补充迁移回退步骤"

# 审核人关闭
req-guard resolve REQ-001 C001 --author 寇工

# 正文修改后重算行号锚点
req-guard comments REQ-001 --refresh-anchors
```

## 应急绕过（有痕、有时效）

```bash
req-guard bypass --reason "线上热修，事后补审" --ttl 60   # 有痕、有时效（默认 60 分钟）
req-guard check                                            # 手动判定（CI 用）
```

**人肉开发什么时候该用**：bypass 是为**高档逃逸**准备的应急阀——
线上热修、实验性试改、以及「来不及走流程」的紧急情况。
**日常小事已经不需要它了**：只改几行代码 / 只改注释 / 文档笔误走
[分级门禁](#分级门禁四档按有效改动行数与高危路径自动定档)的 `trivial` 档
（免审）与 `light` 档（一条命令批三段），不必建 REQ、也不必绕过。

- 每次绕过强制审计：原因（`--reason` 必填）+ 时效（`--ttl`，默认 60 分钟，到期自动失效）
  全部入 `audit/gate-audit.log` 与入库台账 `ledger.md`；
- 事后请补建 REQ 并归档（`req-guard done <REQ-ID> --author <姓名>`），把账还上；
- 高频使用（每周数次）仍是流程失控信号：说明需求拆分或审核节拍出了问题，
  先修流程而不是继续绕。

## 分级门禁（四档：按有效改动行数与高危路径自动定档）

门禁不是只有一档：**改动越重，审批越严；改动越轻，流程越省**。
档位由**变更集派生**，AI 不能自行选择。

| 档位 | 判定 | 审批形态 | 验收标准 |
| --- | --- | --- | --- |
| `trivial` | 有效改动行 ≤ 5 且未命中高危路径 | 免审（免建清单） | 不要求 |
| `light` | 有效改动行 ≤ 80 且未命中高危路径 | `approve --all-steps`（一条命令批三段） | 选填 |
| `standard` | 其余 | 三段逐段批准 | 必填 |
| `critical` | 命中**内建锁定项**（`core/src/**`、`templates/hooks/**`、`templates/ci/**`、`.gates/req-guard.yaml`） | 三段逐段批准 + 附加检查 | 必填 |

- **有效改动行 = 剔除注释行与空行之后的增删行**。判法不是数 `diff` 行，也不是逐行看
  diff 是不是以 `//` 开头，而是按**文件全文**重建注释状态机再按行号过滤 ——
  跨 hunk 的块注释、字符串里的 `//` 都判得对。所以「只改注释、只加空行」天然落进免审档。
- **高危路径可配置**（`.gates/req-guard.yaml` 的 `tier.risky_paths`，支持目录前缀与
  glob）；**门禁自身源码恒为 `critical` 且不可配置** —— 配置里写 `locked_paths` 键会被
  直接拒绝（不是忽略）。
- **声明只能往上抬**：清单 frontmatter 的 `tier` 键与派生档取 `max`。
  写小不成立 —— CI 会按完整变更集复算，派生档更高时报 `TierEscalation` 并退出码 1，
  要求把清单按更高档**重新批准**。
- **三层用同一份判定**：L1 `hook-check` 只看单次写入（**下界**，不拦，只提示
  「最终以 CI 复算为准」）、L2 pre-commit 看索引内变更集、L3 CI 看完整变更集（**权威**）。
- **豁免区不参与定档**：改动整体落在 `touch.exempt` 时直接放行并在审计记
  `PASS no-managed-path`（清单正文、草稿、`target/**` 都在豁免区）。

```bash
req-guard tier check --staged        # 算档位并输出理由（逐文件有效行 / 命中 glob / 声明 vs 派生）
req-guard tier check --base origin/main   # CI 口径（权威层）
req-guard approve REQ-007 --all-steps      # 轻档：一条命令批三段（三段仍各留一条 APPROVE 台账）
```

`approve --all-steps` 的四条硬约束：**原子性**（任一段不合规则一段都不批）、
**留痕不减**（三条 `APPROVE`，各带 `sum=`，`channel=quick`）、**凭据不放宽**
（L3 下需要 `scope` 为 `<需求ID>:*` 的通配票据，一次性；精确票与通配票互不对冲）、
**实质正文仍强制**（至少写清「改了什么 + 怎么自测」）。

阈值不许拍脑袋写死：改阈值前先跑 `python scripts/calibrate_tier.py`（回放本仓历史
变更集，输出各档分布与阈值 ±50% 敏感度）。口径与边界详见
[`docs/设计/分级门禁技术方案.md`](docs/设计/分级门禁技术方案.md)。

**回滚**：`.gates/req-guard.yaml` 的 `tier` 段置空或 `enabled: false` →
判定退回三段逐段批准，且门禁输出与启用前逐字一致；清单里的 `tier` 键保留即可
（`standard` 在新旧枚举下都合法）。

## 合规部署（三层 + 锁）

```bash
req-guard install --verify   # 校验资产 / AI 工具 hook / pre-commit / CI 接入就位（CI 步骤，缺口退出码 1）
req-guard audit-digest       # 本机审计日志 SHA-256 摘要 → 入库 .gates/audit/DIGEST
req-guard whoami             # 本仓库审批身份：git 身份 + sig 指纹 + auth 等级 + AI 上下文判定
```

- **L2 fail-closed**：`.gates/hooks/req-guard-check.sh` 缺失时 `git commit` 被阻止（非静默放行）
- **L3 默认开启**：`enforce.ci: true`——流水线调 `req-guard check` 并设为必需状态检查 + 分支保护；
  `install --verify` 会**体检 CI 是否真接入**（无编排或编排未调用 req-guard 即红；可显式 `enforce.ci: false` 放弃）
- **工具原生 schema**：默认注入全部内置工具 claude / codebuddy / codex / cursor，各自注入
  原生 hook 配置（`install --tool` 可指定子集）；无 PreToolUse 机制的 agent（Copilot / Trae 等）
  **无法被 L1 拦到**，只能靠 L2/L3 兜底；
  Codex/Cursor 走 deny 包装（exit 2）适配其拦截语义，CodeBuddy 已修正为 `.codebuddy/settings.json`
- **审批锁（方案 A，软标记）**：Claude Code / CodeBuddy 会话注入 `REQ_GUARD_AI_CTX=1`（配置 `env` 段），
  `approve/reject/amend/resolve/bypass/seal` 检测到即拒；审核人在自己的终端审批。
  ⚠️ 白名单式：**未登记的 AI 工具会话内没有该标记**，"没标记 = 人类"是 fail-open，
  靠它单独挡不住新工具（2026-09-29 Firedit 事故）
- **审批凭据（方案 B）**：`req-guard token issue` 签发凭据（原文仅打印一次，人类带外持有），
  启用后审批必须携带有效凭据，AI 拿不到 → 无法自批；`token status/revoke` 查询与撤销。
  凭据管理本身受审批锁保护：已有生效凭据时重签/撤销**必须出示当前凭据**（防 AI 静默替换）
- **带外审批（方案 C）**：审批显式声明带外渠道 `--oob` 或 `req-guard oob <命令>`；
  设 `REQ_GUARD_OOB_ONLY=1` 可强制"仅接受带外审批"；台账按 `channel=oob|interactive` 留痕
- **人类在场证明（方案 D，fail-closed）**：审批必须出示凭据，或在**真实终端**输入一次性挑战码。
  AI 的执行通道是管道（实测 stdin/stdout 均非 TTY），两者都拿不出 → 被拒；
  且不依赖"工具是否已登记"，新工具不登记也不会静默失效。台账带 `tty=`/`ai=` 通道指纹，AI 代跑（`tty=0`）可直接检出
- **审批严格等级 `auth.level`（0–3，逐级包含；`init` 模板默认 L3）**：
  `L1` 起**签发凭据必须人类在真实终端**（AI 再也无法 `token issue` 自签后自批）；
  `L2` 起凭据**只认进程内显式传入**，不再读 `REQ_GUARD_TOKEN`（环境变量会被同会话 AI 子进程继承）；
  `L3` 起凭据升级为**一次性范围票据**（绑定 `<需求ID>:<步骤>`，用后即废，不可重放，静态令牌不受理）。
  环境变量 `REQ_GUARD_AUTH_LEVEL` 与 `REQ_GUARD_STRICT_AUTH` **只能抬高等级**，不能降低。
  GUI / TUI 由界面进程**进程内签发并内存持有**凭据（人类点击界面即在场证明），
  既不落在 stdout / 环境变量里，也无需先 `export REQ_GUARD_TOKEN`
- **payload 真解析**：拦截脚本第 0 段优先调 `req-guard hook-check`，由 Rust 解析 AI 工具的
  PreToolUse JSON——路径可写成 `.gates\u002f…comments.md` 这类 Unicode 转义，与明文**完全等价**，
  脚本用正则抠字段会漏判并静默放过；`req-guard` 不在 PATH 时退回正则粗判（保底，不完备）
- **清单正文可写、状态行不可改**：AI 可用 Write 整篇填写 `.gates/requirements/*.md`
  （不等三段批准——否则"AI 填三段正文"这步会被门禁自己拦死）；但 GATE 状态行必须与磁盘
  **逐行一致**，改动即拦截（防 AI 自批）；片段编辑（Edit）无法与基线比对，一律禁止
- **身份绑定（可归属）**：`approve/reject/amend/resolve/done/bypass` 的操作人**自动取 git 身份**
  （`git config user.name/user.email`，即提交本身的署名），并把 `email` + 指纹 `sig` 写进
  `GATE:STEP` 标记行、审核记录、入库台账与 `.bypass`；`--reviewer` 可省略。
  `auth.level ≥ 1` 时 `--reviewer` 与 git 身份冲突、或取不到 git 身份**一律拒绝**；
  `L0` 放行但台账标 `mismatch=1` 供事后批量捞出。
  `req-guard whoami` 可先查看"我会以谁的身份落账"。
  ⚠️ `sig` 是**公开可复算的派生值**（无密钥），本项保证"审批记录归属到某个 git 身份"，
  **不保证"审批不可伪造"**——AI 仍可 `git config user.name` 伪造身份。它与方案 D 叠加而非替代，
  真正兜底仍是服务端 required 检查 + 事后复核
- **规格绑定代码（SDD 契约）**：清单 frontmatter 声明
  `review_policy = codebound` + `source_refs`（本次要改的文件/模块目录），
  字段名与 doc-guard 的 FRS 族**逐字对齐**，同一份清单可被它的
  FRS001/003/004/005/007 与 DRF001 直接接管——「代码改了规格没同步」会被报出来
- **技术方案批准前必须声明 `source_refs`**（`auth.level ≥ 1` 拒绝，L0 放行并告警）。
  卡第二段而非第一段：需求分解阶段还不知道要改哪些文件，逼迫 AI 填占位值比不填更糟。
  **这条只能放在批准动作上**：FRS001 只检查 `source_refs` 这个键是否存在，
  `source_refs: []` 一律通过、FRS004 也无从匹配——声明侧留空在 doc-guard 侧完全静默，
  规格看似接入时效治理、实则永远不会被判过期
- **install 生成物豁免**：`touch-check` 的豁免集默认项 + **由 install 写入目标派生**的
  AI 工具 hook 配置（精确到文件，非目录通配）；这些文件同时进 `.gitignore`（可再生派生物）。
  `touch.exempt` 配置为**追加**到默认集，不是替换。
- **审计入库**：approve / reject / amend / resolve / bypass / seal / 阻塞评论写入 `.gates/audit/ledger.md`（PR 可复核）

详见 [`docs/规范/AI工具合规保证规范.md`](docs/规范/AI工具合规保证规范.md)。

## 命令一览

`init` `create` `approve` `reject` `amend` `apply` `comment` `resolve` `done` `archive` `status` `list` `ids` `comments` `check` `ac` `tier` `touch-check` `touch` `verify-content` `seal` `install` `bypass` `audit-digest` `whoami` `token` `oob` `ui`
（`ac check [<需求ID>] [--all]` 校验清单内容：第 3 段验收标准（编号连续 +
Given/When/Then 齐备）+ **三段实质正文**（模板占位不算）；硬伤退出码 1；
`approve` 时已跑同一判据，CI 这一步是服务端兜底）
（`verify-content [<需求ID>]` 校验已批准段的正文未被改动 —— `GATE:STEP` 的 `sum=` 绑定
批准那一刻的内容；`seal <需求ID>` 把摘要绑定到当前正文，AI 禁止执行）
（`touch-check [--base <ref>]` 校验「实际改动 ⊆ 技术方案段 `GATE:TOUCH` 声明并集」；
`touch --declare --glob <路径>` 扩张声明范围，AI 禁止调用且会打回技术方案重审；
CI 侧的 `--base` 是服务端对应物，抵消 `--no-verify`）
（`tier check [--staged | --base <ref>]` 分级门禁：按变更集算档位并输出**理由**
（逐文件有效行、命中 glob、声明档 vs 派生档）；只读，不改状态，退出码只表示「算出来了」，
**不表示通过门禁** —— 门禁裁决仍是 `check`）
（内部命令 `hook-check`：由拦截脚本调用，读 stdin 做证据保护判定，通常无需手工执行）
（`req-guard -h` 查看完整参数；`-p` 指定项目根；**操作人身份取 git 身份**（`user.name`），
`--reviewer` / `--author` 可省略，回退顺序为「参数 → `REQ_GUARD_REVIEWER` → git 身份」，
无 git 环境时可用 `REQ_GUARD_REVIEWER_EMAIL` 声明邮箱；审批凭据 `--token`（L0–L1 亦可用
`REQ_GUARD_TOKEN`）；带外审批 `--oob`/`REQ_GUARD_OOB`；审批严格等级 `auth.level` /
`REQ_GUARD_AUTH_LEVEL` / `REQ_GUARD_STRICT_AUTH`）

## 门禁管理台（TUI / GUI）

```bash
cargo build -p req-guard --features tui   # CLI + 终端界面（依赖小）
cargo build -p req-guard --features gui   # CLI + 桌面界面（依赖大，含内嵌中文字体）
cargo build -p req-guard --features full  # 三合一，单二进制自动探测

req-guard ui            # 自动探测：Windows/macOS → GUI，SSH 会话 / 无图形环境 → TUI
req-guard ui --tui      # 强制终端界面
req-guard ui --gui      # 强制桌面界面
```

TUI 键位：`↑↓` 在当前聚焦面板内选择（默认聚焦「三段」，进来直接按 `↑↓` 换段；焦点面板标题带 `▶`）·
`←→/Tab` 切换焦点（需求列表 → 三段 → 正文，`Shift+Tab` 反向）· `PageUp/PageDown` 滚动正文 ·
`a` 批准 · `r` 打回 · `e` 修订 · `m` 评论面板 · `s` 绑定内容摘要 · `n` 新建 ·
`g` 门禁检查 · `b` 应急绕过 · `d` 归档（done）· `A` 物理归档 · `v` 归档区 ·
`i` 身份与鉴权等级 · `H` 体检 · `W` 追加变更范围声明 · `S` 一键批量绑定 · `G` 刷新审计摘要 ·
`Y` 工程卫生 · `L` 审计日志 · `R` 刷新 · `?` 帮助 · `q` 退出。

评论面板（TUI `m` / GUI 底部「评论」按钮 / 中部「查看评论」链接，三个入口等价）：

| 操作 | TUI | GUI |
| --- | --- | --- |
| 查看评论（含状态 / 阻塞 / 行号锚点 / 回复关系） | `m` | 「评论」按钮或「查看评论」链接 |
| 新增评论（锚定当前段） | `n` 普通 · `N` 阻塞 | 「新增普通评论」/「新增阻塞评论」 |
| 关闭评论 `resolve`（解除阻塞，只能人类署名） | `x` | 「关闭选中（resolve）」 |
| 正文改动后重算行号锚点 | `A` | 「重算行号锚点」 |
| **修订**当前段（方向没错、只是要改；回退待审 + 清摘要，**仍需重审**） | `e` | 未通过段的按钮组 / 已通过段的「请求修订」 |
| **绑定内容摘要** `seal`（存量清单补绑定；已全绑定再封必给原因，记 `RESEAL`） | `s` | 底部「绑定内容摘要（N 段待处理）」/ 段上的「绑定本段内容摘要」 |

> 与 CLI 完全等价：面板只调 `core::comment::{list,add,resolve,refresh_anchors}`，
> 界面不自己解析评论文件。`resolve` 属审批类动作，与批准/打回一样走
> "界面进程内签发凭据"（L3 下为 `resolve:<需求ID>` 范围票据）。

内容冻结（REQ-002 起的"批准即绑定正文摘要"）在界面上是可读的：

| 段标题标记 | 含义 | 审核人该做什么 |
| --- | --- | --- |
| 无标记 | 已批准且正文与批准时一致（真冻结） | 什么都不用做 |
| `[未绑定]` | 已批准但 `sum=-`（存量清单 / 未 seal） | `s` / 「绑定内容摘要」补绑定 |
| `[已改动]` | 已批准且已绑定，但正文被改过 | 正常路径：`e` 修订 → 改 → 重新批准；确实无需重审才 `s` 并写明原因 |
| `[无法校验]` | `sum=` 损坏或段落标题定位失败（fail-closed） | 手工修好清单结构 |

该判定在 core 的 `requirement::seal_state()`（含"当前正文是否仍一致"），
经 `ReqStatus.steps[].seal` 下发，两个界面只渲染，不各自重算。

GUI 为三面板：左需求列表（红=被卡 / 绿=已解锁）、右三段折叠清单（状态 + 审核人 + 只读正文 + 批准/打回）、
底部操作区（创建/刷新/检查/绕过/审计/评论），另可切换项目根目录。
展开哪一段就只看那一段的正文（切段规则在 core，与 TUI 一致），不必在整篇清单里自己找。
正文默认**按 Markdown 渲染**（标题 / 任务勾选框 `- [ ]` / 有序无序列表 / 表格 / 引用 / 代码块 / 粗斜体 /
行内代码，`<!-- GATE:… -->` 标记行自动隐藏），顶栏「正文：渲染 | 原文」可一键切回等宽原文视图
（逐字核对 GATE 行、整段复制时用）。渲染是零新增依赖的内置实现（`gui/src/markdown.rs`），
**链接不可点击、图片不加载**——清单正文由 AI 生成，审核人不该被一个外链带走。

### 需求生命周期：归档（done）/ 物理归档 / 归档区

> ⚠️ **归档 ≠ 交付完成。** 本工具里 `done` 的语义是**把需求移出门禁管辖**
> （`GATE:HEAD` 的 `status=done`），不是"这个功能做完了"——本工具不存在"交付完成"这个状态。
> 按钮与文案一律写作「归档（done）」，就是为了不让人误读。

| 操作 | TUI | GUI | 说明 |
| --- | --- | --- | --- |
| **归档（done）**当前需求 | `d` | 底部「归档（done）」 | 移出门禁管辖；有未关闭阻塞评论时 TUI 需按两次、GUI 需勾选「我已知悉」 |
| **物理归档**预演（`src → dst`，不动磁盘、不写审计） | `A` | 「物理归档」→「预演归档」 | 先看搬去哪，再决定搬不搬 |
| **物理归档**执行（`fs::rename`，git 记为 rename） | `Shift+D` | 「执行归档」 | 正文与评论**成对**搬移，历史不丢 |
| 清理**到期**归档（判据 `archive.after_days`，默认 30 天） | `A` 后选到期 | 「预演清理到期」/「执行清理到期」 | 结果会写明判据天数 |
| 浏览**归档区**（只读历史） | `v` | 底部「归档区」 | 只读：批准 / 打回 / 修订 / 封存 / 归档全部停用 |
| 查看身份与鉴权等级 | `i` | 顶栏右侧 | `姓名 <邮箱> sig=指纹 · L<等级>` |

归档确认框（GUI）/ 第一次 `d`（TUI）会先列出三项事实，做不到"点一下就过去"：

1. 当前进度 `N/3 已通过`；
2. 未关闭阻塞评论数（> 0 时必须额外确认）；
3. 「归档后门禁不再管辖本需求，其上的阻塞评论随之失效」。

理由：`done` 在界面上走**免票据**的界面凭据（以"人类亲手点击界面"为在场证明），
门槛比 CLI 低；而它又是不可逆的、且会让阻塞评论失效。
**用「知情同意」补偿「免票据」**——这是本工具在这一处的取舍，不是默认。
界面不提供 un-done；找回请用 CLI `req-guard status --archived` 浏览归档区。

> 界面上 `done` 成功后**不会**自动触发物理归档（CLI 会）。
> 自动搬文件是不可逆动作，界面上应让人显式点一次。

### 校验报告：四组只读体检 + 三个写入动作

界面过去**只有写入动作、没有体检视图**——core 里那套判定（变更范围契约、
内容冻结一致性、验收标准机检、交叉引用体检）一个都没接。现在收在一个「体检」入口里：

| 组 | core 判定 | 回答的问题 | 对应 CLI |
| --- | --- | --- | --- |
| ① 变更范围 | `touch::check` | 本次改动是否都在方案声明的范围内 | `touch-check` |
| ② 内容一致性 | `requirement::verify_sums` | 已批准段的正文有没有被偷改 / 摘要格式是否合法 | `verify-content` |
| ③ 验收标准 | `ac::check` | AC 编号格式、Given/When/Then 齐备、交叉引用是否合规 | `ac check` |
| ④ 交叉引用 | `touch::check_cross_refs` | 第 2 段引用的文件 / 小节号是否真实存在 | `touch-check` 的一部分 |

TUI `H` 打开、`1`–`4` 切子标签；GUI 底部「体检」按钮。
**①–④ 全部只读，不签发凭据、不消耗审批资格**，也不进 3 秒轮询（一次性算完缓存）。

三个写入动作（各自在对话框里，不在总览面板上直接可点）：

| 动作 | TUI | GUI | 副作用 |
| --- | --- | --- | --- |
| 追加变更范围声明 `touch --declare` | `W`（两步：路径 → 原因） | 「追加变更范围声明」 | **人类专属**（AI 不得自己扩范围）；`touch.reapprove` 为真时技术方案被打回 pending **且该段 `sum=` 被清空**，界面上会先写明 |
| 批量绑定内容摘要 | `S`（一键全部） | 「批量绑定内容摘要」（多选） | 走 `seal_many` 而非循环 `seal`：L3 下票据用后即废，循环写法第 2 份起必失败 |
| 刷新审计摘要 | `G` | 「刷新审计摘要」 | 写 `.gates/audit/DIGEST`；**非审批类**，不签发凭据（L0 也能用） |

> ⚠️ 两条诚实边界（实测，与需求文档的设想有出入，已在代码注释里写明）：
> ① `seal_many` 是**全有全无**——第一条被拒就整体中止，给不出「成功 2 / 尝试 3 + 失败编号」
> 这种逐条结论；界面如实报「中止」并把 core 那句**带编号**的错误原样透传。
> ② 交叉引用体检在**第 2 段定位失败**时只说「定位失败」，不给出任何具体引用的失效结论
> ——`section_span` 是 fail-closed，宁可说"我不知道"也不回退整篇去"看起来校验过了"。

### 工程卫生：编号冲突 / 门禁自检 / 环境

界面上过去没有任何工程卫生视图——编号冲突、自检、身份与票据状态全在 CLI 里。
现在收在一个「工程卫生」入口（TUI `Y`，`1`–`3` 切分组）：

| 分组 | core 判定 | 回答的问题 | 对应 CLI |
| --- | --- | --- | --- |
| ① 编号冲突 | `idcheck::check` | 同 id 多文件 / 自动编号污染 / 前缀歧义 | `ids --check` |
| ② 门禁自检 | `gate::verify_install_with(None, **true**)` | 本机门禁资产是否真的**拦得住** | `install --verify --semantic` |
| ③ 环境 | `identity::describe` + `auth::effective_level` + `token::summary()` | 我是谁、什么等级、还有票吗 | `whoami` / `token status` |

自检的两个参数是刻意的：**基准引用传空**（本地没有"PR 视角"，硬判 `origin/main` 只会误报），
**语义自检传真**（只做子串存在性检查形同虚设——往 hook 顶部插 `exit 0` 仍报 PASS）。
语义自检会在临时仓库实跑脚本，首次约需数秒；结果只在打开面板时算一次，**不进 3 秒轮询**。

新建需求现在可以指定**自定义编号**（留空 = 自动编号，与改前逐字一致）。
`lint_id` 的形态提示**只提示、不阻断**——编号规范是团队约定，不是门禁。
真正的重复由 `requirement::create` 返回错误，界面**原样显示**那句话（含是哪个编号、与谁冲突）。

返工率（`amend_counts`）在清单详情里按段显示，**≥ 3 次**才加一句「建议复盘」。
它**不是门禁**：做成门禁只会催生"少写 amend 刷分"。

> ⚠️ **界面上不提供票据签发与撤销**，这是鉴权底线而不是取舍：
> 界面能自签票，等于把「人类在场」这道门自己拆了（票据的意义正是"人类带外持有"）。
> 界面用的「界面凭据」是另一回事——它由界面进程**自己签发并持有**，
> 以"人类亲手点击界面"作为在场证明，从不落盘、不进环境变量。
>
> ⚠️ 票据摘要走的是 `token::summary()`（**脱敏**：只有 `enabled` / `mode` / `scoped` /
> `minutes_left` 四项，**不含** `hash` 与任何原文）。设计成"结构体里就没有可泄的东西"，
> 而不是"实现了脱敏的 Debug"——后者写一次 `{:#?}` 就失效。
> 票据过期时显式显示「已过期」，因为那是 L3 下最常见的失败原因。
>
> TUI 的 `n`（新建需求）从**一步变两步**：① 编号（回车 = 自动编号）→ ② 标题。
> 这是已知的交互代价，已在帮助浮层里写明。

> 界面只做**管理台**：状态读写全部走 core，与 CLI 行为完全等价（不会出现"界面放行、CLI 拦截"）。
> 清单正文在界面中**只读**——正文由 AI/编辑器维护，界面只负责审核决策；
> 想留意见走**评论**（面板/CLI 都行），不要去改正文。
> GUI 启动失败（无图形栈 / wgpu 初始化失败）时会**自动回退到 TUI**（`full` 特性下）。

## 与 gates-toolkit 的关系

- 由 `setup-gates` 装配（片段 `030-reqguard`，`hookctl register ... --priority 30 --mode block`）
- dev-scaffold `new` 默认启用，`--no-req-guard` 关闭
- 拦截脚本随仓库提交（`.gates/hooks/`）——保证 clone 即生效；脚本缺失时 **fail-closed**

## 目录

```
.gates/
├── req-guard.yaml               # 门禁声明
├── requirements/                # 活跃：REQ-00N-*.md 清单 + *.comments.md 评论
├── requirements/archive/<年>/   # 到期归档（done 满 N 天自动搬入；misc/ 存无日期段）
├── hooks/req-guard-check.{sh,ps1}   # 拦截脚本（Claude/CodeBuddy 直连）
├── hooks/req-guard-deny.{sh,ps1}    # deny 包装：Codex/Cursor 拦截编码转 exit 2
├── ci/req-guard-ci.yml          # L3 接入样例（GitHub Actions），复制进 .github/workflows/；install 未接 CI 会提示，--verify 未接入即红
└── audit/gate-audit.log         # 审计（不入库）
```

## 构建与验证

工程为 cargo workspace：`core`（零依赖 lib）+ `cli`（唯一 bin）+ `tui` / `gui`（界面 lib）。

```bash
cargo build --release                  # 默认只构建 core + cli：零 UI 依赖、秒级
cargo build -p req-guard --features tui --release   # 含 TUI
cargo build --release --target x86_64-pc-windows-gnu # 交叉编译（需 mingw-w64 工具链，见「多平台 Release」）
bash scripts/build-release.sh          # 一键多平台 Release 构建（5 目标 × 2 变体）

cargo fmt --all                        # 格式
cargo clippy --workspace --all-targets -- -D warnings   # 静态检查（零警告为门槛）
cargo test --workspace                 # 单元测试（全 workspace；用例数随迭代增长，以实跑输出为准）
python scripts/verify_gate.py          # 拦截脚本真机场景（13 固定场景 + 1 个需二进制在 PATH 的条件场景）
bash scripts/verify_tests_have_teeth.sh   # 判决性实验：确认关键判据真的被测试守着（会临时改工作区文件）
```

### 判决性实验：让「测试有效」可机械检查

「测试通过」与「测试有效」是两件无法区分的事 —— 一个断言方向写反的测试照样绿
（REQ-008 里就有一个）。`verify_tests_have_teeth.sh` 把这条纪律变成命令：对每条
关键判据注入一个「注入它就坏」的变异，跑对应测试，**测试仍全绿就说明没人守**。

```bash
bash scripts/verify_tests_have_teeth.sh                 # 全量清单
bash scripts/verify_tests_have_teeth.sh --only 20       # 只跑第 20 条
bash scripts/verify_tests_have_teeth.sh --only 围栏      # 只跑理由含「围栏」的条目
```

三种结果互斥，任一非 `killed` 都让退出码非 0：

| 结果 | 含义 |
| --- | --- |
| `killed` | 目标测试跑起来了且有失败 → 这条判据有人守着 |
| `survived` | 目标测试跑起来了且全绿 → **这条判据没有任何测试守着** |
| `build_failed` | 编译失败 → **不算守住**（只证明代码与该文件有耦合） |

变异清单在 `scripts/mutation-manifest.txt`，每行 5 段以**单个 TAB** 分隔：
`<文件>\t<锚点原文>\t<追加文本>\t<测试过滤串>\t<理由>`。用 TAB 而非 `|` 是因为
锚点是 Rust 源码行，而闭包参数 `|c|`、`||` 在 Rust 里满地都是。

> ⚠️ **脚本会临时修改工作区文件**。三道保证：注入前检查目标文件是否被 git 跟踪
> 且干净、`trap` 还原、还原后校验内容与注入前逐字一致。任一道失效都以非 0 退出码
> 报出。运行期间不要在别的终端改同一批文件。

新增一条判据：往清单里加一行（**理由必填** —— 那是这份清单里最容易随时间丢失的
东西），再跑一次。清单只能覆盖已经想到的失效方向；「没想到的」靠每次复盘追加。

> **Windows + Git Bash 注意**：`/usr/bin/link`（GNU coreutils）会遮蔽 MSVC 的 `link.exe`，
> 直接 `cargo build` 会失败。需把 MSVC `bin/Hostx64/x64` 前置到 `PATH`，并设置 `LIB`
> 指向 MSVC `lib/x64` 与 Windows Kits 的 `um/x64`、`ucrt/x64`。
>
> **Linux 上跑静态检查 / 单测必须排除 `gui`**：`gui` 的 eframe 关了 `default-features`
> （连带 x11 / wayland 后端），Linux 上编 winit 会直接
> `compile_error!("The platform you're compiling for is not supported by winit")`。
> 加 `--exclude req-guard-gui`（CI 同理，见 `.github/workflows/ci.yml` 的 `GUI_EXCL`）。

## 多平台 Release 构建与发布

流程与 sql-guard 的 CNB 流水线保持一致：**推 tag → 创建 Release → 交叉编译 → 上传附件**。

```bash
# 1) 把 Cargo.toml 的 [workspace.package] version 改到目标版本（如 0.2.0）
# 2) 提交后打 tag（tag 名必须是 v + 该版本号，两者会被流水线交叉校验）
git tag v0.2.0 && git push cnb main --tags
```

CNB 上 `v*` 触发 `req-guard release` 流水线（`.cnb.yml`），三个阶段：

| 阶段 | 动作 |
|---|---|
| 创建 Release | 内置任务 `git:release`，tag/标题 = 触发 tag，`overlying: true`（同 tag 重跑不报错） |
| 构建多平台二进制 | `bash scripts/build-release.sh`，在 Linux x86_64 执行机上交叉编译到 `dist/` |
| 上传 Release 附件 | `cnbcool/attachments` 上传 `./dist/*`（`git:release` 本身不支持附件） |

**产物矩阵**（每目标 2 个变体，命名规则 `req-guard-<target>[.exe]`）：

| target | 平台 | 默认变体（零依赖 CLI） | TUI 变体（CLI + 终端界面） |
|---|---|---|---|
| `x86_64-unknown-linux-musl` | Linux amd64（静态，无 glibc 依赖） | `req-guard-x86_64-unknown-linux-musl` | `req-guard-ui-x86_64-unknown-linux-musl` |
| `aarch64-unknown-linux-musl` | Linux arm64（静态） | `req-guard-aarch64-unknown-linux-musl` | `req-guard-ui-aarch64-unknown-linux-musl` |
| `x86_64-pc-windows-gnu` | Windows amd64 | `…-windows-gnu.exe` | `…-windows-gnu.exe` |
| `x86_64-apple-darwin` | macOS Intel | `…-apple-darwin` | `…-apple-darwin` |
| `aarch64-apple-darwin` | macOS Apple Silicon | `…-apple-darwin` | `…-apple-darwin` |

另附 `dist/SHA256SUMS` 供下载后校验。

- **版本号规则**：Release tag = `v` + `Cargo.toml` 的 `[workspace.package] version`。
  脚本会做一致性检查（不一致只告警不阻断，但产物以 `Cargo.toml` 为准）。
  `req-guard -V` 输出的就是编译期版本，可用于核对下载到的二进制。
- **失败策略**：Linux/Windows 目标是**必需**的（失败即整体失败，避免发布缺件版本）；
  两个 macOS 目标是 **best-effort**（拿不到 Zig/cargo-zigbuild 时跳过，不阻断）。
  TUI 变体默认必需，可设 `REQGUARD_TUI_BEST_EFFORT=1` 降级为跳过。
- **GUI 不参与交叉编译**：`gui`（eframe/wgpu/rfd）在 Linux 侧依赖 X11/Wayland/GTK
  系统库，无法在 Linux 执行机上交叉编译，需由各平台原生执行机构建
  （见 `.github/workflows/ci.yml`，GitHub Actions 已在 Windows/macOS 上构建 gui）。

**本地复现**（任意 Linux x86_64，或 Windows 上单独验证某个 target）：

```bash
# 完整多平台构建（需联网 apt/下载 Zig），产物在 dist/
bash scripts/build-release.sh

# 只验证单个目标的链接配置（不改动 .cnb.yml 也能提前发现问题）
cargo build --release -p req-guard --target x86_64-unknown-linux-musl        # 默认变体
cargo build --release -p req-guard --target aarch64-unknown-linux-musl --features tui

# 校验产物：ELF magic + 架构（3e00=x86-64, b700=aarch64）
od -An -tx1 -N4 target/x86_64-unknown-linux-musl/release/req-guard
```

### 发布包（可分发安装包）

除裸二进制外，还有一条"开箱即用"的分发形态：**组装脚本 + 包内容模板**。

```bash
bash scripts/make-release-package.sh              # 构建 + 组装 + 打包（默认）
bash scripts/make-release-package.sh --no-build   # 复用 dist/raw/ 直接组装
```

产出 `dist/req-guard-v<version>/`（目录）与 4 个归档：

| 归档 | 内容 |
|---|---|
| `req-guard-v<ver>-windows-x86_64.zip` | Windows 三变体 + 全部文档/脚本/模板 |
| `req-guard-v<ver>-linux-x86_64-musl.tar.gz` | Linux amd64 静态二进制（含执行位） |
| `req-guard-v<ver>-linux-aarch64-musl.tar.gz` | Linux arm64 静态二进制 |
| `req-guard-v<ver>-all.zip` | 全平台完整包（镜像/制品库分发用） |

包内结构（`bin/` 平台分目录、`docs/` 文档、`scripts/` 脚本、`templates/` 接入模板）：

```
README.md  VERSION  CHANGELOG.md  SHA256SUMS.txt  THIRD-PARTY-NOTICES.md
bin/{windows-x86_64,linux-x86_64,linux-aarch64}/   预编译二进制（cli / ui / gui 变体）
docs/     安装指南 · 用户手册 · 校验说明 · 常见问题
scripts/  install / uninstall / verify-checksums / selfcheck（.sh 与 .ps1 双份）
```

设计要点（都在脚本注释里写明了原因）：

- **包内容模板放 `packaging/`**（入库），产物落 `dist/`（不入库）——文档与脚本可版本化、可评审。
- **版本自证三处对齐**：`VERSION` 文件 / `req-guard -V` / 归档名，任一不符即视为混装。
- **不动 Python 之外的删除**：组装过程只改名归档、不删文件（旧快照进 `dist/.trash/`），
  因此在带删除保护的环境（CI/沙箱）里也能无人值守跑完。
- **执行位写进归档条目**：Windows 上 `chmod` 是空操作，若只在本地文件系统设权限，
  打出的 tar 里会是 0644；权限位改为在 tar/zip 条目中显式写入（`req-guard`、`*.sh` = 0755）。
- **`.ps1` 一律补 UTF-8 BOM**：否则 Windows PowerShell 5.1 按 ANSI 读，中文乱码 + 解析报错。
- **`{{占位符}}` 由构建脚本注入**（版本/日期/提交/工具链），文档不用手改。
- 收尾自动跑一遍包内 `verify-checksums.sh`，27/27 通过才算组装成功。

用户侧的安装、校验、自检分别由 `scripts/install.{sh,ps1}`、`verify-checksums.{sh,ps1}`、
`selfcheck.{sh,ps1}` 完成；`selfcheck` 会实跑"未过审必须拦截 / 过审必须放行"两条路径。

**如何验证发布结果**

| 检查项 | 方法 | 期望 |
|---|---|---|
| 附件齐全 | Release 页面 / `cnbcool/attachments` 的 `FILES` 输出 | 10 个二进制 + `SHA256SUMS` |
| 完整性 | `sha256sum -c SHA256SUMS` | 全部 OK |
| 版本号 | `./req-guard-<target> -V` | `req-guard <tag 版本号>` |
| 可用性 | Linux：`./req-guard-x86_64-unknown-linux-musl check`；Windows：`req-guard-…-windows-gnu.exe check` | 正常输出门禁判定（退出码 0/1） |
| 静态性 | `ldd req-guard-aarch64-unknown-linux-musl` | `not a dynamic executable` |


## 文档

设计与规范文档统一放在 [`docs/`](docs/README.md)：

| 目录 | 内容 |
| --- | --- |
| [`docs/需求/`](docs/需求) | 需求分析报告（功能范围 / 命令规格 / 验收口径） |
| [`docs/设计/`](docs/设计) | 技术方案、UI 架构细化方案、GUI 跨平台方案选型 |
| [`docs/规范/`](docs/规范) | AI 工具合规保证规范（三层 + 审批锁 / 部署验收清单） |
| [`docs/提案/`](docs/提案) | AI 协同审核 GUI 自动弹出（未实现，含 P0–P5 落地路径） |

完整索引与文档清理记录见 [`docs/README.md`](docs/README.md)。

## 源码结构

```
core/  req-guard-core   零依赖 lib：error / requirement / comment / gate / status / ui_mode
cli/   req-guard        唯一 bin：main + cli（参数解析）+ render（文本渲染）
tui/   req-guard-tui    终端界面 lib：app（状态机）+ ui（渲染）
gui/   req-guard-gui    桌面界面 lib：app + fonts（内嵌 Noto Sans SC 子集）
```

判定逻辑只有一份，在 `core`；三个前端只负责渲染——这是"GUI 与 CLI 不会行为漂移"的根本保证。

中文字体：`gui/assets/NotoSansSC-Regular.otf`（Noto Sans SC 子集，SIL OFL 许可，见
`gui/assets/OFL-NOTO.txt`）；用 `scripts/make_font_subset.py` 可重新生成。

详细设计见 [`docs/设计/技术方案.md`](docs/设计/技术方案.md)、[`docs/需求/需求分析报告.md`](docs/需求/需求分析报告.md)；
架构决策见 dev-scaffold `架构决策记录.md` ADR-001。

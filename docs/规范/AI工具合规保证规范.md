# 保证 AI 工具合规实施规范（req-guard）

> 配套：`docs/设计/技术方案.md`、`docs/需求/需求分析报告.md`、`docs/设计/UI架构细化方案.md`。
> 本文定义 req-guard 在"多 AI 工具共存"环境下的**保证模型**与**部署/验收清单**，
> 明确：门禁保证什么、不保证什么、每一层必须如何配置才能算"真正生效"。
> 适用范围：任何接入 req-guard 的项目、任何会写代码的 AI 工具（Claude Code / CodeBuddy / Codex / Cursor 及未来工具）。

> **状态校注（2026-09-30 复核）**：本文各章"现状（已实现）"与当前代码一致，无需修订——
> L2 fail-closed（`core/src/gate.rs::PRE_COMMIT_BLOCK`）、`enforce.ci: true`、`install --verify`、
> 审批锁方案 A（`REQ_GUARD_AI_CTX`）/ B（`token`，SHA-256 落盘）/ C（`--oob`、`REQ_GUARD_OOB_ONLY`）
> / **D（终端挑战码在场证明）**、
> **审批严格等级 `auth.level` 0–3**（L1 签发须人类 TTY、L2 凭据只认进程内传入、L3 一次性范围票据；
> 旧键 `auth.strict: true` 等价 L1）、
> 入库台账 `audit/ledger.md` + `audit-digest`、四工具原生 hook schema（Codex/Cursor 走 `deny` 包装）
> 均已落地。本文是**部署与验收的唯一权威口径**，改动门禁保证能力时须同步更新第 2.2、4、6 章。

---

## 1. 目的与范围

req-guard 是 gates-toolkit 的流程门禁，管"AI **该不该写**"。本规范解决一个具体问题：

> **如何保证"每一个"接入项目的 AI 工具都遵守三段审核流程，而不是只管住部分工具？**

答案不是"把 L1 hook 注入得更多更全"，而是**把保证建立在工具无关、AI 绕不过的边界上**，并把 L1 明确定位为"体验层"而非"保证层"。

---

## 2. 信任模型（必须先读）

### 2.1 req-guard 保证什么

| 保证项 | 含义 | 依赖 |
|---|---|---|
| 状态不可被"正文编辑"骗过 | 正文随便改，脚本只读 `GATE:*` 标记行 | 三段不全 `approved` 即拦 |
| 评论证据不可被 AI 直接篡改 | 独立 `*.comments.md` 文件 + L1 第 0 步拦截 | L1 生效时 |
| 绕过有痕、有时效 | `.gates/.bypass` 带 `expires_epoch`，命中即审计 | — |
| 缺失即最严 | `strict_order`、`StepState` 未知值一律按最严解释（fail-closed） | — |

### 2.2 req-guard **不**保证什么（关键边界）

> **req-guard 是"纪律/流程强制工具"，不是"密码学/OS 级安全边界"。**
> 其硬边界建立在以下信任假设上；任一假设不成立，对应保证即失效：

| 假设 | 不成立时的后果 | 失效层级 |
|---|---|---|
| AI 只通过被 hook 管控的 `Write\|Edit\|MultiEdit\|NotebookEdit` 写文件 | AI 改用 Shell（`cat >`/`tee`）或 `req-guard approve` 自批 → 绕过 | L1 失效 |
| 审核人是**真实人类**，在 AI 执行上下文之外操作 | AI 自批/自结（`--reviewer` 曾仅字符串、`resolve` 仅拒 `author=ai` 字面量） | 命令层失效 |
| 审批人姓名与 **git 身份**一致（`auth.level ≥ 1` 时强制） | AI 用任意 `--reviewer` 冒名 → 审批记录无法归属到真实提交者 | 审批记录归属失效（见 §4.4 身份绑定） |
| L2 pre-commit / L3 CI 真正执行 | 脚本缺失被跳过（`if [ -f ]` 静默 fail-open）、CI 默认 `enforce.ci:false` | L2/L3 失效 |
| 各 AI 工具确实加载了注入的 hook | 工具 schema 不识别、已存在配置未合并 → 静默无 hook | L1 静默缺失 |
| **CI / pre-commit 能取到「本次变更集」** | 多需求仓库下裁决对象是**变更集**（按各清单 `GATE:TOUCH` 声明反查归属）。取不到变更集（CI 漏 `--base`、pre-commit 无暂存集）就无从归因 → 报歧义并拦截 | L3 恒拦（非漏拦，但会表现为"门禁坏了"） |
| **AI 的写操作能被逐次归因** | 裁决按**单文件**判定；`Bash` 里的 `sed -i`、格式化器批量改写不带 `file_path`，PreToolUse 阶段不可见 | L1 覆盖不到（由 L2/L3 的变更集兜底） |

**设计推论**：要让"每个工具"守规矩，必须做到——

1. **状态核验放在工具无关的边界**（git pre-commit + 服务端 CI），不依赖具体工具；
2. **审批动作必须鉴权**（人类持有、AI 环境拿不到的凭据），否则状态可被欺诈性置为 `approved`，L2/L3 照单全收；
3. **L1 仅作 UX 快失败**，其缺位不得削弱 L2/L3。

> **落地进展（2026-09）**：§4.2 L2 fail-closed、§4.3 `enforce.ci` 默认 true、
> §4.4 **方案 A + 方案 B + 方案 C + 方案 D + 严格等级 `auth.level`（0–3）**
> （`REQ_GUARD_AI_CTX` 软标记 + `token issue/status/revoke`
> 审批凭据 + `--oob`/`oob <命令>` 带外声明与 `REQ_GUARD_OOB_ONLY` 强制开关 + **严格模式下
> "无凭据即拒"与终端挑战码** + **L1 禁 AI 自签 / L2 断环境变量通道 / L3 一次性范围票据**）、
> §4.5 `install --verify`、§4.6 入库台账 + `audit-digest` 已实现；Claude/CodeBuddy/Codex/Cursor
> 已各自注入**原生 hook schema**（Codex/Cursor 走 deny 包装适配其 exit 2 拦截语义）——见第 8 章附录。
> 方案 C 目前为流程化带外（渠道声明 + 留痕），非对称签名需打破"零外部依赖"（见 §7）。
> **方案 D 的由来**：2026-09-29 Firedit 事故——WorkBuddy 未登记在注入列表、其会话内无
> `REQ_GUARD_AI_CTX`，"没标记 = 人类"导致 AI 自批成功。判级已反转为 fail-closed（详见 §4.4）。

---

## 3. 保证架构：三层定位

```
AI 工具 A / B / C / ...（未知工具也算）
        │
   [L1 PreToolUse]  ── 体验层（快失败，非保证）
   matcher: Write|Edit|MultiEdit|NotebookEdit
        │  越靠前越硬，但只覆盖"用这些工具写文件"的路径
        ▼
   AI 改写源码 / 清单
        │
   [L2 git pre-commit]  ── 墙①（工具无关，fail-closed）
   重跑同一脚本，未过审 exit 1
        │
   [L3 CI / 合并门]  ── 墙②（工具无关，服务端独立，必需检查）
   req-guard check 作为 required status
        │
   合并入主干
        ▲
        │
   [锁] 审批动作鉴权：approve/resolve/bypass 须人类凭据
        │             ← 保证"状态是被人类合法置为 approved"，而非 AI 自批
```

**一句话**：L1 是提醒，L2/L3 是墙，审批鉴权是锁。墙只认状态、锁才认人——两者缺一不可。

---

## 4. 各层部署规范与校验清单

### 4.1 L1 —— 体验层（不做"保证"要求，但要做"厚"）

**定位**：AI 一写就被拦，反馈及时；**不计入保证**，其缺失由 L2/L3 兜底。

**部署要求**：

| 项 | 规范 | 代码/配置锚点 |
|---|---|---|
| 支持工具 | 至少覆盖 `claude` / `codebuddy` / `codex` / `cursor` | `core/src/gate.rs` `TOOL_PROFILES` |
| matcher | 各工具原生工具名：Claude/CodeBuddy/Cursor `Write\|Edit\|MultiEdit\|NotebookEdit`；Codex `apply_patch` | `gate.rs::hook_json()` 按工具渲染 |
| 拦截编码 | Claude/CodeBuddy 直连 `check`（exit 0/1）；Codex/Cursor 走 `deny` 包装（exit 2，因这二者把非 exit 0 视为 fail-open） | `gate.rs::DENY_SH/DENY_PS1` |
| 幂等 | 已含该工具 marker（`req-guard-check` 或 `req-guard-deny`）则跳过 | `inject_tool()` + `marker_of()` |
| 冲突 | 已存在配置**不含** hook 时，**不得静默跳过** | 提示手工合并 + `install --verify` 在 CI 检出（见 4.5） |

**校验清单（L1）**：
- [ ] 每个在用 AI 工具的配置文件含对应 hook marker（`check` 或 `deny`），且路径为该工具原生位置
- [ ] 配置 schema 与该工具原生格式一致（非仅 Claude 风格）——见第 8 章附录
- [ ] 未知/新工具出现时，CI 能检出"缺 hook"并红（见 4.5）
- [ ] 明确记录：L1 **不覆盖** AI 经 Shell 写文件 / 自调 `req-guard approve` 的路径

### 4.2 L2 —— pre-commit 真 fail-closed（墙①，工具无关）

**现状（已实现）**：`core/src/gate.rs` 的 `PRE_COMMIT_BLOCK` 为
```sh
if [ ! -f .gates/hooks/req-guard-check.sh ]; then
  echo "✗ req-guard 门禁脚本缺失……提交已被阻止。" >&2
  exit 1
fi
sh .gates/hooks/req-guard-check.sh || exit 1
```
脚本缺失即**拦截提交并提示初始化**（fail-closed），与 gates-toolkit `030-reqguard`
片段语义一致；真机回归见 `scripts/verify_gate.py` 场景 11。

**判定归属（2026-10 起，REQ-006）**：主门禁脚本已收缩为「**只取参与渲染**」——
它读 payload / 暂存集，然后调用 `req-guard check`（判定全在 `core`）。带来三条规范变化：

| 变化 | 规范要求 |
|---|---|
| `req-guard` 二进制不在 PATH 即**全拦** | 容器 / CI 镜像必须把二进制放进 PATH；`install --verify` 会检出脚本是否仍自带判定（`sort -r` / `GATE:STEP` / `verify-content` 出现即报缺口） |
| `req-guard check` **不再执行**仓库里的 hook 脚本 | 服务端判定不依赖某台机器装没装 hook；脚本与 core 版本漂移不再影响 L3 裁决 |
| 脚本的 `HOOK_PS1`（Windows）需与 `HOOK_SH` 同步 | 两侧都改；`verify_gate.py` 的文本断言锁住"脚本不再自带裁决" |

> **为什么这么改**：判定散落到 shell 与 core 两处的那一刻起，它们就会漂移；
> 而门禁最坏的失效不是"报错"，是"看着在拦、其实没拦"。代价是 sh/ps1 双份镜像——
> 已通过"判定只在 core、脚本无分支"把镜像面缩到最小。

**规范**：
- 脚本/清单缺失 → `exit 1` 并提示初始化；
- **二进制缺失 → `exit 1`**（判定在 core，缺它即无从判定）；
- 追加时幂等、不覆盖既有 hook（保留）；
- 与 gates-toolkit `030-reqguard` 片段语义对齐（片段侧应为 fail-closed）。

**校验清单（L2）**：
- [ ] 在干净仓库删除 `.gates/hooks/req-guard-check.sh` 后 `git commit` → **必须失败**
- [ ] 三段未批时 `git commit` → `exit 1`，无提交产生
- [ ] 本机 `git commit --no-verify` 仍被**服务端 push rule** 抵消（见 4.3）

### 4.3 L3 —— CI 强制门禁（墙②，服务端独立）

**现状（已实现）**：`REQ_GUARD_YAML` 模板默认 `enforce.ci: true`，`req-guard install`
输出 CI 接入提示，`.gates/README.md`（随项目生成）含部署指引；流水线侧的
必需状态检查与分支保护仍需各项目按指引自行配置。

**规范**：
- 默认 `enforce.ci: true`；流水线显式调用 `req-guard check --base "origin/<base>"`；
- **`--base` 不可省**（REQ-006）：裁决对象是「本次变更集」，裸 `check` 在多份未归档清单下
  无变更集可依 → 报歧义并恒拦。`install --verify` 会检出 CI 模板的 `check` 步骤是否带 `--base`；
- 该检查为**必需（required）**状态检查，不通过禁止合并；
- 分支保护：禁止直推 `main`、禁止绕过本机 hook 的提交（服务端再次跑 check）；
- 本机环境无关——开发者用任何工具、任何 OS，服务端都独立重跑。

**校验清单（L3）**：
- [ ] 三段未批的 PR → CI 红、无法合并
- [ ] 直接 push 到受保护分支 → 被服务端拒绝
- [ ] CI 镜像内含 `req-guard` 二进制（见构建产物矩阵）且版本与 tag 一致
- [ ] `req-guard install --verify` 全绿（含"CI 模板含 `check --base`"与"脚本已委托 core"两项）

### 4.4 锁 —— 审批动作鉴权（最关键，堵自批）

**现状（方案 A/B/C/D + L3 一次性票据已实现）**：`core/src/auth.rs` 提供 `ensure_human()`，在
`requirement::review`（approve/reject）、`comment::resolve`、`requirement::done`、
`requirement::archive_*`、`gate::bypass` 入口统一判定（core 层生效，CLI/TUI/GUI 同约束）：
- **方案 A**：`install` 给支持会话 env 的工具（claude 与 codebuddy）注入
  `"env": {"REQ_GUARD_AI_CTX": "1"}`；检测到该标记即拒；
- **方案 B**：`core/src/token.rs` 签发凭据，原文仅打印一次、由人类带外持有
  （存密码管理器/自身会话）；库文件只存 **SHA-256 哈希**于 `~/.config/req-guard/guard.cfg`（0600）。
  - `static`（L0–L2）：短期令牌，TTL 内可重复使用；
  - `scoped`（L3）：**一次性范围票据**，`scope=<需求ID>:<步骤>` 绑定审批对象，
    **用后即废**（`used=true`），同一张票无法重放。
- 未启用凭据时自动回退方案 A；`token status/revoke` 查询与撤销。
- **方案 C**（已实现，流程化带外）：`approve/reject/resolve/bypass` 可显式声明带外
  渠道（`--oob` 或 `req-guard oob <命令>`）；设 `REQ_GUARD_OOB_ONLY=1` 时强制"仅接受
  带外审批"（未声明即拒，fail-closed）。审批事件按 `channel=oob|interactive` 写入入库台账。
  零依赖下**不引入非对称数字签名**——C 的隔离与留痕来自"渠道显式声明 + 审计标注入库"；
  若需独立签名者（私钥带外、公钥验证）的密码学强隔离，须为 core 引入密码学依赖（见 §7 边界）。

- **方案 D**（**人类在场证明**，fail-closed）：审批必须出示凭据，或在**真实终端**输入一次性
  挑战码。物理依据：AI 的执行通道是管道，**实测 stdin/stdout/stderr 均非 TTY**，既看不到
  交互提示也无法回读挑战码 → 天然被拒；且不依赖"AI 工具是否已登记"，新工具不登记也不会
  静默失效。审批事件在台账里带 `tty=`/`ai=` 通道指纹，`tty=0` 即非人类终端。

**审批严格等级 `auth.level`（0–3，逐级包含）——判定的主轴**：
配置在 `.gates/req-guard.yaml`（随仓库版本化、改动 PR 可见，`init` 模板默认 L3）。
环境变量 `REQ_GUARD_AUTH_LEVEL` / `REQ_GUARD_STRICT_AUTH` 只能**抬高**、不能降低
（取 max），避免自身成为 AI 的降级开关。

| 等级 | 签发（`token issue/revoke`） | 审批（approve/reject/resolve/done/archive/bypass） |
| --- | --- | --- |
| **L0** | 仅方案 A 软标记 | 已启用凭据模式才要求凭据；否则放行（旧行为，不打断存量项目） |
| **L1** | **必须人类在真实终端**（TTY 挑战码） | 无凭据即拒（凭据 / TTY 挑战二选一） |
| **L2** | 同 L1；GUI/TUI 走**进程内签发** | L1 + 凭据只认**进程内显式传入**，不读 `REQ_GUARD_TOKEN` 环境变量 |
| **L3** | 同 L2，且签发的是一次性票据 | L2 + 凭据必须是**一次性范围票据**（静态令牌不受理） |

**为什么必须有 L1（堵自签）**：凭据是方案 B 的信任根。若 `token issue` 无人看守，AI 自签
凭据再自批，方案 B 一招即破（2026-09-29 发现的设计缺陷）。L1 起签发/撤销必须过 TTY 挑战，
AI 的管道通道拿不出。

**为什么必须有 L2（断环境变量）**：`export REQ_GUARD_TOKEN=…` 会被同一 shell 会话里的
AI 子进程**继承**，等于把凭据交给 AI。L2 起凭据只认进程内显式传入（CLI `--token` 参数、
GUI/TUI 内存字段），凭据不出人类进程。

**为什么必须有 L3（抗重放）**：静态令牌在 TTL 内可被重放、且与审批对象无关。L3 改为一次性
范围票据（绑定 `<需求ID>:<步骤>`，用后即废）。**不采用"口令 + HMAC 挑战应答"**：核验方若要
本地校验 `HMAC(口令, 挑战)`，就必须把口令（或等价密钥）落到与 AI 同用户可读的存储里，
AI 直接照算即可伪造——那是安全剧场。零依赖下，一次性票据才是可落地的抗重放手段。

**为什么必须有 D（2026-09-29 Firedit 事故）**：方案 A 是**白名单注入**——`install` 只往
claude / codebuddy / codex / cursor 四个 profile 写配置，WorkBuddy 不在列表且**不读**
`.codebuddy/settings.json`、`~/.workbuddy/settings.json` 也无 env/hooks 段 → AI 会话内
无任何 `REQ_GUARD_*` 变量 → `ensure_human` 判定"非 AI 上下文"→ approve 直接放行。
即**"没标记 = 人类"是 fail-open**：工具未登记 = 审批锁静默失效。
判级因此反转为 **"无凭据即拒"**（L1 起），而非"无标记即人类"。

**判级顺序**（`core/src/auth.rs::authorize`，正交叠加）：
1. AI 上下文标记非空 → 拒（A，任何等级都生效）；
2. **L0**：已启用凭据模式 → 必须出示有效凭据，否则放行（旧行为）；
3. **L1+**：凭据有效 → 放行；否则 TTY 在场证明 → 放行；两样都没有 → 拒（fail-closed）；
4. `REQ_GUARD_OOB_ONLY=1` → 必须已声明带外渠道（C）。

**凭据管理自身的审批锁**：`token issue/revoke` 同属审批类动作（`ensure_token_admin`）——
AI 若能自签凭据即可"自签→自批"，方案 B 一招即破。规则：已有生效凭据时重签/撤销
**必须出示当前凭据**（阻止静默替换）；尚无凭据时 L1+ 要求人类在场。
**GUI/TUI 是例外且是解法**：它们是**人类亲手启动并点击**的界面进程，界面里的确认动作
即由界面进程**进程内签发**凭据（`auth::ui_issue_credential`），凭据全程只在该进程内存里，
不经 stdout、不经环境变量、不落剪贴板——AI 无法凭空拉起一个带人类交互的界面，也无法点击
按钮。这同时解决了两个问题："AI 拿不到凭据"与"GUI 不必先 `export REQ_GUARD_TOKEN`"。

#### 批准必须绑定内容：内容冻结（`sum=`）

`GATE:STEP status=approved` 只锁**状态行**，不锁**内容**：批准人与批准时刻之间没有绑定。
本项给 `GATE:STEP` 加一个 `sum=` 字段（被批准那一段的 SHA-256），判定时重算比对。

- **摘要只覆盖该步对应的那一段**，不覆盖整篇。理由是鸡生蛋：批准任何一步都会改写
  文件头并追加 `## 审核记录`，整篇摘要在批准后面试盘
- **归一化只做两件事**：行尾统一（走 `str::lines()`，CRLF/LF 天然一致）与尾部换行。
  **刻意不去空白** —— 任何宽容都会让真实改动藏起来
- **三个强制点**：`doc_write_guard` 写入即拦（最强，改都写不进去）、
  `gate_check` 重算比对（pre-commit / CI / 手动 `check` 同一份判定）、
  `HOOK_SH` 第 3.5 段兜底直跑脚本的路径（缺二进制即 fail-closed）
- **`req-guard check` 自己就是那个二进制**，所以它经 core 自查并置
  `REQ_GUARD_SUM_CHECKED`，脚本据此跳过自己那段 —— 不让入口反过来依赖自己在 PATH 上
- **合法改法只有一条**：`reject`（清 `sum=`）→ 改 → 重新 `approve`。
  **不提供 `unseal`**：多一个逃逸口就多一条绕过路径
- **摘要跟着状态走**：`touch --declare` 打回 solution 时同步清 `sum=`，
  否则旧摘要会继续"保护"一份已被改动的正文 —— 那正是本项要消灭的形态
- **存量**：无 `sum=` 的已批准段 → **报 Warn 并明示**（不静默跳过，否则等于
  "看起来有冻结、实际没有"）；`req-guard seal <ID>` 绑定当前内容，**AI 禁止执行**
  （否则等于自证已批内容未变）

#### 代码围栏内的 `## ` 不是段落边界（REQ-008）

`section_span` 决定「哪几行属于第 N 段」，因而决定 `GATE:TOUCH` 声明读哪一段、
内容摘要覆盖哪一段、`EmptySection` 统计哪一段。它的边界判据是
「`trim_start()` 后以 `## ` 开头」—— **代码围栏里的 `## xxx` 是示例内容，
会被误当成真标题**。缩进的示例（放在列表项下，实际文档里几乎都是缩进的）
同样命中。

后果极具迷惑性：声明块、摘要、`EmptySection` 判定会**一起**指向错误段落，
而报错（`MissingBlock`：你没写 TOUCH 块）与真实原因完全无关。实测起草 REQ-007
时在正文里写了「草稿格式形如 `## solution`」就触发了它 —— 而且**四条判定里
只有 `touch-check` 报了错，另外三条静默地判错了段落**。

故段落定位前先做**围栏掩码**（`section::mask_fenced`，与
`touch::mask_html_comments` 共用一份，掩成等长空格以**保留行号**）：

- 支持 ``` 与 ``` 两种围栏；
- **变长围栏**：开围栏取连续反引号/波浪号的长度，闭围栏要求长度不少于它 ——
  只认长度 3 会让「文档里展示三反引号示例的文档」再次切错边界；
- **未闭合围栏按持续到文末处理**（fail-closed）：少一个收尾标记就让围栏内所有
  `## ` 变回标题，正是要消灭的形态。

围栏外的 `## ` 仍算边界 —— 掩码把真边界也吃掉的话，三段就永远分不开。

#### 草稿叠加区与一次命令修订（`apply`）

实施期改一段清单原本要人工跑两趟：`amend`（清摘要、打回待审）→ AI 改 →
`approve`（写新摘要）。**「一次命令完成 amend + approve」不能独立成立** ——
两者之间必然夹着「编辑」，而 AI 改不了已冻结的段（写入守卫会拦）。

草稿叠加区就是那个「编辑」的落点：AI 把拟改内容写进 `.gates/drafts/<需求ID>.draft.md`
（行内标记 `{step=<段名>}` 分段），**该文件对所有判定不可见** —— 写入守卫、
实质正文统计、`ac check`、摘要比对，四处一律不参与。于是已批准的正文一个字节都没变，
而 AI 可以自由迭代措辞。

`req-guard apply <需求ID> --step <步骤> --comment <意见>` 一条命令完成
「读草稿 → 校验 → 写正文 → 重新批准 → 写新摘要」，人工只介入一次。

三条不可让步的约束：

- **apply 由人执行**，走 `ensure_human`，AI 上下文直拒。AI 只能写草稿。
  一旦 AI 能自己把草稿变成 approved，审批锁与内容冻结同时失去意义。
- **复用既有的 `review(pass=true)`**，不自建判定。AC 合规、摘要写入、
  `GATE:TOUCH` 有效性、交叉引用有效性、身份绑定、票据消费全部自动生效 ——
  自建一套就会立刻出现「apply 路径绕过某条检查」这类最难发现的漏洞。
- **先校验后落盘**。校验失败时磁盘逐字不变；半落盘会留下「正文改了、状态没改」
  的中间态。草稿只提供**散文部分**，段内的 `GATE:AC` / `GATE:TOUCH` 块
  原样保留保序 —— 让草稿整段覆盖会把声明块冲掉，那等于 apply 绕过声明侧校验。

台账落 **AMEND + APPROVE 两条**事件：apply 是一次真实返工，只记 APPROVE 会让
REQ-004 的返工率指标漏计它 —— 指标漏计比不指标更坏，那会让「这段很少返工」变成假象。

草稿区进 `.gitignore`：它是工作态，可能含尚未成形的表述。

#### 承诺的两种改法：`reject` 与 `amend`

内容冻结落地后，实施期必然出现"测试跑出来的反馈要求改文档"。合法路径只有一条
（清 `sum=` → 改 → 重审），但**它有两个名字，语义不同**：

| | `reject`（驳回） | `amend`（修订） |
| --- | --- | --- |
| 表达的意思 | 这段被否决 | 方向没错，只是要改内容 |
| 段状态 | `rejected` | `amended` |
| `sum=` | 写 `-` | 写 `-` |
| `--comment` | 必填 | 必填 |
| 是否必须重审 | 必须 | **必须** |
| 台账事件 | `REJECT` | `AMEND` |

机制上两者是**同一条路径**（`core/src/requirement.rs` 的 `review_inner`），
只在段状态标签与台账事件名上分开。分开的目的不是好不好看，而是**可度量**：
按 (需求, 段) 统计 amend 次数即得返工率，反复返工的段说明方案当初没想清楚。
`status` 会展示这个计数（有记录才显示，不刷屏）。

刻意**不给 `amend` 任何额外能力**（免重审、免 comment）：一旦有，它立刻变成
`reject` 的绕过口，两个动作会迅速合并回去。

#### `seal` 只有一种正当用途：存量清单首次启用

`seal` 把已批准段的摘要重绑到当前正文，**跳过重审**。它的正当用途只有一个 ——
REQ-002 上线前批准的存量清单补绑定（三段 `sum=` 全为 `-`）。

已绑定过的清单再 seal，必须给 `--reason`，且台账记 `RESEAL` 而非 `SEAL`。
判据取"是否已绑定过"这个**事实**，而不是次数上限：上限仍在鼓励"攒到上限前多擦几次"。
正常路径始终是 `reject` → 改 → `approve`，那会自动写入新摘要。

#### 设计文档的漂移靠交叉引用存在性兜住

`docs/设计/` 下的设计文档**不加内容冻结**（它是持续演化的"记录"，冻结它会逼着人
为了改措辞去重审需求清单）。改为机械判定**交叉引用存在性**：

- 约定写法：行内 `docs/<目录>/<文件>.md §<编号>`。**只认这一种** ——
  判定必须可机械解析，约定不明确时误报会让整条规则连同其他规则一起被忽略。
- 校验两件事：目标文件存在；文件内存在该编号的小节标题。
- 两种失效用**不同措辞**（文件不存在 / 小节不存在），因为修法不同。
- HTML 注释与围栏代码块里的引用**不判定**：那是模板示例，不是作者承诺。
- 该判定挂在**技术方案批准**上（`ensure_cross_refs_ok`）：挂在命令上要人记得跑，
  挂在钥匙上则物理上无法批出去一份引用已失效的方案。

#### `install` 生成物：豁免由「install 写了什么」派生，不手写清单

`req-guard install` 会往 `.claude/settings.json`、`.codebuddy/settings.json`、
`.codex/hooks.json`、`.cursor/hooks.json` 写 hook 配置。这些文件**不是任何人写的
代码**，却被变更范围门禁判成「不在任何需求的声明里」—— 每份清单都会冒出来。

手写四条豁免是错的：那等于把「install 写了哪些文件」这个事实**复制一份**到别处，
工具清单一改（新增工具 / 改名 / 改路径）就漂移。故豁免集 = 默认项 ∪ **由 install
的写入目标派生**（`touch_exempt_patterns` 与 install 共用同一份 `TOOL_PROFILES`）。

两条硬约束：

- **精确到文件，不给目录通配**。`.claude/**` 会把工具自己的 `CLAUDE.md`、
  `settings.local.json` 一并放行 —— 那正是绕过声明的口子。
- **豁免只放宽「是否需要声明」，不放宽任何内容判定**。这些文件不含清单正文，
  不参与 `ac check` / `verify-content` / 审批锁。

同时它们进 `.gitignore`（同样由 `TOOL_PROFILES` 派生、根锚定）：可再生派生物入库
即多一份真相源，且会让 `git status` 常带噪声，久而久之连真正该看的改动也被淹没。
新 clone 的人本来就得跑 `req-guard install`，否则门禁不生效 —— 这不是额外成本。

顺带修掉一个同源尖角：`touch.exempt` 配置原先**整体替换**默认集，于是「加一条豁免」
必须把默认 5 条逐条抄回，漏抄一条就把 `.gates/**` 变成未声明文件。现改为**追加** ——
默认 5 条全是「本来就不该被声明」的项，没有人有理由主动收窄它们。

#### 清单内容不得是占位空话（`core/src/section.rs`）

三段清单里第 3 段有 `GATE:AC`（A1–A12）、第 2 段有 `GATE:TOUCH`（T1–T6），
**第 1 段曾完全无内容校验**：`create` 出来的模板占位原样留着也能过审。
本项把"某段实质正文为 0"变成机械判定（`AcIssueKind::EmptySection`）。

- **什么不算实质正文**：空行、HTML 注释整块（含跨行注释）、Markdown 标题、
  表格分隔行、水平线、块引用行、与模板占位文案同源的 checkbox 行（**勾选也不算**）
- **GATE 标记块内的条目不算**：AC 条目由 A8 管、TOUCH 声明由 T1 管。
  排除之后 A8 命中时本规则必然也命中 —— 两条独立路径指向同一结论，
  不会出现「只有一个空 AC 骨架的段落被判为有内容」这种互相掩护
- **占位判定用「整行相等」而非前缀匹配**：作者会在占位文案后继续写，
  前缀匹配会误杀真实内容 —— 这是最危险的误报方向
- **占位文案从模板常量派生**（`requirement::template_placeholder_texts`），
  不另写副本；副本等于给文档漂移开一个口子
- **无行数下限**：3 行写到的比 50 行复述有价值
- **强制点**：既在 `ac check` 报，也在 `approve` 时拒绝该步
- **存量影响**：存量清单中"某段无实质正文"者新增报错（AC-011 记录：对既有
  合规清单零新增报错）

#### 规格绑定代码：`source_refs` 声明门禁（`core/src/specmeta.rs`）

身份绑定保证"这条审批属于谁"；本项保证"这份规格与哪些代码绑在一起"。两者正交。

- **声明位置**：清单 frontmatter，字段名与 doc-guard 的 FRS 族**逐字对齐**
  （`doc_type`/`tier`/`owner`/`review_policy`/`verified_at`/`source_refs`），
  不用映射层。`req-guard create` 直接生成骨架；置 `review_policy = codebound`
  后 doc-guard 会追加要求 `verified_at` + `source_refs`，并用 FRS003（落后 HEAD）/
  FRS004（源已改而规格未改）/ DRF001（路径不存在）持续复核
- **为什么必须在 req-guard 侧也拦**：FRS001 只检查 `source_refs` 这个**键是否存在**，
  `source_refs: []` 一律通过；FRS004 需要非空列表才能匹配变更集。于是"声明为空"
  这条路径在 doc-guard 侧**完全静默**——规格看似接入时效治理，实则永远不会被判过期。
  故声明侧的门禁只能放在批准动作上
- **卡在第二段（技术方案）而非第一段**：需求分解阶段还在澄清背景与目标，
  往往还不知道要改哪些文件；到"涉及的文件与模块清单"这一步，声明范围才成为可核对的契约。
  卡第一段只会把 AI 逼去填占位值
- **判级与身份绑定一致**（L1+ 拒绝 / L0 放行并告警）：AI 也能调 approve，
  放在 L0 等于没有门禁；但 L0 必须放行，否则所有存量项目（清单创建于本版本之前、
  根本没有 frontmatter）会一步卡死在 approve 上
- **不引 doc-guard 依赖**：两个 crate 各自独立发版。把 doc-guard 挂进 req-guard 的
  依赖图意味着 doc-guard 挂了会导致 req-guard 的 approve 也用不了——这个方向的反依赖
  不能接受。需要共享的是**字段名与取值枚举**，不是代码
- **诚实边界**：本项只保证"规格声明了它绑定哪些代码"。规格**写得对不对**
  （验收标准是否可度量、技术方案是否可行）仍不在机械校验范围内，
  那是 §4.4 人工审批要兜的部分

#### 身份绑定：把 `--reviewer` 锚定到 git 身份（`core/src/identity.rs`）

方案 A/B/C/D 解决的是"**动作**由人类发起"；本项解决"**这条审批记录属于谁**"。
在此之前 `--reviewer` 是自报字符串，台账里 `reviewer=寇工` 无法证明是谁，也无法区分
"人类审批"与"AI 随手填个名字"。

- **身份来源**：`git config user.name` / `user.email`（即提交本身的署名，随提交进入版本历史，
  事后无法悄悄改写）。`--reviewer` 省略时**自动取**该身份，不再要求手打。
- **指纹 `sig`**：`sha256("req-guard-id-v1\0name\0email\0<仓库绝对路径>")` 前 12 位，
  用 core 自带的 [`digest::sha256_hex`](../../core/src/digest.rs)（零外部依赖）。
  **纯派生值、无密钥、人人可重算**——它的作用是让台账里每条审批都带一个可离线复算的
  一致性锚点，便于事后批量核对"这条审批出自哪个身份"，**不是防伪签名**。
  仓库绝对路径参与派生：同一个 `sig` 在别的 clone 上照抄无意义（可归属性不跨仓库传播）。
- **落盘位置**：`GATE:STEP` 标记行新增 `email=` / `sig=`，"审核记录"区块写成 `姓名 <邮箱>`，
  入库台账 `.gates/audit/ledger.md` 的事件行追加 `email=` `sig=`（冲突时再加 `mismatch=1`），
  `.gates/.bypass` 同样记录。三处都可 PR diff 复核。
- **判级矩阵**（与 `auth.level` 同一判级器）：

  | 情形 | L0 | L1+ |
  | --- | --- | --- |
  | 身份一致（含"用邮箱当 `--reviewer`"） | 放行 | 放行 |
  | `--reviewer` 与 git 身份冲突 | 放行，台账标 `mismatch=1`（事后可批量捞出） | **拒绝** |
  | 取不到 git 身份 | 放行，`email`/`sig` 记 `-` | **拒绝**（无法归属的审批不予采信） |
  | 无 git 环境（精简容器）逃生口 | 设 `REQ_GUARD_REVIEWER_EMAIL`（姓名取 `REQ_GUARD_REVIEWER`）后同"身份一致" | 同"身份一致" |

- **自查**：`req-guard whoami` 打印当前仓库根、生效身份、`sig`、`auth` 等级与 AI 上下文判定，
  审批前可先确认"我会以谁的身份落账"。
- **诚实边界（§7 口径）**：同用户下的 AI 仍可执行 `git config user.name ...` 伪造身份，
  本项**抬高成本、不构成安全边界**。它与方案 D（TTY 在场证明）是**叠加**关系而非替代：
  D 保证"发起动作的是人类终端"，身份绑定保证"记录归属到某个 git 身份"，两者都靠不住时
  兜底仍是 §7 说的**事后复核**。

**规范（任选其一，强度递增）**：

| 方案 | 机制 | 强度 | 实现要点 |
|---|---|---|---|
| A. `REQ_GUARD_AI_CTX` 软标记 | AI 的 shell 会话自动带此变量，`approve/resolve/bypass` 检测到即拒 | 中（**白名单式**：未登记工具静默失效；AI 可 `env -u` 剥离） | 包装器/启动脚本注入；命令层读取并拒绝 |
| B. reviewer token | 签发短期令牌，原文由人类带外持有（库文件只存哈希） | 高（强度来自"凭据值只有人知道"） | 令牌生成/校验；`token issue` 本身须受锁保护 |
| C. 带外审批 | 人类在独立终端/Web 台签批，门禁只信任带人类签名的审批记录 | 最高 | 审批与 AI 执行上下文完全隔离 |
| D. 在场证明 | 终端一次性挑战码；非 TTY 通道一律拿不出 | 高（**fail-closed，不依赖工具登记**） | 台账记 `tty=`/`ai=` |
| E. 严格等级 `auth.level` | 把 A/B/D 的正交加固按 0–3 级组合，逐级收紧签发与审批 | 最高（L3，可配置） | `gate::auth_level`；`REQ_GUARD_AUTH_LEVEL` 只能抬高 |

**落地状态**：A/B/C/D 与 `auth.level`（0–3）均已实现；`init` 模板默认 **L3**，
存量项目只需写 `auth: level: <0-3>`（旧键 `strict: true` 等价 L1，不降级）。

> 纯字符串身份**不构成保证**。推荐 **B 或 D**（可叠加）作为生产保证；A 仅作开发期快速加固。
> 同用户隔离下**没有本地不可伪造的凭据**（guard.cfg 与 AI 同用户可写），
> 最终兜底仍是**事后复核**：审批须与人类在 PR / CI 侧的批准对应，见 §7 边界。

**校验清单（锁）**：
- [ ] AI 上下文内执行 `req-guard approve` → 被拒（路径 A 失效）
- [ ] AI 上下文内执行 `req-guard resolve` → 被拒（路径 B 失效）
- [ ] AI 直接写 `.gates/.bypass`（超长 `expires_epoch`）→ 被凭据/审计机制识别（路径 C 失效）
- [ ] **未登记的 AI 工具（无 `REQ_GUARD_AI_CTX`）在 L1+ 下执行 `approve` → 被拒**（路径 D，事故场景）
- [ ] AI 执行 `req-guard token issue`（非 TTY）→ 被拒（L1，自签失效）
- [ ] L2：`REQ_GUARD_TOKEN=… req-guard approve` → 被拒（环境变量通道已禁用）
- [ ] L3：同一张票据审第二次 → 被拒（`used=true`，抗重放）
- [ ] L3：票据 `scope` 与目标需求/步骤不符 → 被拒
- [ ] L3：用静态令牌审批 → 被拒（只认一次性票据）
- [ ] `REQ_GUARD_AUTH_LEVEL=0` 试图降低 L3 项目的等级 → 审批仍被拒（只能抬高）
- [ ] 真实人类在带外/带凭据或在 GUI 界面里审批 → 正常通过
- [ ] 台账 `tty=0` 的审批事件 → 复核时标红（AI 代跑特征）
- [ ] L1+ 下 `--reviewer 张三`（git 身份为寇工）→ 被拒（身份绑定，§4.4）
- [ ] L1+ 且未配 git 身份 → 审批被拒；设 `REQ_GUARD_REVIEWER_EMAIL` 后放行
- [ ] `req-guard whoami` 的 `sig` 与台账里同名审批的 `sig` 一致（可离线复算）
### 4.5 L1 注入器：未知工具默认拒绝（消除静默缺口）

**现状（已实现）**：提供 `req-guard install --verify`——核心资产缺失、在用工具配置
缺 `req-guard-check`、env 注入型工具缺 `REQ_GUARD_AI_CTX`、`.git` 存在但 pre-commit
缺拦截，任一命中即退出码 1；可直接作为 CI 步骤（见 4.1/4.3 校验清单）。
注入器对"已存在但不含 hook"的配置仍不覆盖（避免破坏既有配置），改为提示手工合并 +
`--verify` 在 CI 检出。

**规范**：
- 注入器在 `req-guard install` 后，提供 `req-guard install --verify` 或 CI 步骤，扫描所有已知工具配置是否含 `req-guard-check`；
- 任一在用工具缺 hook → **CI 报错**（而非本机提示）；
- 未知 AI 工具出现时，默认视为"未受管控"，CI 红并要求显式登记。

**校验清单**：
- [ ] CI 跑 `req-guard install --verify` → 全部在用工具配置含 hook 才绿
- [ ] 新增未知 AI 工具 → CI 红并提示登记

### 4.6 审计可信化

**现状（已实现）**：关键事件（approve / reject / resolve / bypass / 阻塞性评论新增）
同步写入**入库**的 `.gates/audit/ledger.md`（Markdown 台账，PR diff 可复核）；
`req-guard audit-digest` 对本机 `gate-audit.log` 计算 SHA-256，追加到入库的
`.gates/audit/DIGEST`，与本地日志比对即可发现事后篡改。

**规范**：
- 审计摘要或哈希定期提交到可审阅位置（版本控制 `audit/` 或中央日志）；
- PR 中可复核"谁在何时批了什么、是否带阻塞评论"。

**校验清单**：
- [ ] 关键审批/绕过事件在 PR 可见
- [ ] 审计摘要哈希与本地日志可比对

---

## 5. 部署总检查清单（上线前逐项勾选）

工具无关保证（核心，必须全绿）：
- [ ] **L2 真 fail-closed**：脚本缺失、或 `req-guard` 二进制不在 PATH 时 `git commit` 失败
- [ ] **L3 强制开启**：`enforce.ci:true` + 必需状态检查 + 分支保护
- [ ] **CI 用 `check --base <ref>`**：缺 `--base` 时多需求仓库恒拦（`install --verify` 会检出）
- [ ] **锁已上**：`approve/resolve/bypass` 在 AI 上下文被拒（方案 B/C）
- [ ] **审计入库**：PR 可审阅审批轨迹

多需求并行仓库（REQ-006，额外两项）：
- [ ] **每份未归档清单的 `GATE:TOUCH` 覆盖自己的改动**（否则该清单的改动落进歧义分支）
- [ ] **`multi.mode` 已复核**：默认 `resolve`；只有团队接受"都在开工前批完"才开 `all`

L1 体验层（做厚，但不计入保证）：
- [ ] 所有在用 AI 工具配置含 hook（CI `--verify` 全绿）
- [ ] 未知工具触发 CI 红
- [ ] 已向团队声明：L1 不覆盖 Shell 写文件 / 自调命令路径

---

## 6. 验收标准（如何证明"每个工具都守规矩"）

给出**可证伪**的验收方法，而非"看起来能拦"：

| 验收项 | 操作 | 期望 |
|---|---|---|
| 未批即写 | 任一 AI 工具尝试写源码（无论用 Write 还是 Bash `cat >`） | 提交流程被 L2/L3 拦截，无法合并 |
| 自批失效 | AI 上下文调 `req-guard approve` | 被锁拒绝（4.4） |
| 绕过失效 | AI 直写 `.bypass` 超长过期 | 被凭据/审计识别 |
| 跨工具一致 | 用 Claude / CodeBuddy / Codex / Cursor 各跑同一未批需求 | 行为一致，均被 L2/L3 拦 |
| 缺失即严 | 删脚本后 commit | L2 fail-closed 失败 |
| 审计可溯 | 走完一轮审批+绕过 | PR 可见完整轨迹；`PASS`/`BLOCK` 行**列出被裁决的需求 id**，不只记 PASS |
| **并行不漏拦** | 仓库并存 1 份已批 + 1 份未批清单，改动落在**未批**那份声明的范围内 | 提交被拦（抽取式裁决会在这里漏拦） |
| **并行不误锁** | 同上，改动落在**已批**那份声明的范围内 | 提交通过（抽取式裁决会在这里误锁） |
| **缺二进制从严** | 把 `req-guard` 从 PATH 移除后 `git commit` | 失败，且提示装回 `req-guard`（不放行） |

> 若某条验收不通过，说明对应层未达"保证"要求，需回到第 4 章对应小节补齐。

---

## 7. 残留风险与边界（诚实披露）

1. **L1 对 Shell 写文件天然覆盖不到**：这是 hook 机制的固有限制，靠 L2/L3 + 锁兜底，不靠 L1。
2. **锁的强度取决于凭据隔离**：方案 A（软标记）可被 `env -u` 绕过，仅作开发期加固；生产用 B/C。
3. **人类凭据本身泄露**则保证崩塌——属"人类侧"风险，需配合凭据轮换与最小授权。
4. **`done` 状态归档后不再拦截**：属预期行为（已交付需求），非绕过。
5. **hook 执行失败语义依赖 AI 工具**：若工具将 hook 非零视为"仅警告"，L1 可能 fail-open——故 L2/L3 必须独立存在。
6. **身份绑定（§4.4）不是密码学签名**：`sig` 是公开可复算的派生值，AI 可用
   `git config user.name` 伪造身份后自批。它解决的是"审批记录可归属"，不是"审批不可伪造"；
   后者仍依赖方案 B/D 的凭据隔离与 §6 的事后复核。
7. **多需求仓库的歧义分支会拦「纯文档提交」**：改动集剥掉 `touch.exempt` 后为空且仓库有多份
   未归档清单 → 报歧义并拦。这是 fail-closed 的诚实答案（无变更集就无从归因），但纯文档提交
   需要 `--req` 或先归档无关清单。**已知代价，不是缺陷**；代价见多需求门禁裁决方案文档。
8. **裁决按单文件粒度**：`Bash` 批量改写、格式化器全量重排不带 `file_path`，PreToolUse 判不到。
   这类改动由 L2/L3 的变更集兜底（`touch-check` 会因"未声明"而拦），但 L1 不覆盖。
9. **草稿通道（`.gates/drafts/` + `apply`）不经 GATE 块**：块内条目（如 `GATE:AC` 的验收标准）
   只能由人经 `amend` 改，草稿改不到 —— 这是刻意限制（否则草稿就成了绕过 AC 校验的通道），
   代价是"改一条验收标准"要人工粘贴。多段草稿会被 `apply` 拒绝（一次一段）。
10. **C 的签名级隔离受"零外部依赖"约束**：规范 C 的"最高保证"设想独立签名者（私钥带外、公钥验证），
   需非对称签名；而项目 `core` 坚持"纯 Rust 标准库"，标准库不含签名原语。当前 C 落为"流程化带外"
   （渠道声明 + 台账留痕 + `REQ_GUARD_OOB_ONLY` 强制），强度不高于 B。若团队接受为核心引入密码学依赖
   （如 ed25519），可将 C 升级为真签名审批。

---

## 8. 门禁工具自身怎么被开发（写给改 req-guard 的人）

前面七节约束的是「**清单怎么被校验**」。本节约束的是「**校验器本身怎么被改**」——
这个不对称是有代价的：清单侧的规则有人审（§4 的验收清单），而校验器侧的改动
一旦悄悄放松了判据，**没有任何信号会提示你**。

以下两条来自 REQ-008 那次实测教训，不是理论。

### 8.1 期望值必须能独立推出来，禁止「跑一遍看是多少就写多少」

**规则**：断言里的数字与字面量，必须能从**规格或结构**推出，不能从实现输出抄。
一旦期望值抄自实现，它就永远失去校验能力 —— 而且看不出区别：数字在那里，
看起来像规格。

**真实案例**（REQ-008）：修围栏边界时，我写了
`assert_eq!((6, 11), section_span(&doc, 1))`。人工推演：文档第 7 行是 `## 2.`，
修复后终点应是第 15 行的真标题，契约返回 `(7, 14)`。而 `(6, 11)` 在**修复前后都对不上**
（未修复时终点应是第 10 行 → `(7, 9)`）。那个数字是凭空写的，不是推出来的。

**怎么自查**：断言前先用结构推一遍（哪一行是标题、边界约定是开区间还是闭区间），
把推出来的值与实现返回值对照。**对不上时先怀疑期望值**，因为「实现不符测试」是
需要证据的结论，而「测试写错了」是默认假设。

**不变量优于具体值**：`assert!(s3 > e1, "…")` 这类相对断言比硬编码行号更能表达意图，
且不会因无关的格式变动而失效。但要配一句说明「为什么这条不变量就是需求」。

### 8.2 改完测试必须做判决性实验：把实现改坏，看测试是否失败

**规则**：修改测试断言后，必须验证新测试**确实能抓住它要抓的 bug**，然后才提交。
做法是把实现改坏（短路成空操作、注释掉关键分支），确认测试变红，再恢复。

**为什么这条是硬规矩**：「测试通过」与「测试有效」是两件**无法区分**的事。
一个断言方向写反的测试照样绿 —— REQ-008 里就有一个：它验的是「围栏内的 `## `
不成为段**起点**」，而围栏实际影响的是**终点**。改成正确方向前，那个测试一直是绿的，
却什么都没验。

**成本极低、收益极高**：REQ-008 的判决性实验是把 `mask_fenced` 短路成
`return text.to_string()`，跑一遍 —— 5 个单测 + verify_gate 场景 38/39 共 **7 处全挂**，
恢复后全绿。这一次实验就证明了测试有牙齿。**几十秒的实验，换掉「测试是装饰」的风险。**

**与「改实现迁就测试」的界限**：判据是**谁提供了独立证据**。
- 有独立证据（规格、推演、判决性实验）→ 可以改测试。
- 只有「实现这样跑出来的」→ **改实现去迁就测试**，或者把两边都停下来重新推演。

**本项目的已知不足**（诚实披露，尚未修）：
- `apply_校验失败不落盘` 那条用例改过三次前提，最后靠「草稿带一个畸形第二 AC 块」
  触发失败，与它真正想验的「声明侧校验失败时磁盘逐字不变」**未完全对齐**，应重写。
- 草稿段解析保留尾部空行（`{step=A}` 与 `{step=B}` 之间的空行算进 A 段），
  下游有 `trim_end_matches` 兜住所以无害，但更合理的是解析时 trim —— 让作者
  写出来的就是拿到的。这两条都应进下一轮需求。

---

## 9. 附录：各 AI 工具 hook 配置形态（参考）

注入点（`core/src/gate.rs::TOOL_PROFILES`），每工具注入**原生** schema：

| 工具 | 配置文件 | 事件 | matcher | 拦截命令 | 审批锁 env |
|---|---|---|---|---|---|
| claude | `.claude/settings.json` | `PreToolUse` | `Write\|Edit\|MultiEdit\|NotebookEdit` | `check`（exit 0/1） | ✅（`env` 段） |
| codebuddy | `.codebuddy/settings.json` | `PreToolUse` | `Write\|Edit\|MultiEdit\|NotebookEdit` | `check`（exit 0/1） | ✅（顶层 `env`） |
| codex | `.codex/hooks.json` | `PreToolUse` | `apply_patch` | `deny` 包装（exit 2） | ✗ |
| cursor | `.cursor/hooks.json` | `preToolUse` | `Write\|Edit\|MultiEdit\|NotebookEdit` | `deny` 包装（exit 2） | ✗ |

命令形态（Windows 用 ps1）：
- 直连：`sh .gates/hooks/req-guard-check.sh`（Claude/CodeBuddy）
- deny 包装：`sh .gates/hooks/req-guard-deny.sh`（Codex/Cursor，内部调 check 并把 exit 1 转成工具可识别的 exit 2；因这二者把非 exit 0 视为 fail-open）

matcher 区分大小写、匹配**工具名**；返回非零即阻断对应写路径。

> 新工具接入：在 `TOOL_PROFILES` 增加条目（含其原生事件名/matcher/拦截编码/是否支持 env）+ 提供该工具原生 schema 的注入片段 + 登记到 4.5 的 CI 校验白名单（`marker_of()` 决定该校验的 marker 子串）。

---

*本规范随 req-guard 演进维护；任何"保证"相关改动须同步更新第 2.2、4、6 章并回归验收清单。*

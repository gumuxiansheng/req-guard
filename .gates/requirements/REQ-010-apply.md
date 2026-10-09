---
doc_type: proposal
tier: critical
owner: -
review_policy: codebound
verified_at: 2026-10-05
source_refs: [core/src, cli/src, docs, .gates, packaging/docs, packaging]
---

# REQ-010 apply 草稿通道的契约硬化（多段草稿 / 位置配对 / 块内条目）

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-010 status=approved created=2026-10-05_00:51:32 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_09:37:48 sum=47b1c3f3931951f2bff04bab4d402d43733e95d9ba1bd2bc8ab8d93b6b27de1d -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-09_12:57:19 sum=e6a576193de00c77c85b1634af189f7352473b03567e1919253a3677fe28df29 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_01:32:48 sum=f93e61fd532fc36f64b84b01a1a00866687208b9f3c6beceacc2c4d628365a31 -->

## 1. 需求分解

### 背景与问题

REQ-007 的「草稿叠加区 + `apply` 一次命令」设计本身是对的（把「amend → AI 改 → approve」
的两次人工介入压成一次），但**草稿通道的契约只存在于实现里，没有写进任何文档，也没有校验**。
REQ-006 实施期实测踩了三次，其中两次是我的错、一次是设计缺口：

| # | 现象 | 触发方 | 后果 | 当前行为 |
| --- | --- | --- | --- | --- |
| **1** | 草稿含 `{step=solution}` + `{step=testplan}` 两段，连跑两次 `apply` | AI（我） | 第一次 `apply` 后草稿文件被删除，第二次报「未找到草稿」；**testplan 段的内容无声丢失** | `apply` 结尾无条件 `remove_file(draft_path)` |
| **2** | 草稿按「整段原文 + 插入」写（把 `GATE:AC` 块也照抄进草稿） | AI（我） | `replace_section` 的位置配对从第一处插入点起全部错位一行 → `GATE:AC` 开闭标记与条目被拆散 → `ac check` 报 50 条 `OutsideBlock`，`apply` 拒绝 | 靠 `dry_run_review` 兜住（磁盘逐字不变，**红线守住了**），但报错是 50 条噪音，看不出真因 |
| **3** | 想在段中间插入两行 | AI（我） | 做不到：多出的草稿行只能追加到**段末**，位置配对模型固有限制 | 静默追加，人不知道 |

三个现象里只有第 2 条被现有校验挡住，第 1、3 条是**静默**的。而「静默丢改动」与「静默改块」
在本项目里属于最坏失效那一类（`core/src/gate.rs` 的 `pretool_verdict` 文档注释称之为
「看着在拦、其实没拦」的同族：不是拦住你，而是让你以为它做了）。

另有两条同源缺口：

- **GATE 块集合变化无人把关**：`replace_section` 遇到原段的 GATE 块就原样保留，
  既不校验块集合是否与草稿预期一致，也不告警。AI 若把块标记写进草稿试图改块，
  结果是块被静默保留 —— 行为上是安全的，但**人拿不到任何反馈**，会以为改成功了。
- **「草稿 = 散文」这条契约无处可查**：`--help` 与 `.gates/README.md` 都没写，
  草稿区的注释只在 `core/src/requirement.rs` 的函数文档里。第三次实测证明它**不被发现**。

### 目标

- **G1** 多段草稿不再静默丢失：`apply` 一次只接受一段，多段即**拒绝**并列出草稿现有段名。
- **G2** GATE 块集合变化可感知：应用后块的 kind 集合与原文不一致时**报错**。
- **G3** 位置配对约束明示：新增行落在段末这件事**说出来**，不再靠人猜。
- **G4** 契约写进可查之处：`--help`、`.gates/README.md`、草稿区文件头注释三处一致。
- **G5** 块内条目（`GATE:AC` 等）**不经草稿通道**：契约写明，且给出人工路径（`amend`）。
- **G6** 草稿在校验与写盘之间被改时不被静默吞掉：写盘前重读逐字比对，不一致即拒（见 §2 关键设计 5）。

### 非目标

- **N1** 不改 `apply` 的**整体**流程与既有能力（一次完成修订、复用 `review_inner`、留痕双事件）——
  REQ-007 的核心价值不动。
- **N2** 不让草稿通道获得改 GATE 块的能力（那是绕过 AC 校验的路，见 G5）。
- **N3** 本期**不做**「段中间插入」的实现（见 P3 的取舍与代价）。
- **N4** 不改 `amend` / `approve` / `seal` 的语义。
- **N5** 不引入「草稿文件被工具改写」这种新状态（见 §2 关键设计 2 的取舍表）。

### 子任务拆解（编号 + 预估工时）

| 编号 | 内容 | 预估 |
| --- | --- | --- |
| **P0** | `apply` 拒绝多段草稿（G1）+ GATE 块集合校验（G2）+ 块集合/新增行落点的提示（G3） | 3h |
| **P1** | 契约落到三处文案（G4/G5）：`--help`、`.gates/README.md`、`write_draft` 的文件头注释 | 2h |
| **P2** | 单测：多段拒绝、块集合变化被拦、草稿行数差异的提示内容、既有 apply 路径不回归 | 3h |
| **P3**（**本期不做**） | 支持段中间插入（见 §2 关键设计 3 的代价评估，需人决定） | — |
| **P4** | 并发防护：写盘前重读草稿逐字比对 + 竞态单测（B-05 / G6；不引入锁文件，残留窗口见 §7） | 1h |


P0 必须先于 P1：文案若先于校验发布，等于**教用户使用一个会被静默丢内容的通道**。

### 影响范围

| 层 | 影响 |
| --- | --- |
| 模块 | `core/src/requirement.rs`（`apply` / `replace_section` / `parse_draft`）、`cli/src/main.rs`（apply 提示）、`cli/src/cli.rs`（help） |
| 命令 | `apply` 的**输入校验**收紧（多段草稿由「能用」变「拒绝」）；成功输出多一行提示 |
| 配置 | 无 |
| 草稿区 | `.gates/drafts/` 格式不变；文件头注释补充（由 `write_draft` 写出的夹具会带上） |
| 接口 / 数据表 / 外部 API | `Draft` / `parse_draft` / `replace_section` 的**签名不变**；`apply` 的错误文案变 |

### 验收标准

见第 3 段 `GATE:AC` 块（编号 + Given/When/Then，可机械判定）。

## 2. 技术方案

### 总体思路

**把隐式契约变成显式校验 + 显式文案。** 三个现象（第 1、2、3 条）里，两个是缺校验、一个是缺文案，
都不需要改 `apply` 的核心流程。改动集中在「应用前多问两句」与「把为什么这么写清楚」。

### 关键设计

#### 1. 多段草稿：拒绝，不写回

```
apply(root, id, step, reviewer, comment):
    draft = read_draft(root, id)?
    若 draft 含 >1 段 → Err("一次只应用一段…请按 <段名> 逐段应用；现有段：A、B")
```

**为什么不「应用后把剩余段写回草稿」**（我在 REQ-006 实施期评估过的原方案 A，评估后否掉）：

| |写回剩余段（原 A） | 拒绝（本方案） |
| --- | --- | --- |
| 改动面 | `apply` 增「构造剩余草稿并写盘」+ 单测 | `apply` 增一段输入校验（**纯只读**） |
| 新状态 | 草稿文件**被工具改写** → 「草稿不再等于人的原始意图」；而草稿区 `.gitignore` 忽略、不留版本，出错无法追溯 | 无 |
| 人的心智 | 「apply 会改我的草稿」 | 「一份草稿一段」 |
| 解决本次事故 | 是 | 是（**更早失败，且失败在写盘前**） |

多段草稿本来就不是设计意图（`parse_draft` 支持多段是为了「逐段独立：apply 可只应用其中一段」，
但从未承诺一次消费全部）。把**误用挡在写盘前**，比**为误用补一套写盘逻辑**便宜得多。

#### 2. GATE 块集合：应用后比对 kind 集合

`replace_section` 保留原段全部 GATE 块，故应用后块的 kind 集合**只可能等于原文**——
但这是实现的副产物，不是被检查的契约。补一条显式校验（写盘前，与 `dry_run_review` 同层）：

- 应用后块的 kind 集合 ≠ 原文的 kind 集合 → `Err`（实现变更导致的不一致，属 bug，须暴露）；
- 草稿散文里出现 `<!-- GATE:` / `<!-- /GATE:` 开头的行 → `Err` 并指出行号
  （这是本次事故第 2 条的直接成因：把块标记写进草稿，结果被静默忽略，人以为改了）。

**为什么不自动剥离草稿里的块再合并**：那等于默许「草稿可以写块」，而块是审批基准
（N2）。报错让人改草稿，比替他猜哪部分该保留要可靠。

#### 3. 位置配对：明示，不改实现

`replace_section` 是逐行位置配对：原段非 GATE 行 ↔ 草稿行；原段 GATE 块原样保留且不消耗草稿行；
草稿多出的行**追加到段末**。这带来两条硬约束，都必须写进文案：

| 约束 | 后果 |
| --- | --- |
| 草稿只提供**散文**，块由原清单保留 | `GATE:AC` 内的条目**不经草稿通道**，须人工经 `amend` 改（G5） |
| 多出的草稿行落到**段末** | 想在段中间插入做不到；这不是 bug，是位置配对模型的必然结果 |

**本期只做明示**（G3/P1），不做「段中间插入」。代价评估（供后续决策）：

| 方案 | 做法 | 代价 |
| --- | --- | --- |
| **维持 + 明示**（本期） | 位置配对不变，文案写清 | 新增行只能落段末；对「想在中间插一段」的人仍需人工分两次 apply |
| 整段替换 + 按 kind 复原块 | 剥掉草稿与原段的 GATE 块，用草稿散文**整段替换**，再把块按 kind 追加到段末 | 实现约 30 行；**块位置从段中间移到段末** → 渲染布局变化，影响既有清单的 diff 可读性；需为「块落在段末」补一批 AC |
| 支持插入锚点 | 草稿加 `{{after=<行锚点>}}` 之类标记 | 格式复杂度上升，且锚点与位置配对两套模型并存，最易出错 |

三案都要动既有行为，本期不做；本期把「为什么不能插中间」写清楚，避免第四次踩。

#### 4. 契约落到三处文案（G4/G5）

| 位置 | 内容 |
| --- | --- |
| `cli/src/cli.rs` 的 `help()` | `apply` 行补一句「草稿 = 散文；多段草稿会被拒绝」 |
| `.gates/README.md` | 新增「草稿区通道约束」小节：格式、只写散文、一次一段、块内条目走 `amend` |
| `write_draft` 的文件头注释 / 空草稿模板 | 若 `write_draft` 会写文件头，则在头里写同样三条 |

三处**逐字一致**：措辞分散正是本次「契约只存在于实现里」的成因之一。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
core/src/requirement.rs
core/src/lib.rs
cli/src/main.rs
cli/src/cli.rs
docs/README.md
.gates/README.md
packaging/docs/用户手册.md
packaging/docs/常见问题.md
packaging/CHANGELOG.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

| 维度 | 影响 |
| --- | --- |
| **兼容性** | 单段草稿的既有路径**逐字不变**；只有「多段草稿」与「草稿含块标记」两种输入由「能用/被静默忽略」变为「拒绝」。前者是修正静默丢失，后者是修正误用 |
| **性能** | 无影响（多两次 kind 集合比对，O(块数)） |
| **安全** | 只收紧不放宽：块仍是唯一由 req-guard 维护的机器标记，AI 经草稿改块从「静默无效」变为「明确报错」 |
| **草稿区不入库** | `.gitignore` 含 `/.gates/drafts`，故「工具改写草稿」无从追溯 —— 这也是 P1 拒绝写回方案的理由之一（§2 关键设计 1） |

### 风险点与回滚方案

| 风险 | 处置 |
| --- | --- |
| 有人已在用多段草稿（写回方案能照顾，改拒绝方案是行为收紧） | 该用法至今**静默丢内容**，收紧不产生「原本能用的功能被拿走」；且报错文案列出段名与逐段命令 |
| 「块集合校验」写成实现自证（恒成立，等于没检查） | 单测要构造一个**会失败**的用例（直接调 `replace_section` 传入含块标记的草稿），否则无法证明它在拦 |
| 文案与实现漂移（写了「一次一段」而代码没改） | 校验在 P0、文案在 P1，顺序硬约束；单测覆盖输入校验，`--help` 文案由用例断言含关键句 |
| **回滚方案** | P0/P1/P2 均不涉及 `apply` 的核心流程，可整体 revert；revert 后行为回到「多段静默丢、草稿含块静默忽略」，即本需求实施前的状态 |


#### 5. 草稿并发替换：写盘前重读并逐字比对（G6 / B-05）

`apply` 现在的顺序是「读草稿 → 内存里拼 `proposed` → `dry_run_review(proposed)` → 写清单」。
**草稿文件本身在这几步之间被换掉**（另一个进程 / 另一个人改了草稿，或编辑器保存）时，
落盘的是**基于旧草稿**的内容，而人以为应用的是新草稿 —— 这正是本需求要消灭的那类静默。

```
apply(root, id, step, reviewer, comment):
    raw0 = read_draft_raw(root, id)          ← 记下原始字节
    draft = parse_draft(raw0)
    …（P0 的输入校验 + dry_run_review）…
    raw1 = read_draft_raw(root, id)          ← 写盘前重读
    若 raw1 != raw0 → Err("草稿在校验期间被修改（<旧字节数> → <新字节数> 字节），未落盘；请重跑 apply")
    write 清单
```

**为什么逐字比而不是 mtime / size**：mtime 会被 `touch`、`git checkout`、编辑器保存改；
size 会被「改了一个字又改回来」骗过。草稿长度是几百字节量级，逐字比的代价可忽略。

**残留的 TOCTOU 窗口**：重读与写盘之间草稿仍可能变。本期**明确接受**这个窗口 ——
彻底消除需要锁文件（POSIX `flock` / Windows `LockFileEx` 跨平台两套实现），
与「草稿不入库、单人单次 apply」的现实协作约定相比收益不足。写进 §7 残留风险。

## 3. 测试计划

### 单元测试用例（编号 + 断言点）

- **U-01** `apply` 在草稿含 2 段时返回 `GateError::Validation`，message 含两个段名与
  「一次只应用一段」；且**清单文件与草稿文件都逐字不变**
- **U-02** `apply` 在草稿含 2 段时，台账与审计日志**不新增**任何事件（拒绝不得留痕成"已修订"）
- **U-03** `replace_section` 传入含 `<!-- GATE:TOUCH -->` 的草稿正文时，
  应用前校验拦下且指出该行行号
- **U-04** 应用后块的 kind 集合与原文不一致时报错（构造用例证明该校验**会失败**，
  不是恒成立的自证）
- **U-05** 单段草稿的既有路径逐字不变：应用后散文 = 草稿散文、块 = 原文块、状态 = approved、
  新摘要已绑定
- **U-06** 草稿比原段短时，**不得静默丢原段尾部散文**：要么报错，要么逐行覆盖后仍等于草稿长度
  （按 P0 选定的行为断言，不留第三种可能）
- **U-07** `help()` 含「草稿」「散文」「一次」三处关键句；`.gates/README.md` 含同样三处
- **U-08** 草稿为空文件 / 只有标记行 → 仍报「该段没有实质正文」（既有行为不回归）
- **U-09** `apply` 的写盘前重读：把草稿在校验与写盘之间改掉（单测里用「校验后改文件」模拟）
  → 返回 `GateError::Validation`，message 含两个字节数，且**清单文件逐字不变**（G6）
- **U-10** 同一份草稿内容不变时（哪怕 mtime 被 `touch` 改过）→ 正常落盘（比对按内容，不按 mtime）

### 端到端用例（编号 + 执行步骤）

- **E-01** 两段草稿 → 第一次 `apply` 成功 → 第二次 `apply` 报「一次只应用一段」并列出段名；
  第一个段确实已更新（草稿丢失的是**未应用的段**，且本次要求人显式重写）
- **E-02** 单段草稿 → 一次 `apply` 成功 → 重跑 `apply` 报「未找到草稿」（既有行为不回归）
- **E-03** 草稿含块标记 → `apply` 拒绝且清单逐字不变；改对草稿后重跑成功
- **E-04** 草稿就绪后先改草稿再 `apply` → 应用的是**改后**的内容；刻意在两个命令之间改草稿
  → 报错且清单逐字不变（G6 的真机版：不用单测里的模拟）

### 边界 / 异常 / 并发场景

- **B-01** 草稿里同一段出现两次（`{step=testplan}` 写两遍）→ 拒绝并指出重复
- **B-02** 草稿段落名非法（`{step=foo}`）→ 沿用 `parse_draft` 的 `validate_step` 报错，不改
- **B-03** 草稿段名为空（`{step=}`）→ 拒绝
- **B-04** 草稿含 CRLF / 末尾无换行 → 与既有行为一致，不因本需求改变
- **B-05** 并发替换草稿（**G6 已覆盖**）：校验与写盘之间草稿被改 → 写盘前重读逐字比对，
  不一致即报错。残留的「重读→写盘」TOCTOU 窗口见 §2 关键设计 5 与 §7 残留风险
- **B-06** 草稿在写盘瞬间被换掉（TOCTOU）：本期**明确接受**该窗口，不引入锁文件

### 回归范围与影响面

- 全量：`cargo test --workspace --exclude req-guard-gui`、
  `cargo clippy --workspace --exclude req-guard-gui --all-targets -- -D warnings`、
  `cargo fmt --all --check`、`python3 scripts/verify_gate.py`
- 受影响：`core/src/requirement.rs`（`apply` / `replace_section` / 草稿原始字节读取）、
  `cli/src/main.rs`、`cli/src/cli.rs`
- 不受影响：`HOOK_SH` / `HOOK_PS1`、门禁裁决（`core/src/resolve.rs`）、`touch` 口径、
  CI 模板、`core/Cargo.toml`（零依赖）
- 需同步镜像：无（纯本地 CLI 行为）

### 验收门槛（全部可机械判定）

- **拒绝优先于放行**：三条新增校验各自都要有"会失败"的单测（U-01 / U-03 / U-04），
  只有通过路径的用例等于没测
- **不回归**：U-05 / E-02 锁住单段草稿路径逐字不变
- **并发防护可测**：U-09 / E-04 必须真的构造出"草稿被改"的情形；只断言"报错文案存在"等于没测
- **窗口被披露**：TOCTOU 残留（B-06）必须在 §7 残留风险里明写，不得只留在设计文档
- **文案可查**：U-07 断言三处关键句同时存在
- 用户输入路径零新增 `unwrap()`；失败一律走 `GateError::Validation`
<!-- GATE:AC -->
### AC-001
- Given: 某清单三段已 approved，`.gates/drafts/<需求ID>.draft.md` 含 `{step=solution}` 与 `{step=testplan}` 两段且各自有实质正文
- When: 执行 `req-guard apply <需求ID> --step solution`
- Then: 退出码非 0，message 含 2 个段名与「一次只应用一段」，且清单文件与草稿文件均逐字不变

### AC-002
- Given: 某清单三段已 approved，草稿含两段且 `--step testplan`
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码非 0，且 `.gates/audit/ledger.md` 的行数与执行前相同（拒绝不得留痕为已修订）

### AC-003
- Given: 某清单三段已 approved，草稿的 `{step=testplan}` 正文里含一行 `<!-- GATE:AC -->`
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码非 0，message 含 `GATE:AC` 与该行的 1-based 行号，且清单文件逐字不变

### AC-004
- Given: 某清单三段已 approved，草稿的 `{step=testplan}` 正文里不含任何 GATE 块标记
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码 0，应用后该段 GATE 块的 kind 集合与原文完全相同，且 `req-guard verify-content <需求ID>` 退出码 0

### AC-005
- Given: 某清单三段已 approved，其第 3 段散文 3 行，草稿的 `{step=testplan}` 正文只有 1 行
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 应用后该段散文行数为 1，或退出码非 0 并报行数差异；不得静默保留第 2 至第 3 行

### AC-006
- Given: 某清单三段已 approved，草稿的 `{step=testplan}` 正文比原段散文多 2 行
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 成功信息里明确写出新增 2 行落在段末，且 `req-guard ac check <需求ID>` 退出码 0

### AC-007
- Given: 某清单三段已 approved，草稿的 `{step=solution}` 正文含一行 `core/src/requirement.rs`
- When: 执行 `req-guard apply <需求ID> --step solution`
- Then: 退出码 0，且 frontmatter 的 `source_refs` 至少含 `core` 一项（派生仍生效）

### AC-008
- Given: 某清单三段已 approved，草稿的 `{step=testplan}` 正文为空文件
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码非 0 且 message 含「没有实质正文」，且清单文件逐字不变

### AC-009
- Given: 某清单三段已 approved，草稿的 `{step=testplan}` 正文第 1 行是 `{step=testplan}` 的重复标记
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码非 0 且 message 指出重复的段名，不静默采用最后一份

### AC-010
- Given: 某清单三段已 approved，草稿含 `{step=foo}` 非法段名
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码非 0 且 message 含 `foo` 与合法段名列表（decomposition / solution / testplan）

### AC-011
- Given: 某清单三段已 approved，其第 3 段散文末行为一段说明文字
- When: 草稿的 `{step=testplan}` 正文比该段散文少 1 行并执行 apply
- Then: 应用后该段以草稿末行结尾，或退出码非 0 并指出少 1 行；末行不得被原段内容覆盖

### AC-012
- Given: 已构建的二进制与本仓 `.gates/README.md`
- When: 检索 `req-guard --help` 中 `apply` 行的说明与 `.gates/README.md` 的草稿区小节
- Then: 两处均含「草稿」「散文」「一次」三个关键词，且 `.gates/README.md` 含「amend」

### AC-013
- Given: 某清单三段已 approved，且 `req-guard --help` 与 `.gates/README.md` 均已含草稿契约三句
- When: 手工把 `core/src/requirement.rs` 的多段校验注掉后执行 `python3 scripts/verify_gate.py`
- Then: 脚本退出码非 0 且报出缺失的断言（文案与校验的漂移必须被机械检出）

### AC-014
- Given: 某清单三段已 approved，草稿含两段且 `--step testplan`
- When: 先对清单文件与草稿文件各取一次 SHA-256，执行 apply 失败后再取一次
- Then: 两处哈希字符串均与执行前逐字相同，即各 2 个 SHA-256 全部一致（拒绝路径零副作用）

### AC-015
- Given: 某清单三段已 approved，草稿合法且 `apply` 的各项校验均已通过
- When: 在校验完成与写盘之间把草稿文件内容改掉，再继续执行 apply
- Then: 退出码非 0，message 含「草稿在校验期间被修改」与两个字节数，且清单文件逐字不变

### AC-016
- Given: 某清单三段已 approved，草稿内容合法但其 mtime 已被 `touch` 命令改过
- When: 执行 `req-guard apply <需求ID> --step testplan`
- Then: 退出码 0 —— 比对按内容而非 mtime，mtime 变化不得触发拦截

### AC-017
- Given: 某清单三段已 approved，草稿合法
- When: 先 `cat` 草稿内容另存副本，改完草稿后执行 apply，再把副本与落盘后的段落逐字 diff
- Then: 退出码 0，且该段 diff 输出 0 行差异 —— 落盘内容等于改后的草稿（真机路径）
<!-- /GATE:AC -->
- `cargo clippy --workspace --exclude req-guard-gui --all-targets -- -D warnings` 零告警；
  `cargo fmt --all --check` 通过

### 验收标准

规则见 `docs/设计/AC与变更范围契约技术方案.md` §2；本块条目即 P0–P2 的验收基准。
编号自第一条起连续，形态唯一为三行式，Given 自包含，Then 可度量。

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-05_00:59:15 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-05_00:59:55 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-05_01:00:04 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
- 2026-10-05_01:30:58 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | B-05 纳入本期：G6 + P4 + 关键设计 5
- 2026-10-05_01:32:48 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | 补 U-09/U-10、E-04、B-05/B-06
- 2026-10-05_09:32:51 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | amended | 补记账缺口：B-05 并发替换已并入本期，缺 G6 目标行与 P4 子任务行
- 2026-10-05_09:37:48 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | 补 G6 / P4，与 solution / testplan 的已批内容一致
- 2026-10-09_12:57:19 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | 档位升至 critical（§8.5 方案 2）
<!-- /GATE:AUDIT -->

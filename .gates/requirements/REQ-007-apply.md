---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-04
source_refs: [core/src, cli/src, scripts, .gitignore, docs/规范, README.md]
---

# REQ-007 草稿叠加区与一次命令修订（apply）

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-007 status=approved created=2026-10-04_19:29:22 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_19:35:41 sum=005343cddaec25299f45aac474450d7080787b481d263e27486c1832407191d8 -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_19:35:58 sum=d0be2c8c7d06caa7dc6b6f79aa19e5869d0ee44a3d34274db05d27aee6bd21a5 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_19:36:11 sum=da789ce3f223b1aa4066c9112dee77271f3df318cfcafbb4afb59135ed6b1ffe -->

## 1. 需求分解

### 背景与问题

实测（REQ-004 / REQ-005 两条需求）下来，实施期修改清单的**往返成本**偏高：
每改一段要人工跑两条命令 —— `amend`（清摘要、打回待审）→ AI 改 → `approve`
（写新摘要）。一次修订 = **2 次人工介入 + 1 次等待 AI**。

REQ-004 的 `amend` 把「方向没错、只是要改」与「这段被否决」在语义上分开了，
但**没有减少往返次数**。本需求要解决的就是这个摩擦。

先说清一个容易被误解的点：**「一次命令完成 amend + approve」不能独立成立。**
两者之间必然夹着「编辑」，而：

- AI 改不了已冻结的段（`doc_write_guard` 会拦，`core/src/gate.rs` 的
  `frozen_section_violation`）；
- 人必须先 `amend` 清掉摘要，AI 才能改。

所以「一次命令」只能是「**apply 一次把 amend + 落草稿 + approve 全做完**」，
而这要求 AI 的改动**先写到一个不参与任何判定的地方**。
**草稿叠加区不是附加功能，它是「一次命令」的实现前提。** 两者一起做才对。

### 目标

- **G1** 草稿叠加区：AI 把拟改内容写进**不参与判定**的区域（不冻结、不计入实质
  正文、不影响摘要、不被 `ac check` 读取），已批准正文字节不动。
- **G2** `req-guard apply <需求ID> [--step <步骤>] [--draft <文件>]`：一条命令完成
  「读草稿 → 校验 → 写正文 → 重新 `approve` → 写新摘要」，人工只介入一次。
- **G3** apply **不豁免任何审批守卫**：仍走 `ensure_human`、AI 上下文直拒、
  L3 票据绑定到具体 (需求, 步骤)；审批记录与台账**照常留痕**。
- **G4** 草稿被拒时给出**可定位的差异**（哪一段、哪些行、为什么），而不是
  「校验失败」四个字。

### 非目标

- **N1** 不引入「AI 自批」。`apply` 由人执行，AI 仍只能写草稿。这是本需求的红线：
  一旦 AI 能自己把草稿变成 approved，REQ-001 的审批锁与 REQ-002 的内容冻结
  同时失去意义 —— 三条需求建立的信任会被一个 `--auto` 抹掉。
- **N2** 不做「按改动大小分级」（微调免审）。分级不可机械判定，等于留一个由
  AI 说了算的口子；这与 REQ-004 的 N1 同源，不推翻。
- **N3** 不动 `GATE:TOUCH` 的语义。变更范围仍由声明 + `touch --declare` 管，
  apply 不绕过 `ensure_touch_declared`。
- **N4** 不给草稿区提供「部分生效」：要么整段应用，要么不动。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | 草稿区格式与解析：`.gates/drafts/<需求ID>.draft.md`，按段分节、逐段独立 | 3h |
| T2 | 草稿区不参与判定的四处豁免：写入守卫、实质正文统计、`ac check`、`verify-content` | 3h |
| T3 | `apply` 命令：校验 → 写正文 → `approve`（复用 `review_inner`，不留两套判定） | 3h |
| T4 | 差异输出：apply 前打印逐段 diff（可用则高亮），被拒时定位到行 | 2h |
| T5 | 审批守卫与留痕：AI 直拒、L3 票据、`AMEND`+`APPROVE` 双事件 | 2h |
| T6 | CLI 接线、单测、`verify_gate.py` 场景、规范与 README | 3h |

合计 16h。T1+T2+T3 是最小可交付（草稿能写、apply 能一次完成修订）。

### 影响范围

- **模块**：`core/src/requirement.rs`（草稿解析与 apply）、`core/src/gate.rs`
  （写入守卫放行草稿区）、`core/src/ac.rs` 与 `core/src/section.rs`（草稿区不计入）、
  `core/src/touch.rs`（草稿区不参与）、`cli/src/cli.rs`、`cli/src/main.rs`、
  `scripts/verify_gate.py`、`.gitignore`（草稿区为本机态）、文档。
- **配置**：新增草稿目录 `.gates/drafts/`（不入库）。
- **清单格式**：清单本身**零变更** —— 这是本设计的核心约束（G1）。
- **接口 / 数据表 / 外部 API**：新增 `req-guard apply` 子命令。
- **向后兼容**：既有命令行为不变；不 apply 草稿时，系统与今天完全一致。

### 本次事故暴露的相邻缺陷（不在本需求范围）

草稿区若长期堆积会掩盖「真正待审的修订」—— 需要一个「草稿年龄」提示或
`status` 里的草稿计数（G4 只解决单次可定位性）。留待后续。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量）。

## 2. 技术方案

### 总体思路

**草稿写在门禁看不见的地方，人用一条命令把它变成正式内容。**

草稿区（`.gates/drafts/<需求ID>.draft.md`）对所有判定不可见：写入守卫不拦、
实质正文不统计、`ac check` 不读、摘要不覆盖。于是 AI 可以自由迭代措辞，
而**已批准的正文一个字节都没变** —— 内容冻结（REQ-002）在草稿阶段完全不生效，
因为草稿根本不是「清单内容」。

`apply` 是唯一入口，且只做三件事：校验草稿、把整段替换进正文、调用既有的
`review_inner(pass=true)` 写新摘要。**不新建任何判定逻辑** —— 审批门禁、
身份绑定、来源派生、AC 合规全部复用现成路径，这是本需求能保持可信的根本原因。

### 关键设计

1. **草稿区位置与形态**：`.gates/drafts/<需求ID>.draft.md`，以**行内标记**声明段名
   （`{step=<段名>}`），段名取 `decomposition` / `solution` / `testplan`，
   标记之后到下一个标记或文末之间的全部内容，就是该段的拟替换正文。

   逐段独立：apply 可只应用其中一段。未提及的段不动。空文件或缺失文件 =
   「没有草稿」，apply 明确报「未找到草稿」而不是静默成功。

   **为什么不用 `## <段名>` 作分隔**：段边界判定 `section_span` 的 `is_heading`
   判的是 `trim_start().starts_with("## ")` —— **缩进的 `## solution` 同样命中**。
   于是「文档里展示草稿格式的示例」会把自己那份清单的段落边界切错，
   表现为 `GATE:TOUCH` 块突然「消失」（实测起草本需求时踩到）。
   行内标记不受行首缩进影响，且不与 Markdown 标题语法撞车。

2. **四处不参与判定**（T2 的全部内容，缺一不可）：
   - `doc_write_guard`：草稿区路径不在清单路径集合内，直接放行
   - `section::is_section_empty` 的实质正文统计：草稿区根本不参与
   - `ac::check`：只读清单文件的三个段
   - `requirement::verify_sums` / `verify_content`：同理

   之所以要显式列出这四处而不是「反正不在清单里」：这四条路径里只要有一条
   将来 broadened 到整个 `.gates/`，草稿就会突然开始被判定 —— 而**失效方向
   是「草稿被误判为正式内容」**，表现为莫名其妙的报错，没人查得出原因。

3. **apply 复用 `review_inner`**（而非另写一套）：apply 的最后一步就是
   `review(pass=true)`。这样 REQ-001 挂在 approve 上的 AC 合规、REQ-002 的摘要
   写入、身份绑定、`consume_credential_if_scoped` **自动全部生效**。
   若 apply 自己实现「写摘要」，就会立刻出现「apply 路径绕过 AC 合规检查」
   这类最难发现的漏洞 —— 本项目已经吃过一次（落盘钩子是旧版那次）。

4. **先校验后落盘**（G4）：apply 必须**先**把草稿送进全部校验（AC 合规、
   `GATE:TOUCH` 有效性、段定位、交叉引用），**全部通过**才写文件。
   任一失败则**不落盘**，并输出逐段 diff + 定位到行 —— 半落盘会留下
   「正文改了但状态没改」的中间态，那比直接失败难收拾得多。

5. **审批守卫与留痕**（G3 / T5）：apply 走 `ensure_human("apply", …)` +
   `ScopeCheck::Exact("<id>:<step>")`，AI 上下文直拒。台账写**两条**事件
   （`AMEND` + `APPROVE`）而不是一条 `APPLY`：单条事件会让「返工率」指标
   （REQ-004 的 `amend_counts`）漏计这次修订，而它**恰恰是一次真实返工**。

6. **草稿区不入库**（进 `.gitignore`）：草稿是工作态，且可能被包含尚未成形的
   考虑。入库会让 `git status` 噪声化，并可能误提交半成品表述。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
core/src/requirement.rs
core/src/gate.rs
core/src/ac.rs
core/src/section.rs
core/src/touch.rs
cli/src/cli.rs
cli/src/main.rs
scripts/verify_gate.py
.gitignore
docs/规范/AI工具合规保证规范.md
README.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：清单格式零变更；既有命令行为不变。不写草稿、不调 apply 时，
  系统与今天完全一致（这是可回滚性的保证）。
- **性能**：草稿按需读取，不进任何热点路径；apply 与一次 approve 同量级。
- **安全**：草稿区对判定不可见，**但也不是安全区** —— 草稿内容绝不会被当作
  已批准内容引用；apply 是唯一入口且需人类在场。apply 的鉴权强度与 `approve`
  完全一致（同一个 `ensure_human` 入口）。

### 风险点与回滚方案

- **风险 1（高）**：某条判定路径将来 broadened 到草稿区 → 草稿被误判。
  缓解：T2 显式列出四处豁免并各配一个用例；加一条「草稿区内容永不进入判定」
  的回归测试（AC-007）。
- **风险 2（中）**：apply 部分落盘（正文改了、状态没改）→ 中间态难收拾。
  缓解：关键设计 4「先校验后落盘」，任一校验失败即中止（G5 用例锁死）。
- **风险 3（中）**：apply 被误用为「绕过 `touch --declare` 扩张声明范围」。
  缓解：apply 复用 `ensure_touch_declared`，声明为空时**同样拒绝**（AC-010）。
- **回滚**：草稿区与 apply 是纯增量，删掉 CLI 分支与草稿解析即可完全回退；
  清单格式没变，所以不存在需要迁移的数据。

## 3. 测试计划

### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | 草稿按段解析 | 两段草稿解析出两个独立段 |
| U2 | 草稿只提一段时 apply 只动那段 | 另一段 `sum=` 逐字不变 |
| U3 | 草稿区写入不被守卫拦 | `doc_write_guard` 返回 Ok |
| U4 | 草稿区不计入实质正文 | 段仍被判为空（草稿不影响判定） |
| U5 | 空/缺草稿明确报错 | apply 报「未找到草稿」，非静默成功 |
| U6 | 校验失败不落盘 | 磁盘内容与 apply 前逐字相同 |
| U7 | apply 写新摘要 | 新 `sum=` 与应用后正文一致 |
| U8 | apply 在 AI 上下文被拒 | 消息含 `REQ_GUARD_AI_CTX` |
| U9 | 台账两条事件 | 同时含 `AMEND` 与 `APPROVE` |
| U10 | 返工率计入 apply | `amend_counts` 该段 +1 |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 已批准清单 → AI 写草稿 → `apply --step solution --comment ...` | 一次命令完成；`verify-content` 通过 |
| E2 | 草稿令 AC 不合规 → `apply --step testplan` | 被拒且**磁盘未变**，报错含 A 规则名 |
| E3 | 草稿清空 `GATE:TOUCH` → `apply --step solution` | 被拒（声明为空） |
| E4 | 草稿含失效交叉引用 → `apply --step solution` | 被拒，报 `BrokenCrossRef` 与行号 |
| E5 | 同一草稿连续 apply 两次 | 第二次幂等：摘要与正文仍一致 |

### 边界 / 异常场景

- **B1** 草稿段标题拼错（如 `## solutions`）→ 报「未知段名」并列出合法值。
- **B2** 草稿引用了清单里不存在的段号 → 拒绝，不静默忽略。
- **B3** 草稿文件编码非 UTF-8 / 含 BOM → 明确报错，不产出半成品。
- **B4** 对 `done` 清单 apply → 拒绝（归档是终点）。
- **B5** 草稿段落为空（只有标题无内容）→ 拒绝写入，防「apply 即清空该段」。
- **B6** 并发：草稿在 apply 过程中被改 → 以 apply 启动时的内容为准（快照）。

### 回归范围与影响面

- `core` 全量单元测试；`verify_gate.py` 全部场景。
- REQ-001~005 的 `check` / `ac check` / `touch-check` / `verify-content` / `seal`
  结论**一律不变**（清单格式零变更是本需求的硬约束）。
- 门禁脚本资产的阶段序号不变（草稿区不引入新阶段）。

### 验收门槛

- `cargo fmt --all --check` 与 scoped `clippy -- -D warnings` 零输出。
- `cargo test -p req-guard-core -p req-guard` 全绿。
- `python3 scripts/verify_gate.py` 全部场景通过（新增场景随实现补齐）。
- core 依赖清单不新增第三方依赖。

<!-- GATE:AC -->
### AC-001
- Given: 一份三段均已批准且 `sum=` 已绑定的清单
- When: 在 `.gates/drafts/REQ-001.draft.md` 写入 `## solution` 段草稿
- Then: `req-guard verify-content` 退出码为 0，清单 `sum=` 三段均未变化

### AC-002
- Given: 上述草稿文件已存在
- When: 对清单正文路径执行一次写入拦截判定
- Then: 判定返回 Ok（1 处放行、0 处拦截），草稿文件写入后存在且行数大于 1

### AC-003
- Given: 清单第 1 段实质正文为 0 行（`EmptySection` 会拦批准）
- When: 在草稿区写入该段的拟改内容
- Then: `req-guard ac check <需求ID>` 的 `EmptySection` 问题数仍为 1，草稿不改变判定

### AC-004
- Given: 一份已批准清单与一份含 `## solution` 段的草稿
- When: 执行 `req-guard apply <需求ID> --step solution --comment "修订说明"`
- Then: 退出码为 0，且人工命令总次数为 1（无需先单独执行 amend）

### AC-005
- Given: 上述 apply 已完成
- When: 执行 `req-guard verify-content <需求ID>`
- Then: 退出码为 0，三段 `sum=` 均与当前正文一致

### AC-006
- Given: 一份草稿使第 3 段 `GATE:AC` 块不满足编号连续性
- When: 执行 `req-guard apply <需求ID> --step testplan --comment "x"`
- Then: 退出码为 1，磁盘上清单内容与 apply 前逐字相同，报错含 A 规则名

### AC-007
- Given: 草稿区存在任意内容
- When: 对全部判定入口（`check` / `ac check` / `touch-check` / `verify-content`）逐项统计
- Then: 因草稿内容导致的错误或告警数为 0

### AC-008
- Given: 一份含 `## solution` 段的草稿与一份三段已批准的清单
- When: 执行 `apply` 后统计台账中该需求的事件
- Then: `AMEND` 事件数与 `APPROVE` 事件数各为 1，且返工计数为 1

### AC-009
- Given: 环境变量 `REQ_GUARD_AI_CTX` 非空的终端
- When: 执行 `req-guard apply <需求ID> --step solution --comment "x"`
- Then: 退出码为 1，消息含 `REQ_GUARD_AI_CTX`，且清单 `sum=` 保持原值

### AC-010
- Given: 一份草稿把第 2 段的 `GATE:TOUCH` 声明清空
- When: 执行 `req-guard apply <需求ID> --step solution --comment "x"`
- Then: 退出码为 1，磁盘未变，报错指明声明为空

### AC-011
- Given: `.gates/drafts/` 目录下不存在任何草稿文件
- When: 执行 `req-guard apply <需求ID> --step solution --comment "x"`
- Then: 退出码为 1 且消息含「未找到草稿」，退出码不为 0

### AC-012
- Given: 一份草稿只包含 `## solution` 一个段
- When: 执行 `req-guard apply <需求ID> --step solution --comment "x"`
- Then: 第 1 段与第 3 段的 `GATE:STEP` 行 `sum=` 与 apply 前逐字相同
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-04_19:35:41 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-04_19:35:58 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-04_19:36:11 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

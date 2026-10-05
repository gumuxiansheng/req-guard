---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-05
source_refs: [tui/src, gui/src, docs/设计]
---

# REQ-014 界面门禁检查消歧接线：修复多需求仓库下 GUI/TUI 检查恒报 Ambiguous

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-014 status=approved created=2026-10-05_13:22:36 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:19:49 sum=202d58b76a118abf1a3683ac2e834a8229f82501fc108cb695a04d990568cd77 -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:20:01 sum=cdde6ce73e1d8dc4f3a86ce7538d94c5249a8ce265329a708a74c35daf69aa86 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:20:09 sum=2763ab2ab9acc55ddbcebf3cbb02f77797cd354fa56eb664f0c67ea37e37577f -->

## 1. 需求分解

### 背景与问题

GUI 与 TUI 的「执行门禁检查」按钮调用的是 **裸 `gate::gate_check`**，
既不带变更集、也不带需求消歧提示：

- `tui/src/app.rs:707`：`match gate::gate_check(&self.root)`
- `gui/src/app.rs:422`：`match gate::gate_check(&self.root)`

而 `gate::gate_check`（`core/src/gate.rs:713`）的实现是固定传
`PathSource::None` + `hint: None`：

```rust
pub fn gate_check(root: &Path) -> Result<GateVerdict> {
    gate_check_with(
        root,
        &crate::resolve::Ctx {
            source: crate::resolve::PathSource::None,
            hint: None,
        },
    )
}
```

`core/src/gate.rs:707-712` 自己已经把这个后果写清楚了（原文照录）：

> 全局门禁判定（**无变更集**）—— TUI / GUI 的状态面板与「本次改动合规吗」之外的
> 存量用法走这里。… 多需求仓库下它会命中 [`crate::resolve::BlockKind::Ambiguous`]
> （无变更集就无从归因，这是诚实答案而不是缺陷）。**界面上要把「正在做的那份需求」
> 纳入判定，应改用 [`gate_check_with`] 并把该 id 作为 hint —— 属 P4 的 UI 接线。**

这段注释同时给出了两个判断，二者都成立：

1. **「诚实答案而不是缺陷」说的是 `gate_check` 这个函数本身**——无变更集就无从归因，
   判 `Ambiguous` 是对的；
2. **但界面有 `hint` 可给**——界面上明明有一个「当前选中需求」，评审人点检查按钮时
   心里想的必然是「我这份能不能开工」，而这个信息被界面丢掉了。**丢信息才是缺陷。**

实测范围：本仓库当前 `.gates/requirements/` 下有 14 份未归档清单，
默认 `multi.mode = resolve`（`core/src/resolve.rs:55`）。在 `judge` 里，
`PathSource::None` 使变更集为空，候选集只能来自 `hint`；`hint` 为 `None` 时
`resolve.rs:520-530` 的兜底是「`hint` 为空**且** live 恰好 1 份」才返回那份，
否则返回空集 —— 空候选集在多需求下即 `Ambiguous`。
**即：只要仓库里有 ≥2 份未归档清单，GUI/TUI 的检查按钮在多需求模式下恒定显示拦截。**

这不是体验瑕疵，是**功能不可用**：界面把「我这份需求能不能开工」这个唯一的问题
恒定回答成「不能」，且给出的理由（歧义）与用户实际操作（正看着某一份清单）矛盾。
审核人此时唯一能做的就是忽略这个按钮—— 一个恒定误报的按钮比没有按钮更糟，
因为它训练用户忽略告警。

现状是 P4「合一」阶段标 ✅ 完成（`docs/设计/UI架构细化方案.md:337`），
但 P4 的验收标准是「探测决策表 + `full` feature + GUI→TUI 回退」，
**不含**逐个界面的动作接线。这条接线属于遗留项，不是回归。

### 目标

- **G1 界面检查带上当前需求作 hint**：GUI/TUI 的检查动作改调 `gate_check_with`，
  `Ctx.hint = Some(当前选中需求 id)`。用户在清单视图里点检查，判的就是这份清单。
- **G2 未选中任何需求时不猜**：当前无选中项时 `hint = None`，
  行为与现状**逐字一致**（仍走 `PathSource::None`，多需求下仍报 `Ambiguous`）——
  修的是「有信息却不用」，不是「把歧义判成通过」。
- **G3 变更集来源显式且可辨**：界面上检查的语义是「本次改动（staged）是否合规」。
  故 `PathSource` 取 `Staged`（`resolve.rs:69`，读 `HOOK_STAGED_FILES` 或
  `git diff --cached`），并在结果面板显式写出用的是哪一种口径。
  未选中需求时保持 `None` 以免用空变更集反查。
- **G4 拦截理由可归因且装得下**：结果面板显示 `BlockKind` 的类别（至少区分
  `Ambiguous` / `SelectionMismatch` / `StepNotApproved` / `NoRequirement` / `UnknownSelection`），
  并在 `hint` 生效时说明「按 <ID> 判定」。当前只渲染 `detail` 首行
  （`tui/src/app.rs:718-720`），`Ambiguous` 的多条理由被截断，看不出该怎么处置。
  **TUI 用浮层而非 footer**：footer 固定 4 行（`ui.rs:26`，去掉边框剩 2 行），
  15 份清单的仓库里 `Ambiguous` 会有 15+ 条理由，固定高度必然截断或撑破布局。
- **G5 核心判定零改动**：`core` 的 `resolve` / `judge` / `gate_check_with` 语义、
  退出码与审计事件一律不动。本需求只改**两个前端各自传什么 `Ctx`**。
  「唯一真相在 core、前端只渲染」这条架构不变量不因本需求松动。
- **G6 回归为零**：本次不碰 `done`/归档、变更范围校验、AC 校验、TUI 渲染
  （这些各有独立缺口，另立需求），避免一次改动面过大而无法定位回归。

### 非目标

- **N1** 不改 `gate_check` 本身，也不删它：它是 CLI `req-guard check` 无参调用的
  合法入口（`cli/src/main.rs:385-411`），语义「全局能不能开工」仍然需要。
- **N2** 不给界面加 `PathSource::Range(base)` / `Stdin`：那是 CI 与 AI PreToolUse 钩子的
  场景，界面里没有「选 base ref」的概念，加了就是无入口的死功能。
- **N3** 不把 `resolve::Verdict` / `BlockKind` 提升到 GUI/TUI 的公共 API 面：
  两端各自在自己的渲染函数里做「类别 → 文案」映射即可，跨 crate 共享一层枚举文案
  收益不足、耦合代价高。
- **N4** 不动 `core` 里的分支名消歧逻辑（`resolve.rs:556-570`，R14）。
  它只在 `hint.is_none()` 且判定为 `Ambiguous` 时生效；本需求给了 `hint` 之后
  该分支自然不再触发，**不需要改它**——但要写一条测试锁住「给了 hint 后
  不再走分支名兜底」，否则以后有人会误加回来。
- **N5** 不做增量/watch 式的自动检查：现状是 3 秒轮询刷新状态（`gui/src/app.rs:161`、
  `tui/src/app.rs:240`），自动检查会让每次切需求都触发一次审计写入，本需求不做。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | `core` 侧零改动确认：核对 `gate_check_with` / `Ctx` / `BlockKind` 的现有签名可满足界面调用 | 0.2h |
| T2 | GUI：`run_check` 改走 `gate_check_with`，按选中项决定 `hint` 与 `source`，结果面板显示口径与 `BlockKind` | 1h |
| T3 | TUI：`run_check` 同上，footer / 结果文案显示口径与 `BlockKind` 类别 | 1h |
| T4 | 单测：多需求仓库下带 `hint` 不再报 `Ambiguous`（GUI/TUI 各一），无选中项时行为与改前一致 | 1.5h |
| T5 | 单测：`hint` 生效后分支名消歧兜底不再触发（锁 N4） | 0.5h |
| T6 | 判决性实验：把 `hint` 改回恒 `None`，对应单测必须 FAIL | 0.3h |
| T7 | 文档：改 `docs/设计/UI架构细化方案.md` 的「落地清单（分阶段）」一节，把 P4 的遗留接线项标注为已收口 | 0.3h |

合计 4.5h。T2 与 T3 是同构改动，可独立回滚；T4/T5 依赖 T2/T3。

### 影响范围

- **模块**：`tui/src/app.rs`（`run_check`）、`gui/src/app.rs`（`run_check`
  与 `Dialog::CheckResult` 渲染）、`docs/设计/UI架构细化方案.md`（§7 状态列）。
- **接口**：**不改任何 core 公开 API**。`gate::gate_check_with`、`resolve::Ctx`、
  `resolve::PathSource`、`GateVerdict::detail/summary/is_pass/bypassed` 全部原样；
  `App::run_check(&mut self)` 的签名也不变（只改内部）。
- **配置 / 清单格式 / 数据表 / 外部 API**：无。`.gates/req-guard.yaml` 不动
  （`multi.mode` 语义不变，只是不再在界面上触发歧义分支）。
- **向后兼容**：无破坏。界面上「检查通过/拦截」的判定输入从「全局无上下文」
  变为「当前选中需求 + staged 变更集」，这是修正而非改语义；
  仓库里只有一份未归档清单时（有 `hint` 与无 `hint` 结果相同）行为完全不变，
  存量单需求仓库零感知。
- **审计**：`resolve::audit_verdict` 已在每次判定时写审计，本需求不改变写入时机与内容，
  只是判定的输入更具体——审计里会开始出现带 `hint` 的裁决记录，属预期。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量）。

## 2. 技术方案


### 总体思路

**只改两端各传什么 `Ctx`，不动 core 任何一行。** 把「界面上有当前选中需求」
这条被丢弃的信息接进 `resolve::Ctx::hint`，并把判定口径从「无变更集」改成
「staged 变更集」，让检查按钮回答评审人真正在问的那个问题。

### 关键设计

1. **`Ctx` 的三个字段各取什么值**（T2/T3 的核心决策，GUI 与 TUI 同构）

   | 情形 | `Ctx.hint` | `Ctx.source` | 语义 |
   | --- | --- | --- | --- |
   | 有选中需求 | `Some(当前 id)` | `PathSource::Staged` | 「我这份需求 + 我 staged 的改动」是否合规 |
   | 无选中需求 | `None` | `PathSource::None` | 与改前逐字一致：全局「能不能开工」 |

   为什么不统一用 `Staged`：`resolve.rs:520-530` 里 `hint=None` 时候选集来自
   `owned_by(live, paths, None, exempt)`——用 staged 变更集反查候选，
   若 staged 为空（评审人还没 `git add`）则候选为空集，
   `BlockKind` 会落到 `NoRequirement`（`resolve.rs:92`「无任何未归档清单」）
   或歧义分支，**比现状更糟**：现状至少稳定报「歧义」，而改动后会在
   「忘了 add」时给出「没有需求」这种误导性理由。无选中项时保持 `None` 是
   fail-open 到现状，不是引入新的失败模式。

   为什么不统一给 `hint`：无选中需求时任何 `hint` 都是编造。`hint` 的语义是
   **显式消歧**，填一个「界面上根本没选」的值会让 `SelectionMismatch`
   （`resolve.rs:99`，"选错了需求，或想用它绕过"）变成可达状态——
   界面不该制造一个能被误读成「试图绕过」的失败态。

2. **为什么不用 `resolve::Verdict` 而是继续用 `GateVerdict`**（N3 的落点）：
   `gate_check_with` 已经把 `Verdict` 转成 `GateVerdict`（`gate.rs:739`），
   三个前端都只认 `Pass` / `Block` 与 `bypassed`（`gate.rs:741-743` 注释明说
   「换形状要动全部渲染层」）。本需求沿用 `GateVerdict`，
   `BlockKind` 的类别从 `detail` 文案里取 —— `verdict_to_gate` 已把
   `BlockKind` 的中文描述放进 `detail` 首条（`gate.rs:750` 附近），
   界面无需新增 core 依赖。

   ⚠️ 连带影响：TUI 当前只渲染 `detail.first()`
   （`tui/src/app.rs:718-720`，`format!("门禁检查：拦截 ⛔（{}）", …)`），
   `Ambiguous` 的多条候选理由会被截断成首行。G4 要求完整展示 `detail`，
   而**扩 footer 是错的路**：`ui.rs:19-26` 的外层布局把 footer 固定为
   `Constraint::Length(4)`（去掉上下边框只剩 2 行可用），
   `Ambiguous` 在 15 份清单的仓库里会产出 15+ 条理由——
   任何固定高度的 footer 都装不下。故新增 `Overlay::Check`：
   footer 只放一行摘要（口径 + 结论），完整 `detail` 放浮层里可滚动，
   `Esc` 关闭。浮层形态照 `Overlay::Audit`（`ui.rs:298`）现成写法。

3. **GUI 结果面板的现状与改法**（T2）：
   `Dialog::CheckResult`（`gui/src/app.rs:1282`）当前渲染 `self.check_detail`
   （`run_check` 里 `v.detail()` 的拷贝 + 绕过时补 `v.summary()`）。
   改法是在 `run_check` 里额外记两个字段：
   - `check_scope: String` —— 人话口径（如「按 REQ-007 判定 · 变更集：staged」）
   - `check_kind: Option<String>` —— `BlockKind` 的类别名
   并在 `CheckResult` 弹窗首行渲染 `check_scope`，拦截时渲染 `check_kind`。
   不新增 core 字段：`GateVerdict` 上没有 `block_kind()`（那是 `Verdict` 才有的方法），
   所以类别只能由前端从 `detail` 里识别 —— 这是 N3「不提升 API 面」的代价，
   已在 G4 的验收里限定为「至少区分这几类」的弱契约。

   保守起见，`check_kind` 的识别走**关键字匹配** `v.summary()`/`detail` 首条
   （如含「歧义」→ `Ambiguous`、含「不是本次改动」→ `SelectionMismatch`），
   而不是给 core 加 `GateVerdict::block_kind()`。理由：给 `GateVerdict` 加方法
   会让三个前端都能用，是更干净的设计；但本需求的核心是接线，
   顺带扩 `GateVerdict` 会把 diff 扩到 `core` + `cli`，
   违反 G5「核心判定零改动」。**这一点记录为遗留**，
   后续若多个前端都需要类别名，应单独立项给 `GateVerdict` 加 `block_kind()`。

4. **分支名消歧兜底不再触发**（N4 的锁法，T5）：
   `resolve.rs:556-570` 的 R14 分支只在
   `verdict.block_kind() == Some(Ambiguous) && ctx.hint.is_none()` 时兜底。
   本需求给了 `hint` 后该条件不成立，走的是 `judge(live, paths, Some(id), …)`。
   测试写法：造一个「分支名指向 REQ-002、但 `hint` 显式给 REQ-001」的仓库，
   断言 `gate_check_with` 的结果按 REQ-001 判定（而非分支名兜底的 REQ-002）——
   即验证「显式选择优先于分支名」这条不变量，而不只是验证「不是 Ambiguous」。

5. **凭据与审计**：检查是只读判定，不走 `auth::ensure_human`
   （对照：`done` / `archive` / `bypass` / `review` 才需要）。本需求
   **不签发界面凭据**，符合 `auth.rs:246-250` 关于「只读动作不该消耗审批凭据」的
   既有分工（GUI 只在 `approve`/`reject`/`amend`/`seal`/`resolve`/`bypass`
   六处调 `prepare_credential`）。⚠️ 顺带修正一个既有越界：
   `gui/src/app.rs:409` 的 `do_bypass` 与 `tui/src/app.rs:860` 的 `bypass`
   都把 TTL 硬编码为 `60`，而 `gate::bypass` 允许到 240（`gate.rs:1063`），
   配置里的 `bypass.default_ttl_minutes: 60` 也没人读。
   **这不是本需求的范围**（G6），记录在此供另立需求时取用。

6. **`.gates/req-guard.yaml` 不动**：`multi.mode` 的两种取值语义都不变
   （`resolve.rs:55` 的 `Resolve` 与 `All`），本需求只是让界面不再触发
   `Resolve` 下的歧义分支。`All` 档本就无歧义，界面行为不变。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
tui/src/app.rs
gui/src/app.rs
docs/设计/UI架构细化方案.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：不改任何 core 公开 API，不改 CLI 行为，不改清单格式与配置键。
  界面 `App::run_check` 签名不变。存量单需求仓库（有 `hint` 与无 `hint` 结果一致）
  行为完全不变。
- **性能**：多了 `git diff --cached`（`PathSource::Staged` 的取集路径，
  `resolve.rs:552` 的 `collect_paths`）。这是**点按钮时才发生**的一次 git 调用，
  不在 3 秒轮询路径上，故不影响空闲时的 CPU。TUI 单次检查的响应延迟增加
  量级为一次 `git diff --cached`（本仓库规模 < 10ms）。
- **安全**：本需求**放宽**了界面检查的判定输入（加了 `hint` 与变更集），
  方向上是「更容易判通过」。必须说清为什么这不是降低安全性：
  `hint` 只影响**归属判定**（这次改动算哪份需求的），
  `StepNotApproved` / 内容冻结 / 阻塞评论这些**硬拦截条件**
  （`resolve.rs` 的 R1–R15）一个都不受影响；且审核人点检查前，
  他在界面上做的选择就是他的显式意图。真正的兜底——`approve` / `done` 的
  `ensure_human` 与 L3 一次性票据——完全不在本路径上。
  反过来说：**现状恒报拦截其实是在用误报掩盖信息缺失**，那不是安全，是噪声。
- **审计**：`resolve::audit_verdict` 的写入时机与格式不变。界面的检查会开始
  在台账里留下带 `hint` 的裁决记录——这是本次修正的可观测副产品，属预期。

### 风险点与回滚方案

- **风险 1（中）**：多行 `detail` 塞进 TUI footer 会被截断（footer 只有 2 行可用，
  `ui.rs:26` 的 `Constraint::Length(4)`）或撑破布局。
  缓解：改用 `Overlay::Check` 浮层 + 可滚动（照 `Overlay::Audit` 现有写法），
  footer 只留一行摘要；用 `TestBackend` 断言 6 条理由完整可见
  （`ui.rs` 已有 15 条渲染冒烟测试可扩展），不靠肉眼看终端。
- **风险 2（中）**：`Staged` 变更集为空时（评审人未 `git add`）结果具有误导性。
  缓解：结果面板显式写出「变更集：staged（空）」——
  **把「空」这个事实显示出来，而不是让它退化成另一个错误理由**。
  这是 G3 要求显示口径的直接原因。
- **风险 3（低）**：`hint` 指向的 id 已被归档 / 不存在（并发场景：另一进程刚把它 done 了）。
  此时 `judge` 报 `UnknownSelection`，而 `resolve.rs:576-586` 的
  `enrich_archived` 会补充「该清单已归档」说明。缓解：第 3 段的验收条目覆盖此路径，
  断言界面显示的是「已归档」而不是裸的 `UnknownSelection`。
- **风险 4（低）**：`gate_check` 失去调用方后被误删。
  缓解：CLI 无参 `check` 仍在用（`cli/src/main.rs:385-411`），N1 已说明保留理由；
  且 core 有单测直接调它。删除会在 CI 上立刻暴露。
- **回滚**：`git revert` 即可，无数据迁移、无格式变更、无配置变更。
  T2（GUI）与 T3（TUI）是同构但独立的改动，可分别回滚。

## 3. 测试计划

### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | `core`：`gate_check` 与 `gate_check_with(hint=None, source=None)` 在同一仓库上结果一致 | 两者 `is_pass()` 与 `detail()` 逐字相同（证明 G2 的「无选中项时行为不变」） |
| U2 | GUI 状态机：造含 2 份已批清单的仓库，选中其中一份，点检查 | 结果为放行；`check_scope` 含该 id；`check_kind` 不是歧义 |
| U3 | TUI 状态机：同 U2 仓库，同样操作 | `message` 含「放行」且不含「歧义」 |
| U4 | GUI：无选中需求时点检查 | 与 U1 一致（多需求下仍报歧义，且 `check_scope` 显示「未选择需求」） |
| U5 | TUI：无选中需求时点检查 | 同 U4 |
| U6 | `resolve`：分支名指向另一份清单 + `hint` 显式给当前份 | 结果按 `hint` 判定，不走分支名兜底（锁 N4） |
| U7 | `resolve`：仅 1 份未归档清单，分别传与不传 `hint` | 两者结果相同（存量单需求仓库零感知） |
| U8 | `resolve`：`hint` 指向已归档清单 | `block_kind()` 为「显式选择指向的清单不存在」类，且 message 含「已归档」 |
| U9 | `resolve`：`source=Staged` 且暂存区为空 | `paths` 为空向量，不 panic、不返回 `Err` |
| U10 | TUI 渲染：`TestBackend` 渲染一条含 6 条 `detail` 的拦截消息 | 6 条全部落在检查浮层缓冲区里；footer 只 1 行摘要且不被截断（锁风险 1） |
| U11 | GUI 渲染：`Dialog::CheckResult` 渲染拦截结果 | 首行含口径说明，且 `check_kind` 出现在文本里 |
| U12 | 既有 core / cli / tui / gui 全部单测 | 全绿，条数不减（回归兜底） |
| U13 | 判决性实验：把 GUI 的 `hint` 改回恒 `None` | U2 必须 FAIL（`check_scope` 不含 id，且结果变歧义） |
| U14 | 判决性实验：把 TUI 的 `hint` 改回恒 `None` | U3 必须 FAIL（同上） |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 在本仓库（14 份 live 清单）起 GUI，选中 `REQ-013`，点「执行门禁检查」 | 结果按 `REQ-013` 判定；面板首行写明「按 REQ-013 判定 · 变更集：staged」，不再是「歧义」 |
| E2 | 同上 TUI 按 `g` | footer 显示同样的口径与放行/拦截结论，多行理由完整可见不被截断 |
| E3 | 清空暂存区（`git reset`）后在 GUI 再点一次 | 面板显式写出「变更集：staged（空）」，不以「歧义」或「无需求」搪塞 |
| E4 | 选中一份已被 `done` 的清单（用归档区浏览路径构造，或临时手改 HEAD） | 显示「该清单已归档」而不是裸的未知选择 |
| E5 | 在只有一份清单的临时仓库里开界面点检查 | 与升级前逐字一致（无 hint 与有 hint 同结果） |

### 边界 / 异常场景

- **B1** 暂存区为空且有选中需求：口径行显示「空」，结论按该需求的步骤/评论状态给，
  不退化成歧义。
- **B2** 选中需求在检查过程中被另一进程 `done` 掉：报「已归档」（`enrich_archived`
  路径），界面不崩、不显示内部枚举名。
- **B3** `hint` 指向的 id 在 live 集内但与 staged 变更集不相交：
  报「选择与本次改动不相交」——这是**诚实且有用**的提示（用户可能 add 错了需求），
  不得为了让它变绿而静默改判。
- **B4** 仓库 0 份清单：无选中项可点，检查应显示「无任何未归档清单」
  （`BlockKind::NoRequirement`），而非崩溃或空面板。
- **B5** `git diff --cached` 失败（非 git 仓 / git 不可用）：
  `collect_paths` 返回 `Err`，界面显示错误文案而非静默按空变更集处理。
- **B6** 同一 `App` 上连续点多次检查：每次都是独立判定，界面状态不残留上一次的
  `check_kind` / `check_scope`（放行后 `check_kind` 必须被清空）。
- **B7** 绕过窗口生效中：`Verdict::Bypassed` → `GateVerdict::Pass{bypassed:true}`，
  界面必须仍显示「⚠️ 命中应急绕过」且口径行照常显示——绕过不等于正常放行。

### 回归范围与影响面

- `cargo test -p req-guard-core` 全量：本需求不改 core，**结论应一字不变**。
  这是「G5 核心判定零改动」的直接验证——若 core 有测试因此变化，说明改动越界了。
- `cargo test -p req-guard` （CLI）：同上，不改 CLI。
- `cargo test -p req-guard-tui` / `cargo test -p req-guard-gui`：新增 U2–U6、U10、U11，
  且既有测试条数不减（TUI 15 条渲染冒烟 + 状态测试，GUI 15 条 app 测试 + 23 条 markdown）。
- `cargo fmt -p req-guard-tui -p req-guard-gui -- --check` 零 diff。
- `cargo clippy -p req-guard-tui -p req-guard-gui --all-targets` 零 warning。
- `cargo build`（default features）仍秒级、依赖图不变（本需求不加依赖）。
- 手工对照：改前在 14 份清单的仓库点检查（记下文案），改后同一操作必须不同。

### 验收门槛

- `cargo fmt --check` 与 `cargo clippy --all-targets` 零输出。
- `cargo test -p req-guard-core` 与 `cargo test -p req-guard` 全绿，且**测试数与改前完全相同**
  （core/cli 零改动的硬证据）。
- `cargo test -p req-guard-tui` 与 `cargo test -p req-guard-gui` 全绿，
  且测试总数 ≥ 改前总数 + 8（U2–U6、U10、U11、U14 中新增的 8 条）。
- `cargo tree -p req-guard` 与 `cargo tree -p req-guard-tui`、`-p req-guard-gui`
  的依赖集合与改前一致（本需求不新增任何依赖）。
- **判决性实验**：把 GUI 与 TUI 的 `hint` 各自改回恒 `None`，
  U2/U3/U13/U14 必须 FAIL（自证的测试不算判决）。

<!-- GATE:AC -->
### AC-001
- Given: 一个含两份清单（REQ-001 与 REQ-002）且三段全部已批准的临时仓库，两份均未归档
- When: 在该仓库上分别调用全局门禁判定入口与带显式选择、变更集来源为暂存区的判定入口，且后者显式选择 REQ-001
- Then: 前者的裁决类别为歧义类，后者为放行，即两者的 `is_pass()` 与明细文本均不相同

### AC-002
- Given: 一个含 REQ-001 与 REQ-002 两份已批清单的临时仓库，且当前界面选中项为 REQ-001
- When: 触发 GUI 的执行门禁检查动作
- Then: 结果弹窗首行包含字面量 `REQ-001`，且全文不含「歧义」二字

### AC-003
- Given: 一个含 REQ-001 与 REQ-002 两份已批清单的临时仓库，且当前界面选中项为 REQ-001
- When: 在 TUI 中触发门禁检查按键
- Then: 界面消息包含字面量 `放行`，且全文不出现 `歧义` 二字

### AC-004
- Given: 一个含 REQ-001 与 REQ-002 两份已批清单的临时仓库，且界面没有任何选中需求
- When: 触发门禁检查动作
- Then: 结果与调用全局判定入口所得的裁决逐字一致，即仍为 `Ambiguous` 类拦截

### AC-005
- Given: 一个含 REQ-001 与 REQ-002 两份清单、当前 git 分支名包含 `REQ-002` 字样的临时仓库
- When: 以显式选择 REQ-001 与变更集来源为暂存区调用判定入口
- Then: 裁决依据为 REQ-001 而非 REQ-002，即分支名兜底未生效

### AC-006
- Given: 一个含 REQ-001 与 REQ-002 两份已批清单的临时仓库，且 REQ-002 的清单已被置为已归档状态
- When: 以显式选择 REQ-002 调用判定入口
- Then: 返回的消息包含 `已归档` 三字，且不出现形如 `UnknownSelection` 的内部枚举名

### AC-007
- Given: 一个含单份清单 REQ-001 且三段全部已批准的临时 git 仓库，其暂存区为空
- When: 分别以无显式选择与显式选择 REQ-001 两种上下文调用判定入口
- Then: 两次裁决的 `is_pass()` 与明细文本逐字相同

### AC-008
- Given: 实现完成后的 tui crate，其门禁检查结果含 6 条明细理由
- When: 打开检查结果浮层并用离屏测试后端渲染该界面
- Then: 6 条理由全部在该后端的字符缓冲区中被检索到，且底部摘要区只占 1 行

### AC-009
- Given: 实现完成后的 tui crate 与 gui crate 各自的门禁检查动作
- When: 把两处传入的显式选择参数都改回恒为空值
- Then: 恰好有 2 条单测失败，即该消歧行为不是自证出来的

### AC-010
- Given: 实现完成后的 core crate 与 cli crate
- When: 依次执行 `cargo test -p req-guard-core` 与 `cargo test -p req-guard`
- Then: 两者失败数均为 0，且各自测试总数与本次改动前的记录条数相同

### AC-011
- Given: 实现完成后的 tui crate 与 gui crate
- When: 依次执行 `cargo test -p req-guard-tui` 与 `cargo test -p req-guard-gui`
- Then: 两者失败数均为 0，且各自测试总数不少于改动前总数加 8

### AC-012
- Given: 实现完成后的 tui crate 与 gui crate
- When: 执行 `cargo fmt` 检查与 `cargo clippy --all-targets`
- Then: 前者无 diff 输出，后者 warning 数为 0

### AC-013
- Given: 实现完成后的 workspace
- When: 对 core、tui、gui 三个 crate 分别执行 `cargo tree` 并比对改动前的依赖集合
- Then: 3 个 crate 的依赖集合与改动前逐项相同，即本次改动未新增任何依赖

### AC-014
- Given: 一个含 REQ-001 与 REQ-002 两份已批清单的临时 git 仓库，其暂存区没有任何改动
- When: 以显式选择 REQ-001 且变更集来源为暂存区调用判定入口
- Then: 得到的变更集路径数量为 0，裁决为放行，且不返回任何 `Err`
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-05_14:19:49 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-05_14:20:01 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-05_14:20:09 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

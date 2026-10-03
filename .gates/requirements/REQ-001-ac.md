# REQ-001 AC 与变更范围契约

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-001 status=approved created=2026-10-03_21:47:56 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-03_22:22:41 -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-03_22:23:14 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-03_23:15:02 -->

## 1. 需求分解

### 背景与问题

req-guard 已经把"三段清单写完并被批准"变成硬拦截，但**只验审批状态、不验内容**：
`HOOK_SH` 第 3 段只 `grep 'GATE:STEP'` 判 `status=approved`。于是三段可以同时是空话——
需求分解写"提升代码质量"，技术方案写"重构相关模块"，测试计划写"补充单元测试"，
照样过审解锁。门禁从"逼出思考"退化为"填表过审"，评审成本却一分没省。

两处具体失效：

1. **第三段无信息量**：验收标准不可判定 → 测试计划无法被任何自动化校验消费，回归全靠人肉。
2. **第二段与代码脱钩**：`SOLUTION_BODY` 里的"涉及的文件与模块清单"是散文，
   实际改动 40 个文件也没人比对，范围悄悄扩张（scope creep）没有任何拦截点。

### 目标

- **G1** 三段内容本身可被机械校验，消灭"三段都是空话"。
- **G2** 技术方案声明的变更范围成为可执行契约：声明要改的 ⊇ 实际改的，超出即拦。
- **G3** 两个契约都接在**已有的唯一强制点**上（审批 / pre-commit），不依赖任何人记得跑检查命令。

### 非目标

- **N1** 不做自然语言质量判断（"写得清不清晰"无法机械判定，误报会催生绕过）。
- **N2** 本期不实现追溯矩阵 UI，只把 join key 落到文档里。
- **N3** 不改 AI 写路径的既有拦截（`HOOK_SH` / `HOOK_PS1` 一行不动）。
- **N4** 不引入任何外部 crate（core 零依赖铁律）。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | `section_span` 抽取 + `core/src/issue.rs` + 模板骨架 + `gate_lines` 扩展（P1） | 2h |
| T2 | `core/src/ac.rs`（parse / lint / check）+ 13 条规则 table-driven 单测（P2） | 3h |
| T3 | `ac check` 子命令 + `review()` 审批挂钩（P2） | 2h |
| T4 | `core/src/touch.rs`（含零依赖 `glob_match`）+ `touch --declare`（P3，只读不拦） | 4h |
| T5 | 独立 touch 脚本 + `PRE_COMMIT_BLOCK` + `verify_install` 扩展（P4） | 3h |
| T6 | `--base` L3 抵消 + CI/CNB 接入 + `verify_gate.py` 新场景（P4） | 3h |
| T7 | 文档同步（设计文档 / 规范 / README / 用户手册 / CHANGELOG） | 2h |

合计约 19h。P1–P4 各自独立可合可回滚；P3→P4 之间留"只读不拦"观察期。

### 影响范围

- **模块**：core（新增 `issue`/`ac`/`touch`，改 `requirement`/`gate`）、cli（新增两个子命令）。
- **配置**：`.gates/req-guard.yaml` 新增 `touch.{exempt,scope,reapprove}`（块级零依赖解析）。
- **清单格式**：新增两种**可选**标记块（`GATE:AC` / `GATE:TOUCH`），旧格式继续可读可判。
- **接口 / 数据表 / 外部 API**：无。
- **向后兼容**：存量清单在 P2 上线后 `ac check` 会红 → 必须先落 `ac init` 迁移命令再开 Error。
- **验收标准**：见第 3 段 `GATE:AC` 块（编号 + Given/When/Then，可机械判定）。

## 2. 技术方案

### 总体思路

在"人读散文"旁边加一块"机器读契约"，再把契约校验挂到**已有的两个唯一强制点**上：
审批（`requirement::review`）与提交（`.git/hooks/pre-commit`）。

关键判断：**新增独立检查命令本身没有约束力**——没人跑的命令等于没有命令。
所以强制点都选在既有出口上，`ac check` / `touch-check` 只是把同一判据提前暴露给 AI 与 CI，用于早失败。

### 关键设计

1. **段落定位必须严格**：新增 `requirement::section_span` 返回 `Option` 行区间；
   `section_of` 基于它实现且对外行为不变（仍回退整篇，保审核人界面不白屏）；
   `ac::check` / `touch::check` 用严格版 —— 否则标题写坏的文档会拿全文去判并误判通过。
2. **判定只在 core**：shell 只取 staged 文件集、只渲染 stderr。新判定不写进 `HOOK_SH`
   （该脚本同时服务 AI PreToolUse 与 pre-commit 两个上下文，且每行都要在 `HOOK_PS1` 镜像，是纯负债）。
3. **反向包含**：实际 ⊆ 声明。声明多写无罪，漏写有罪 —— 方案可以写得比实现细，不能比实现窄。
4. **并集口径**：多份未归档清单的声明取并集，`HOOK_REQ` 可收窄。误报是头号死因，
   按"当前活跃需求"单条比对必然误伤并催生 `--no-verify`。
5. **L3 抵消必做**：`touch-check --base` 与 pre-commit 共用同一份判定，
   否则 `--no-verify` 一穿而过，"闭环"不成立。
6. **范围扩张合法化**：`touch --declare` 走 `ensure_human` + 追加声明 + 打回 solution 重审。
7. **防删块**：`gate_lines` 纳入新标记行，否则 AI 删掉整个 `GATE:AC` 块即永久绕过。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
core/src/issue.rs
core/src/ac.rs
core/src/touch.rs
core/src/idcheck.rs
core/src/lib.rs
core/src/requirement.rs
core/src/gate.rs
cli/src/cli.rs
cli/src/main.rs
templates/hooks/fragments/reqguard-check.sh
templates/ci/req-guard-ci.yml
scripts/verify_gate.py
.cnb.yml
README.md
docs/README.md
docs/设计/AC与变更范围契约技术方案.md
docs/规范/AI工具合规保证规范.md
packaging/docs/用户手册.md
packaging/docs/校验说明.md
packaging/CHANGELOG.md
<!-- /GATE:TOUCH -->

声明之外**不改**：`core/Cargo.toml`（零依赖铁律）、`HOOK_SH` / `HOOK_PS1`（AI 写路径）、
`status.rs`（TUI/GUI 数据模型不变）。`.gates/**` 按默认豁免处理，门禁不自锁。

### 兼容性、性能与安全影响

- **兼容性**：清单格式只新增可选块；`section_of` 对外行为不变；`Severity` 迁到 `core/src/issue.rs`
  后 `idcheck` 改 `pub use` 重导出，**对外 API 不破**。唯一破坏性变更：P2 上线后存量清单 `ac check` 变红。
- **性能**：`ac::lint` / `touch::check` 是单文件线性扫描 + 通配匹配（`declared × staged` 次匹配），
  pre-commit 多一次毫秒级二进制启动。
- **安全**：不新增凭据路径；`touch --declare` 复用既有 `auth::ensure_human` + `identity::bind`。
  本机可绕过（`--no-verify`、`.gates/.bypass`）由 L3 `--base` 抵消。

### 风险点与回滚方案

| 风险 | 处置 |
| --- | --- |
| 误报（头号死因：门禁被习惯性绕过 = 没有门禁） | 并集默认口径 + exempt 最小集 + `--declare` 合法出路 + P3→P4 只读期 |
| 存量清单迁移踩空 | 先落 `ac init`，再开 P2 的 Error |
| `glob_match` 语义歧义（`**` 是否匹配零段） | 语义写死进 doc comment + 单测表；不支持的花括号必须在契约里明文声明 |
| L0 无审批锁下 AI 可自行 `--declare` | 与 `core/src/auth.rs` 信任模型同源，文档写明，不额外兜底 |
| 晚失败（写完 40 个文件才在 pre-commit 被拦） | 这是墙的性质；靠"打回 solution 重审"把它变成改方案的循环而非绕过的循环 |

**回滚**：P1–P4 各自独立可合可回滚。P4 回滚 = 移除 `PRE_COMMIT_BLOCK` 的 touch 段与
`verify_install` 的新增项；`touch.*` 键可留在 yaml 里不被读取，不留半吊子配置。

## 3. 测试计划

### 验收标准

<!-- GATE:AC -->
### AC-001
- Given: 某清单第 3 段完全没有 GATE:AC 标记块
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 MissingBlock，message 含可粘贴的条目骨架

### AC-002
- Given: 已安装含 P1 模板的 req-guard，执行 create 生成一份全新清单
- When: 读取该清单第 2 段与第 3 段
- Then: 第 3 段含且仅含 1 对 GATE:AC 标记且块内含 1 条 AC-001 示例，第 2 段同含 1 对 GATE:TOUCH 且块内至少 1 条路径

### AC-003
- Given: 某清单第 3 段块内含 1 条三行式条目与 1 条单行式条目
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 0，首行为 `✅ 验收标准合规：REQ-001 共 2 条 AC`，随后 2 行行首为各条编号

### AC-004
- Given: 某条目缺 When 子句
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 MissingClause，message 含该条编号、1-based 行号与缺失子句名

### AC-005
- Given: 某块式条目子句顺序为 Given/Then/When
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 OrderClause 而非 MissingClause，message 含实际顺序

### AC-006
- Given: 条目编号分别跳号、重复、非 AC 前缀各一条
- When: 执行 ac check REQ-001
- Then: 退出码 1 且分别报 SeqGap、DuplicateId、BadIdFormat 三种 kind

### AC-007
- Given: 某条目子句形如空占位（Given 后仅 1 个非空白字符）
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 EmptyClause，message 含该条编号与行号

### AC-008
- Given: 第 3 段内 GATE:AC 块之外出现编号 AC-007
- When: 执行 ac check REQ-001
- Then: 退出码 1 且报 OutsideBlock

### AC-009
- Given: 某清单只写了 GATE:AC 起始标记而无结束标记
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 UnclosedBlock，message 含缺失的结束标记字面量

### AC-010
- Given: 某清单第 3 段的二级标题被改写成非法形式
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 SectionNotFound，不得回退整篇后误判通过

### AC-011
- Given: 某条目三子句分别写作 `假设…`、`当…`、`则…`
- When: 执行 ac check REQ-001
- Then: 退出码 0，该条被识别且不报 MissingClause

### AC-012
- Given: 某条目第 1 子句写 `Given …` 而第 3 子句写 `则…`
- When: 执行 ac check REQ-001
- Then: 退出码 0，报 AliasMix 且标注为警告级

### AC-013
- Given: 仅命中 A5 与 A9 两类告警而无任何硬伤
- When: 执行 ac check REQ-001
- Then: 退出码 0，且每条告警都逐行打印，一条都不静默丢弃

### AC-014
- Given: 同时存在 1 项硬伤与 2 项告警
- When: 执行 ac check REQ-001
- Then: 退出码 1，输出中硬伤行全部排在告警行之前，stderr 汇总含两类计数

### AC-015
- Given: 归档区存在 1 份不合规清单且未归档清单均合规
- When: 先执行 ac check 再执行 ac check --all
- Then: 前者退出码 0 不报归档那份，后者退出码 1 并报出该归档清单编号

### AC-016
- Given: 某清单 GATE:AC 块内前 3 条为块式、后 4 条为单行式
- When: 执行 `req-guard ac check REQ-001`
- Then: 7 条全部被识别为合法条目，无一条被误判为格式错误

### AC-017
- Given: 某清单正文（非标记行）含 GATE:STEP name=solution 与 status=approved 散文
- When: 执行 `req-guard approve REQ-001 --step solution`
- Then: 该散文行逐字不变，且全文 GATE:STEP 标记行数量恒为 3（不被注入第 4 条）

### AC-018
- Given: 某清单正文含 1 行提到 GATE:STEP 的散文
- When: AI 用 Write 整篇覆盖该清单并改动那行散文
- Then: 退出码 0 放行，不得报状态行被篡改（只有以注释标记开头的行才算状态行）

### AC-019
- Given: 第 3 段无任何条目
- When: 执行 approve REQ-001 --step testplan
- Then: 退出码 1 且 testplan 保持 status=pending

### AC-020
- Given: 第 3 段条目齐全且编号连续
- When: 执行 approve REQ-001 --step testplan
- Then: 退出码 0 且 testplan 的 status 变为 approved

### AC-021
- Given: 技术方案段没有 GATE:TOUCH 块
- When: 执行 approve REQ-001 --step solution
- Then: 退出码 1 且 solution 保持 status=pending

### AC-022
- Given: 用 Write 整篇覆盖清单并删掉 GATE:AC 标记行
- When: 经 AI 工具 hook 写入
- Then: 退出码 1 拦截，理由含状态行不得改动

### AC-023
- Given: 把某清单第 2 段标题改坏
- When: 执行 req-guard status 并渲染该段
- Then: 仍回退整篇显示且段数不为 0（不白屏），section_span 返回空值

### AC-024
- Given: 清单 TOUCH 声明含 core/src/** 与 core/src/ac.rs
- When: 新增 core/src/ac.rs 并暂存后执行 touch-check
- Then: 退出码 0，声明为超集时放行

### AC-025
- Given: 清单 TOUCH 声明为 core/src/** 与 core/src/ac.rs
- When: 暂存未声明的 cli/src/main.rs 后执行 touch-check
- Then: 退出码 1，stderr 逐条列出未声明文件，并同时给出改方案、声明范围、绕行三条出路

### AC-026
- Given: 清单 TOUCH 仅声明 core/src/**
- When: 暂存 .gates/requirements/REQ-001-ac.md
- Then: 退出码 0，.gates/** 默认豁免，门禁不得自锁

### AC-027
- Given: 某清单第 2 段无 GATE:TOUCH 块或块内无有效条目
- When: 执行 touch-check
- Then: 退出码 1，分别报 MissingBlock 与 EmptyDeclaration

### AC-028
- Given: TOUCH 某条目为 ../outside/**
- When: 执行 touch-check
- Then: 退出码 1，报 BadPath，且该条目不参与匹配

### AC-029
- Given: 显式设置 HOOK_STAGED_FILES 为 2 条已声明路径，且当前不在 git 仓库内
- When: 执行 touch-check
- Then: 退出码 0，完全依据该环境变量判定而不调用 git

### AC-030
- Given: 未设置 HOOK_STAGED_FILES 且暂存区含已声明文件
- When: 执行 touch-check
- Then: 退出码 0，且自行回落 git diff --cached 取参成功

### AC-031
- Given: 未归档清单声明 core/**，另一份 done 清单声明 cli/**
- When: 暂存 cli/src/main.rs 后执行 touch-check
- Then: 退出码 1，done 清单的声明不参与并集

### AC-032
- Given: 两份未归档清单分别声明 core/** 与 cli/**
- When: 分别暂存两侧文件
- Then: 两次执行均退出码 0，并集口径生效

### AC-033
- Given: 未归档清单 A 声明 core/**、清单 B 声明 cli/**，且 touch.scope 设为 strict
- When: 暂存 cli/** 后执行 touch-check
- Then: 退出码 1，strict 口径只比最新活跃清单 A

### AC-034
- Given: 活跃清单 TOUCH 仅声明 core/src/**
- When: 执行 touch --declare 追加 cli/src/** 并填写原因
- Then: 退出码 0，TOUCH 块新增该行、solution 被打回 pending、ledger.md 出现 TOUCH.EXTEND 事件

### AC-035
- Given: touch.reapprove 设为 false
- When: 执行 touch --declare 追加声明
- Then: 退出码 0 且 TOUCH 块新增该行，但 solution 保持 approved 不被打回

### AC-036
- Given: PR 分支相对 origin/main 新增了未声明文件
- When: CI 执行 touch-check --base origin/main
- Then: 退出码 1，--no-verify 在服务端被抵消

### AC-037
- Given: .gates/ci/req-guard-ci.yml 缺 touch-check --base 或 ac check 任一步骤
- When: 执行 req-guard install --verify
- Then: 退出码 1 并报 L3 缺口，模板补齐后转绿

### AC-038
- Given: 开发者新增一个 AcIssueKind 或 TouchIssueKind 变体但未写任何测试
- When: 执行 cargo test --workspace
- Then: 枚举覆盖用例失败并指名该变体，cargo test 退出码非 0

### AC-039
- Given: 1000 条声明与 200 个暂存文件
- When: 执行 touch-check 并计时
- Then: 本机耗时小于 200 毫秒，CI 上小于 2 秒

### AC-040
- Given: 某条编号行写作三级标题且行内混入三子句
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 DirtyIdLine，该条不被识别为合法条目

### AC-041
- Given: 某条编号行与子句分行且编号独占标题行
- When: 执行 ac check REQ-001
- Then: 退出码 0，该条被识别且编号不出现在任何标题正文里

### AC-042
- Given: 某条 Given 写 `与上一条相同`
- When: 执行 ac check REQ-001
- Then: 退出码 1，报 NonSelfContained 并指名该条编号与行号

### AC-043
- Given: 某条 Given 写 `同 AC-032` 这类外部编号
- When: 执行 ac check REQ-001
- Then: 退出码 1，报 NonSelfContained，message 提示须原文写出前置条件

### AC-044
- Given: GATE:AC 块内混入一个非条目用的分组小标题
- When: 执行 ac check REQ-001
- Then: 退出码 1，报 StrayHeading 并给出该小标题字面量

### AC-045
- Given: 某条编号行的下一非空行含 2 个以上全角 ｜ 分隔符
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 1，报 InlineEntry，该条不被识别为合法条目

### AC-046
- Given: 某条编号行之后跟 3 个子句列表项且编号独占标题行
- When: 执行 `req-guard ac check REQ-001`
- Then: 退出码 0，三行式是该契约下唯一合法的条目形态

### AC-047
- Given: 某清单第 2 段 TOUCH 块声明 core/src/** 与 cli/src/cli.rs
- When: 执行 approve REQ-001 --step solution
- Then: frontmatter 的 source_refs 被写入 2 个条目 [core/src, cli/src]，TOUCH 块本身逐字不变

### AC-048
- Given: TOUCH 块含 core/src/** 与 core/src/*.rs 两条指向同目录的条目
- When: 执行 approve REQ-001 --step solution
- Then: source_refs 中该目录只出现 1 次（去重），glob 与文件路径均不残留在 frontmatter

### AC-049
- Given: frontmatter 的 source_refs 现值为 [docs]
- When: TOUCH 块声明 core/src/** 后执行 approve REQ-001 --step solution
- Then: source_refs 的 1 个条目被覆盖为 [core/src]，且 ledger.md 出现 1 条 DERIVE_SOURCE_REFS 事件

### AC-050
- Given: 有人手工把 frontmatter 的 source_refs 改成与 TOUCH 块不同的值
- When: 执行 approve REQ-001 --step solution
- Then: 该手工值被覆盖回派生值（全清单 1 处），声明源唯一性不被绕过，且审计事件含被覆盖的旧值

### AC-051
- Given: 某清单第 2 段 TOUCH 块为空
- When: 执行 approve REQ-001 --step solution
- Then: 退出码 1，且 frontmatter 的 source_refs 保持原值不被写入空数组

### AC-052
- Given: 变更范围声明经派生后精度降级（core/src/ac.rs 归约为 core/src）
- When: doc-guard 的 FRS004 复核该清单
- Then: 误差方向为多报规格腐化而非漏报，且该偏差以 1 条记录进风险表

### AC-053
- Given: 某清单 TOUCH 块声明 **/*.rs 与 core/** 两条
- When: 执行 approve REQ-001 --step solution
- Then: source_refs 只写入 1 项 core，且 stderr 有告警点名 **/*.rs 无法表达，不得静默丢弃

<!-- /GATE:AC -->

### 单元测试用例（编号 + 断言点）

- **U-01** `core/src/ac.rs` 合法块式 → `parse` 出 1 条，三子句均非空
- **U-02** `core/src/ac.rs` 合法单行式（`|` 切 4 段）→ `parse` 出 1 条
- **U-03** `core/src/ac.rs` **块式与单行式混排** → 逐条判定形式，全部识别，无一误判
- **U-04** `core/src/ac.rs` 缺 When → `lint` 出 MissingClause
- **U-05** `core/src/ac.rs` 顺序颠倒 → `lint` 出 **OrderClause**（不再是 MissingClause）
- **U-06** `core/src/ac.rs` 跳号 / 重复 / 非 AC 前缀 → SeqGap / DuplicateId / BadIdFormat
- **U-07** `core/src/ac.rs` 空子句 → EmptyClause；块外编号 → OutsideBlock
- **U-08** `core/src/ac.rs` 零条目 / 缺块 / 未闭合 → NoItem / MissingBlock / UnclosedBlock
- **U-09** `core/src/ac.rs` 中文别名可解析；同条中英混用 → AliasMix（Warn）
- **U-10** `core/src/requirement.rs` 标题写坏 → `section_span` 返回 `None`，且 `section_of` 仍回退整篇
- **U-11** `core/src/touch.rs` glob 表：`**` 跨层、`*` 不跨层、`**/*.rs` 命中根级、`?` 单字符
- **U-12** `core/src/touch.rs` 声明 ⊇ 实际：命中 / 超出各一；`.gates/**` 豁免
- **U-13** `core/src/touch.rs` `..` 逃逸 → BadPath；空块 → EmptyDeclaration
- **U-14** `core/src/touch.rs` 并集 vs `strict` vs `done` 清单不参与并集
- **U-15** `core/src/idcheck.rs` 改用 `issue::Severity` 后既有用例全绿
- **U-16**（**P0 回归**）`core/src/requirement.rs` `review`：正文含 `GATE:STEP name=<step>` 散文时，
  该行逐字不变、全文标记行数量不增加、`label=` 不被写成空值
- **U-17**（**P0 回归**）`core/src/gate.rs` `gate_lines`：只收集以注释标记开头的行，
  正文提及 GATE:STEP 的行不进集合 → `doc_write_guard` 不误拦 AI 正常改正文
- **U-18**（**枚举覆盖**）遍历 `AcIssueKind` 与 `TouchIssueKind` 全量变体，断言每个变体至少被
  一个 case 命中；新增变体未写测试即红

### 端到端用例（编号 + 执行步骤）

- **E-01** `init` → `create` → 填三段 → 逐段 `approve` → `check` 放行（新增两契约后仍放行）
- **E-02** 第 3 段无条目时 `approve --step testplan` 被拒，且状态仍 pending
- **E-03** 暂存未声明文件 → touch-check 拒绝；`--no-verify` 后 CI `--base` 仍拒绝
- **E-04** `touch --declare` → 追加声明 + solution 打回 → 重新 approve → 放行
- **E-05** AI 用 Write 删掉 `GATE:AC` 标记行 → hook-check 拦截
- **E-06**（**P0 回归**）清单正文含 `GATE:STEP name=testplan` 散文 → 依次
  `reject`/`approve --step testplan` 各一次 → 该散文行与 AC 条目均完好，
  全文 GATE:STEP 标记行仍只有文件头 3 条

### 边界 / 异常 / 并发场景

- **B-01** 空清单文件 / 非 UTF-8 字节 → 报可读错误，不 panic
- **B-02** 1000 条声明 × 200 个暂存文件 → `touch-check` 本机 < 200ms、CI < 2s（见验收门槛）
- **B-03** 两份未归档清单并行 → 并集口径下互不误报
- **B-04** 暂存文件数为 0（空提交）→ 放行并在 stderr 说明"未校验到变更"
- **B-05** 非 git 仓库执行 `install` → 跳过 pre-commit 注入并给出 note
- **B-06** 非 git 仓库但显式设置 `HOOK_STAGED_FILES` → 依据环境变量判定，不调 git、不报错

### 回归范围与影响面

- 全量：`cargo test --workspace`、`python scripts/verify_gate.py`（用例/场景数以实跑为准）
- 受影响：`core/src/requirement.rs`（`section_span` 抽取 + `review` 挂钩 + `review` 标记匹配收紧 + 模板）、
  `core/src/gate.rs`（`gate_lines` 收紧与扩展 / `install` / `PRE_COMMIT_BLOCK` / `verify_install` / yaml）、
  `core/src/idcheck.rs`、`cli/src/cli.rs`、`cli/src/main.rs`
- 不受影响：`core/src/status.rs` 与 TUI/GUI 渲染、`core/Cargo.toml`
- 需同步镜像：`HOOK_SH` / `HOOK_PS1` 的 `grep 'GATE:STEP'` 建议加 `^` 锚点（P2 可选，
  当前靠"标记行在文件头、`head -1` 先命中"侥幸成立）

### 验收门槛（全部可机械判定）

- **枚举覆盖**：`AcIssueKind` 与 `TouchIssueKind` 每个变体至少被一个单测命中（U-18）。
  **不设行覆盖率门槛** —— 本仓库无 tarpaulin / llvm-cov / grcov / kcov，CI 亦无覆盖率步骤，
  写"行覆盖 ≥ 90%"就是一条不可判定的门槛，与本需求立项理由矛盾。
- **性能**：B-02 的数字门槛可实测（计时断言，不引 benchmark 框架）
- 用户输入路径零新增 `unwrap()`；解析失败一律走 `GateError::Validation`
- `cargo clippy --workspace --all-targets -- -D warnings` 零告警
- `cargo fmt --all --check` 通过
- `python scripts/verify_gate.py` 场景表按设计文档 §6.4 执行，touch 相关 7 个场景随 P4 落地

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-03_22:22:41 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-03_22:23:14 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-03_22:24:21 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | rejected | 部分AC点格式不正确
- 2026-10-03_23:15:02 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

<!-- SDD-SOURCE: REQ-021 -->
> **唯一来源**：本文件的内容已路由进 `.gates/requirements/REQ-021-sdd-----ci.md`
> （req-guard 需求清单，**唯一真相**）。本文件是 SDD 草稿源，**可评审、可提意见，
> 但不可直接据此实施**；两者冲突时**一律以清单为准**。

# Feature Specification: SDD 产物纳入门禁视野

**Feature Branch**: `021-sdd-gate-visibility`

**Created**: 2026-10-10

**Status**: Draft

**Input**: User description: "把 SDD 产物纳入门禁视野：specs/** 草稿化 + CI 兜底 + 清单 frontmatter 对齐"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 审核人不再被"看起来合规"骗过 (Priority: P1)

审核人在 PR 里看到一份需求清单与一批 SDD 产物（`spec.md` / `plan.md` / `tasks.md`），
需要确信这份 SDD 产物不会悄悄变成第二份需求基线。今天它做不到：`specs/**` 落在门禁视野之外，
既不拦、也不审、也无人回看，却能进版本库——这是"名义合规"最舒服的温床。

**Why this priority**: 门禁的全部价值在于「AI 该不该写」这件事有且只有一个裁决者。
一份能进版本库却不被任何裁决者看见的需求叙事，直接否定这个前提。

**Independent Test**: 在仓库里新增一个 `specs/<feature>/spec.md` 并 `git add`，
跑校验脚本——必须退出码 1 且把该文件精确列出。仅此一条即可独立交付价值。

**Acceptance Scenarios**:

1. **Given** 仓库已接入本特性的校验脚本，且工作区内没有任何被 git 跟踪的 `specs/**` 文件，**When** 执行该校验脚本，**Then** 退出码为 0，且输出含 1 处字面量 `SDD 路由合规`
2. **Given** 仓库内 `specs/021-xxx/spec.md` 已被 `git add` 跟踪，**When** 执行该校验脚本，**Then** 退出码为 1，且输出中含该文件的完整相对路径

---

### User Story 2 - SDD 草稿不污染工作区 (Priority: P2)

开发者用 SDD 工具生成草稿后，`git status` 不应常年挂着一串未跟踪的 `specs/` 文件——
那会让人习惯性忽略 `git status` 里的异常，也会诱使人顺手 `git add .` 把草稿提交进去。

**Why this priority**: 这是"零成本守住约定"的一档——草稿化之后，第二份基线在物理上不成立，
比任何事后校验都可靠。

**Independent Test**: 生成 SDD 草稿后执行 `git status --short`，输出中不含 `specs/` 开头的行。

**Acceptance Scenarios**:

1. **Given** `.gitignore` 含 `specs/` 规则且 `specs/021-xxx/spec.md` 存在于工作区，**When** 执行 `git status --short`，**Then** 输出中不含以 `specs/` 开头的行
2. **Given** spec-kit 已在仓库初始化并生成 `.codebuddy/commands/**`，**When** 执行 `git status --short`，**Then** 输出中不含以 `.codebuddy/` 开头的行

---

### User Story 3 - 规格时效这条链不断 (Priority: P3)

清单 frontmatter 声明 `review_policy = codebound` + `source_refs`，字段名与 doc-guard 的
FRS 族逐字对齐，使得 doc-guard 一旦装上即可直接接管「代码改了规格没同步」的判定。
本特性不要求 doc-guard 在场，只保证契约字段齐备。

**Why this priority**: 三方分工里 SDD 管生成、req-guard 管裁决、doc-guard 管时效。
前两环接上而第三环悬空，规格腐化仍然无人报。但它是"可被下游接管"，不是"本次生效"，故列 P3。

**Independent Test**: 执行校验脚本，被告知活跃清单中 frontmatter 缺 `review_policy`
或 `source_refs` 的清单数量与编号。

**Acceptance Scenarios**:

1. **Given** 某活跃清单 frontmatter 含 `review_policy: codebound` 且 `source_refs` 为非空数组，**When** 执行校验脚本，**Then** 该清单不被列为"契约不齐备"
2. **Given** 本机未安装 doc-guard，**When** 执行校验脚本，**Then** 退出码由前两项检查决定，不因 doc-guard 缺席而失败

---

### Edge Cases

- `specs/` 目录根本不存在（还没用过 SDD）→ 视为合规，退出码 0；**不得**报"目录缺失"
- 存在**历史遗留**的已跟踪 `specs/**` 文件 → 列出精确路径并退出 1；
  脚本**不自动删除**任何文件（删除是人的决定，脚本只给事实）
- `.gitignore` 里写 `specs/` 与 `specs/**` 两种写法 → 都接受，按"是否存在一条能匹配 `specs/` 前缀的规则"判定
- doc-guard / spec-kit 未安装 → 不影响退出码，只在输出里标注"未安装，跳过"
- 归档区 `.gates/requirements/archive/**` 里的清单 → 不参与"契约齐备"统计（它们是只读历史）

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: 系统 MUST 提供一条可执行判定，用于检出"被 git 跟踪的 `specs/**` 文件"，退出码 0/1 二值
- **FR-002**: 该判定 MUST 逐个列出违规文件的完整相对路径，**不得**只报"发现 N 处问题"（与 `install --verify` 同风格：宁可漏，不可扰）
- **FR-003**: 该判定 MUST 同时校验 `.gitignore` 是否含能匹配 `specs/` 与 `.codebuddy/` 的规则
- **FR-004**: 该判定 MUST 接入 CI（`.github/workflows/ci.yml` 与 `.cnb.yml`），失败即让流水线红
- **FR-005**: 该判定 MUST 报告活跃清单中 frontmatter 缺 `review_policy` 或 `source_refs` 的清单编号，且**不自动改写**清单
- **FR-006**: `docs/规范/SDD产物接入规范.md` §7 MUST 从"可选未启用"回填为"已启用"，并记录选型理由（为什么选草稿化而非 GATE:TOUCH 声明）
- **FR-007**: 脚本 MUST 零第三方依赖（纯 Python 标准库），与 `scripts/verify_gate.py` 同风格
- **FR-008**: 脚本 MUST NOT 修改任何文件——它是只读体检，与界面「体检」四组同性质

### Key Entities

- **SDD 产物**：`specs/**` 下由 spec-kit 生成的草稿（spec / plan / tasks / research / data-model / contracts）。语义等价于 `.gates/drafts/`，**不入库**
- **SDD 工具资产**：`.specify/**`（模板与脚本）与 `.codebuddy/commands/**`（斜杠命令）。可再生性弱，**入库**
- **契约字段**：清单 frontmatter 的 `review_policy` 与 `source_refs`。后者由 `GATE:TOUCH` 单向派生，**不可手改**

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 新增 1 个 `specs/**` 文件并 `git add` → 校验退出码 1 且输出含该路径（现状：无此判定）
- **SC-002**: 无 `specs/**` 跟踪文件时 → 退出码 0（现状：无此判定）
- **SC-003**: `req-guard install --verify` 在本特性落地后仍 PASS，不引入新的失败项
- **SC-004**: 脚本在纯 Python 3 标准库下可直接执行，零 import 第三方包
- **SC-005**: CI 两侧（GitHub Actions / CNB）各新增 1 步，且步骤失败能让流水线退出非 0

## Assumptions

- 本仓库已装 req-guard 0.1.6 且 `auth.level = 3`，故任何"批准"动作必须由人类在真实终端完成；本特性不试图改变这一点
- doc-guard 与 sql-guard 本机**未安装**，本特性只做契约对齐，不做实际 FRS 判定——这是已知未验证项，须在交付时明示
- spec-kit 1.1.4.dev0 已装入隔离 venv（`/Users/mikezhu/.workbuddy/binaries/python/envs/speckit`），不入全局环境
- 本特性**不**改动 `core/src/**`：门禁判定逻辑一行不改，只在仓库约定与 CI 侧收口（也因此不引入 `critical` 档的额外审批负担）
- 「想让 `specs/**` 进门禁视野」的第二条路（在 `GATE:TOUCH` 里声明 `specs/**`）本次**不采用**，理由见 plan.md 的选型比较

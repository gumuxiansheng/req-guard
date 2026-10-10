<!-- SDD-SOURCE: REQ-021 -->
> **唯一来源**：本文件的内容已路由进 `.gates/requirements/REQ-021-sdd-----ci.md`
> （req-guard 需求清单，**唯一真相**）。本文件是 SDD 草稿源，**可评审、可提意见，
> 但不可直接据此实施**；两者冲突时**一律以清单为准**。

# Tasks: SDD 产物纳入门禁视野

**Input**: Design documents from `/specs/021-sdd-gate-visibility/`
**Prerequisites**: spec.md（已定稿）、plan.md（已定稿）

## Format: `[ID] [P?] [Story] Description`

- **[P]**：可与其它 [P] 任务并行（不同文件、无依赖）

## Phase 1: Setup

- [ ] T001 [P] 在 `.gitignore` 追加 `specs/` 与 `.codebuddy/` 两行，并各带一行理由注释
- [ ] T002 [P] 新建 `scripts/verify_sdd_routing.py`：A 组（已跟踪的 SDD 产物）+ B 组（忽略规则在位）
- [ ] T003 在 `scripts/verify_sdd_routing.py` 补 C 组（`codebound` 契约齐备，只报告不改退出码）

**Checkpoint**：脚本可独立执行，无 `specs/` 跟踪文件时退出码 0

## Phase 2: User Story 1 - 审核人不再被"看起来合规"骗过（P1）🎯 MVP

**Goal**：`specs/**` 一旦被 git 跟踪，校验脚本立刻红并列出精确路径

**Independent Test**：`specs/x/spec.md` 存在且未被跟踪 → 退出码 0；`git add -f` 后 → 退出码 1 且列出该路径

- [ ] T004 [US1] 用 `git ls-files -- specs/` 实现 A 组判定，逐条打印路径
- [ ] T005 [US1] A 组非空时退出码 1，并输出「这些文件已入库，请删除或加入 .gitignore」
- [ ] T006 [US1] 自检：临时 `git add -f` 一个 `specs/` 文件再撤销，确认脚本由绿转红再转绿

**Checkpoint**：US1 可独立验收

## Phase 3: User Story 2 - SDD 草稿不污染工作区（P2）

**Goal**：`git status --short` 中不再出现 `specs/` 与 `.codebuddy/`

**Independent Test**：生成 SDD 草稿后 `git status --short`，输出不含这两类前缀的行

- [ ] T007 [US2] B 组判定：解析 `.gitignore`，检查是否存在匹配 `specs/` 与 `.codebuddy/` 的规则
- [ ] T008 [US2] 对 `.specify/**` **不**做忽略（它要入库），并在脚本注释里写明这个区分
- [ ] T009 [P] [US2] 核对 `.specify/` 体积，确认入库可接受

**Checkpoint**：US1 + US2 均独立可用

## Phase 4: User Story 3 - 规格时效这条链不断（P3）

**Goal**：报出 frontmatter 缺 `review_policy` / `source_refs` 的活跃清单编号

**Independent Test**：脚本输出的「契约不齐备」清单编号与实际一致

- [ ] T010 [US3] 扫描 `.gates/requirements/*.md`（排除 `archive/`），解析 YAML frontmatter
- [ ] T011 [US3] 缺字段的清单只列入报告，**不改变退出码**；输出写明「doc-guard 装上后此项为 FRS004 前置条件」
- [ ] T012 [US3] doc-guard / spec-kit 未安装时输出标注「未安装，跳过」，不失败

**Checkpoint**：三个 US 全部独立可用

## Phase 5: CI 接入与文档回填

- [ ] T013 `.github/workflows/ci.yml` 新增一步：`python scripts/verify_sdd_routing.py`
- [ ] T014 `.cnb.yml` 新增同一步
- [ ] T015 [P] `docs/规范/SDD产物接入规范.md` §7 回填：由「可选未启用」改为「已启用」，记录选型理由（草稿化 vs GATE:TOUCH 声明）
- [ ] T016 [P] `docs/README.md` 索引同步（新增脚本进文档一览）
- [ ] T017 回归：`python scripts/verify_gate.py` / `req-guard install --verify` / `req-guard ac check` / `cargo test --workspace`

## Dependencies & Execution Order

- T001 → T007（B 组判定依赖 `.gitignore` 真的改了）
- T002 → T003 → T004/T005（C 组并入同一脚本）
- T004/T005/T007 完成 → T013/T014（CI 步骤要等脚本可跑）
- T015/T016 可与 T013/T014 并行

## 实施策略

1. T001–T003（脚本可用）→ 跑一次确认退出码 0
2. T004–T006（US1 闭环）→ **停下来验证**：`git add -f` 一个草稿，确认真会红
3. T007–T009（US2 闭环）
4. T010–T012（US3 体检项）
5. T013–T017（接入 + 文档 + 回归）

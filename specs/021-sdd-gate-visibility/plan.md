<!-- SDD-SOURCE: REQ-021 -->
> **唯一来源**：本文件的内容已路由进 `.gates/requirements/REQ-021-sdd-----ci.md`
> （req-guard 需求清单，**唯一真相**）。本文件是 SDD 草稿源，**可评审、可提意见，
> 但不可直接据此实施**；两者冲突时**一律以清单为准**。

# Implementation Plan: SDD 产物纳入门禁视野

**Branch**: `021-sdd-gate-visibility` | **Date**: 2026-10-10 | **Spec**: [spec.md](./spec.md)

## Summary

让 SDD 产物（`specs/**`）从「门禁看不见、却能进版本库」的灰色地带里出来。做法是**物理排除 +
机械校验**：`specs/**` 与 spec-kit 注入的 `.codebuddy/**` 进 `.gitignore`（草稿化，不入库），
再用一条零依赖、只读的校验脚本守住这个约定并接入 CI 两侧。清单 frontmatter 的 `codebound`
契约字段由同一条脚本体检，使 doc-guard 装上即可接管时效判定。

**本方案不改动 `core/src/**`** —— 门禁判定逻辑一行不改。

## Technical Context

| 项 | 现状 |
| --- | --- |
| 门禁 | req-guard 0.1.6，`auth.level = 3`（审批须人类凭据，AI 不能自批） |
| SDD | spec-kit 1.1.4.dev0，装在隔离 venv `/Users/mikezhu/.workbuddy/binaries/python/envs/speckit` |
| doc-guard | **本机未安装**；本次只做契约对齐，不跑 FRS |
| 语言/依赖 | 脚本为 Python 3 标准库，零第三方依赖（与 `scripts/verify_gate.py` 同风格） |
| 测试 | `scripts/verify_gate.py` / `verify_tier.py` 同风格的脚本自检 |
| 目标平台 | Linux / macOS / Windows（CI 三侧），脚本仅依赖 `git` 与 Python 3 |

## 选型：为什么是「草稿化」而不是「`GATE:TOUCH` 声明 `specs/**`」

`docs/规范/SDD产物接入规范.md` §6 给了两条路。本方案选第一条，理由逐条可复核：

| 判据 | 草稿化（本方案） | `GATE:TOUCH` 声明 `specs/**` |
| --- | --- | --- |
| 第二份基线是否还可能成立 | **物理上不可能**（不入库） | 仍然可能——声明只是让门禁「看得见」，裁决结果是「已批准 → 可写」，副本照样入库 |
| R1（一份需求一份清单）靠什么守 | 机器 | 人。门禁无法判定两份文档是不是在讲同一件事 |
| 是否需要人记得跑 | 否。`git status` 天然干净，`git add .` 也带不进去 | 是。`specs/` 入库后要靠人 review 才发现 |
| 代价 | SDD 产物本身不可在 PR 里评审 | 产物可评审，但与清单并行存在 |

代价的缓解：SDD 的内容经写入清单后才进版本库，**评审对象是清单**（唯一真相），
草稿本身按 R3 本就「apply 后失去独立意义」。

一句话：**「纳入门禁视野」不等于「让它在版本库里被看见」——排除掉它，才是让它不再构成威胁。**

## 设计

### 设计 1：脚本是只读体检，不是修复器

`scripts/verify_sdd_routing.py` 一律不改文件。发现已跟踪的 `specs/**` 时**只列出路径并退出 1**，
不自动 `git rm`、不自动删除。理由与界面「体检」四组一致：**删除是人的决定**，脚本替人删文件
会把一次「提醒」升级成一次「破坏」，且事后无法区分「脚本删的」与「人删的」。

### 设计 2：三组检查，各自防一种回潮

| 组 | 判定 | 防的是什么 | 退出码 |
| --- | --- | --- | --- |
| **A** 已跟踪的 SDD 产物 | `git ls-files -- specs/` 与 `-- .codebuddy/` 均为空 | 有人用 `git add -f` 绕过 `.gitignore` 把草稿提交进去 | 非空 → 1 |
| **B** 忽略规则在位 | `.gitignore` 含能匹配 `specs/` 与 `.codebuddy/` 的规则 | 规则被人删掉（此时 A 组会立刻变红，B 提供「先倒哪一块」的定位信息） | 缺 → 1 |
| **C** `codebound` 契约齐备 | 活跃清单（不含 `archive/`）frontmatter 含 `review_policy` 且 `source_refs` 非空 | 规格时效这条链断在最后一环：doc-guard 装上却接管不了 | 缺 → **只报告，不改变退出码** |

**C 组刻意不改变退出码**：`source_refs` 由 `GATE:TOUCH` 在 `approve` 时**单向派生**
（`requirement::ensure_touch_declared`），存量已批准清单不会自动补写。若 C 组判红，
等于让 20 份存量清单一夜之间把 CI 变红——那不是门禁，是噪音。故 C 组是**体检项**，不是**拦路项**。

### 设计 3：`git ls-files` 而非 `git diff`

规范 §7 原文写的是 `git diff --name-only -- specs/`。本方案改用 `git ls-files`，因为
`git diff` 只看增量：一个**上个提交就已经入库**的 `specs/` 文件，在本次 diff 里根本不出现，
检查永远绿。要判的是「版本库里有没有」，不是「这次改没改」。

### 设计 4：`.specify/` 入库，`.codebuddy/` 不入库

| 目录 | 处理 | 理由 |
| --- | --- | --- |
| `.specify/**` | **入库** | 项目 constitution 与模板，可再生性弱；语义上等价于 `.gates/`（工具资产，随仓库提交） |
| `.codebuddy/**` | **不入库** | spec-kit 每次 `init` 重新注入；且 spec-kit 自己在 init 结束时提示「agent 目录可能含凭据，建议 gitignore」 |
| `specs/**` | **不入库** | R3：草稿源，apply 后失去独立意义 |

### 涉及的文件与模块清单

见「变更范围」节（精确到文件；唯一使用通配的是 `specs/**` 本身——它是本需求的作用对象，
不是宽口径的偷懒）。

### 兼容性、性能与安全影响

- **兼容性**：纯新增脚本 + 配置行，不改 `core/`，对既有判定零影响。
- **性能**：`git ls-files` 两次 + 若干次文件读取，毫秒级，可安全放进每次 CI。
- **安全**：脚本只读，无网络、无写文件、无 subprocess 拼接用户输入（路径全部来自 git 与固定清单）。
- **必须点名的残留风险**：`.gitignore` 本身在 `touch.exempt` 豁免区内，AI 可自行删掉那两行
  而不被变更范围契约发现。真正的兜底是 B 组检查 + CI —— 但 CI 也是在**规则被删之后**才红，
  属事后发现。若要事前拦，需把 `.gitignore` 移出豁免区，那会让 `req-guard install` 每次
  给自己下判（见 `gate.rs::touch_exempt_patterns` 注释），故本次不动。

### 风险点与回滚方案

| 风险 | 严重级 | 缓解 |
| --- | --- | --- |
| 团队确实需要 `specs/` 入库做评审 | 中 | 本需求是**约定落地**，不是能力封闭：删掉 `.gitignore` 两行即回到原状，脚本会明确报出来 |
| `.specify/` 体积随版本膨胀 | 低 | 入库前核对体积；必要时改为只入库 `.specify/memory/constitution.md` |
| C 组报告被长期无视 | 中 | 输出写明「doc-guard 装上后此项即为 FRS004 的前置条件」，并写入规范 §9 验收清单 |

**回滚**：删掉脚本、还原 `.gitignore` 两行、撤掉 CI 两步 —— 三处独立可逆，
无状态迁移、无数据变更。

## 变更范围（将写入 `GATE:TOUCH`）

```
.gitignore
scripts/verify_sdd_routing.py
.github/workflows/ci.yml
.cnb.yml
docs/规范/SDD产物接入规范.md
docs/README.md
specs/**
```

## 实施顺序

1. `.gitignore` 追加 `specs/` 与 `.codebuddy/`（含理由注释）
2. 新增 `scripts/verify_sdd_routing.py`（三组检查，只读）
3. `.github/workflows/ci.yml` 与 `.cnb.yml` 各加一步
4. `docs/规范/SDD产物接入规范.md` §7 回填：从「可选未启用」改为「已启用」并记录选型理由
5. `docs/README.md` 索引同步
6. 回归：`verify_gate.py` / `install --verify` / `ac check` / `cargo test --workspace`

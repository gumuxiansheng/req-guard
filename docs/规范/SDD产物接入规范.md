# SDD 规格工具接入规范（单源路由）

> 定稿 2026-10-05。适用：仓库内使用 **SDD（spec-driven development，spec-kit 类）** 生成需求/设计/任务
> 规格，同时挂了 req-guard 流程门禁的场景。目标只有一个：**不让"规格"出现两份**。

---

## 1 为什么要有这份规范

SDD 工具为每个特性产出 `spec.md` / `plan.md` / `design.md` / `research.md` / `data-model.md` /
`tasks.md` / `contracts/*` 等 6–9 个文件，外加一份 `constitution.md`；req-guard 只认**一份**
`.gates/requirements/REQ-*.md`（三段：需求分解 / 技术方案 / 测试计划）。两套产物并存时，
同一个意图会出现两处叙事——谁改了哪一份、哪一份是准的，全靠人记。这就是"脑裂"。

**关键事实：门禁对 SDD 产物是"看不见"的。** 证据（均为文件名 + 符号名，不写行号）：

| 落盘位置 | 门禁行为 |
| --- | --- |
| `.gates/requirements/*.md` | `gate::pretool`（`pretool` 判定）显式放行正文写入（`PretoolVerdict::AllowDoc`），但 `GATE:STEP` 状态行逐行锁死（防 AI 自批） |
| `*.comments.md` | 硬拦（AI 不得改证据），必须走 `req-guard comment … --reply` |
| `specs/**` 等其它路径 | `pretool` 返回 `Continue`；`req-guard check` 按范围判定，而 `resolve.rs` 的 G4 约定是「**未声明的路径不额外拦**」 |

→ 结论：**SDD 产物落在 `specs/` 且未被第二段声明时，既不拦、也不审、也无人回看**。它不是
"门禁外的另一份基线"，而是一份完全没有基线的自由文本。所以路由不是风格问题，是缺口问题。

---

## 2 三方边界（各做什么、各不做什么）

| 层 | 负责 | 不负责 |
| --- | --- | --- |
| **SDD 工具** | 生成规格内容；跨文件一致性**建议**（`/analyze` 只读，不拦截任何人） | 批准、拦截、范围校验、时效校验 |
| **req-guard** | 流程判定 + fail-closed 硬拦截（PreToolUse / pre-commit / CI）；逐段批准 + 批准即冻结（`GATE:STEP` 的 `sum=`）；方案↔设计文档交叉引用（`touch::cross_refs`）；改动 ⊆ `GATE:TOUCH`；审计台账 | 文档写得对不对（语义判断不做） |
| **doc-guard** | 规格时效：代码改了规格没同步（FRS001/003/004/005/007、DRF001），直接接管 req-guard 清单 frontmatter | 流程顺序、审批 |

**功能不重复**：唯一边缘重叠是 SDD 的 `/analyze` ≈ `req-guard ac check` + 交叉引用校验——
但前者是建议、后者是**拒绝理由**，语义不同，不构成重复投入。

---

## 3 三条硬约束（唯一真相）

### R1 一份需求 = 一份清单

唯一真相 = `.gates/requirements/REQ-<id>.md`。任何别处的"需求描述"都必须是**指向它的引用**，
不是它的副本。

### R2 零复制，只引用

- 细粒度规格（`design/research/data-model/contracts/quickstart`）**落 `docs/设计/`**，由第 2 段
  以 `docs/<目录>/<文件>.md §<数字>(.<数字>)*` 引用，**不在方案段里再抄一遍架构**。
  这条不是约定，是强制：`touch::cross_refs` 只认这一种写法，路径不存在或小节号不存在 →
  `ensure_cross_refs_ok` 直接**拒绝批准技术方案**。
- 文件范围**只声明一次**：第 2 段的 `<!-- GATE:TOUCH -->` 是唯一声明点，`frontmatter` 的
  `source_refs` 由它**单向派生**（`requirement::ensure_touch_declared`），不要求也不允许人工维护第二份。
  SDD 的 `plan.md` 里若再出现一份"涉及文件清单"，就是第二份 —— 按 R3 处理。

### R3 SDD 产物只当草稿源，apply 后不留副本

`specs/<feature>/` 的整个目录只作为**草稿源**存在（语义等价于 `.gates/drafts/<需求ID>.draft.md`，
该目录按设计不参与任何判定），内容经 `req-guard apply --step <段名>` 进入清单后即失去独立意义：

- 首选：**apply 后删除 `specs/<feature>/`**（推荐，零副本）；
- 次选：`specs/**` 进 `.gitignore`（保留本地草稿，不入库）；
- **禁止**：把 `specs/` 内容长期与清单并行维护，或让 AI 继续往里写。

---

## 4 产物映射表

| SDD 产物 | 去向 | 备注 |
| --- | --- | --- |
| `spec.md` | → 第 1 段 `decomposition` 需求分解 | FR 编号可保留；**验收口径**在第 3 段落成 `GATE:AC`，不靠 spec.md 兜 |
| `plan.md` | → 第 2 段 `solution` 技术方案 | 必须含 `<!-- GATE:TOUCH -->` 声明块；架构细节不抄进方案段 |
| `design.md` / `research.md` / `data-model.md` / `contracts/*` / `quickstart.md` | → `docs/设计/` | 落 `docs/` 之外 = 方案段引用不了 = 批准不了 |
| `tasks.md` | **不入库** | 语义与第 3 段不同（实施任务 ≠ 测试计划）。只挑「可观测」条目改写为 AC |
| `constitution.md` | 流程条款 → `docs/规范/`；其余留原处 | 避免出现两份"项目准则" |
| `specs/<feature>/` 目录 | 草稿源（R3） | 与清单并存即违反 R1 |

---

## 5 执行顺序（命令序列）

```bash
# 0) 起需求：唯一真相
req-guard create --id REQ-0xx --title "<需求标题>"      # --id 建议先过 req-guard ids 查重

# 1) SDD /specify → spec.md 草稿 → 第 1 段
req-guard apply REQ-0xx --step decomposition
req-guard approve REQ-0xx --step decomposition --reviewer <姓名>

# 2) SDD /plan → 细粒度文档落 docs/设计/ → 第 2 段（cross-ref + GATE:TOUCH 在此校验）
#    design/research/data-model/contracts → docs/设计/xxx.md
req-guard apply REQ-0xx --step solution
req-guard approve REQ-0xx --step solution --reviewer <姓名>

# 3) SDD /tasks → 挑可观测条目改写成 AC → 第 3 段（ac::lint 硬拦）
req-guard apply REQ-0xx --step testplan
req-guard ac check REQ-0xx
req-guard approve REQ-0xx --step testplan --reviewer <姓名>

# 4) 实施阶段
req-guard touch-check --base origin/<分支>      # 实际改动 ⊆ GATE:TOUCH
# 5) 收尾
req-guard seal REQ-0xx ; req-guard done REQ-0xx   # 满 30 天（archive.after_days）自动归档
```

`approve` 必须带 `--step` 与 `--reviewer`（严格模式下还须凭据 / 终端挑战码——`auth.level ≥ 1`）。

---

## 6 门禁配置里的落盘约束（`.gates/req-guard.yaml`）

| 配置 | 与 SDD 接入的关系 |
| --- | --- |
| `steps` / `strict_order` | 顺序即默认强制审核顺序；SDD 的阶段顺序须与三段对齐（§5） |
| `enforce.{ai_tool_hook,pre_commit,ci}` | SDD 生成的文档同样被拦（写在未声明路径上不额外拦、写在已声明且未批范围内则拦） |
| `touch.exempt` | **逐项具名**，不用 `.gates/**` 通配；`touch.exempt` 是**追加**到默认集，不是替换。`.gates/req-guard.yaml` 与 `.gates/audit/*` **刻意不在豁免内**——它们是门禁配置与审批轨迹，豁免等于允许 AI 自己把门禁调松，改它们须在第二段声明 |
| `archive.after_days` | 清单完成后的归档节奏（默认 30） |

**想让 `specs/**` 也进门禁视野**：在第二段 `GATE:TOUCH` 里声明 `specs/**`，它即被纳入范围判定
（未批准不得改写；批准后可写，且后续改动受 `touch-check` 覆盖）。这是目前唯一不写新规则就能
"看见 SDD 草稿"的办法。

---

## 7 可选机械兜底

约定若不可机械判定，就会退化成人人记得跑才能跑的流程。可选两档：

1. **最轻**：`specs/**` 进 `.gitignore`（草稿化），`git status` 里根本不出现副本。
2. **CI 兜底**：流水线加一步——`git diff --name-only -- specs/` 非空且对应清单第二段未批准 → 退出 1。
   与 `install --verify` 同风格：**宁可漏，不可扰**，只报出精确文件，不做模糊断言。

（两档均可选；本规范定稿时未启用，启用即视为对 `docs/规范/AI工具合规保证规范.md` §4 验收清单的补充。）

---

## 8 不适用的情形

以下可直接走 bypass 或不走 SDD，不必套本规范：热修（问题与解法都显然）、纯重构（行为不变）、
原型 / 一次性脚本、 exploratory 需求探索期。判据：**这次改动会不会改变需求基线**——会，就走 §5；
不会，就不必生成 SDD 产物。

---

## 9 验收清单（改动本规范后必须回归）

- [ ] 仓库内不存在与清单并存的 SDD 需求副本（搜 `specs/` 下的 `spec.md`，每条都能对应到 `.gates/requirements/*.md`，或该目录已不在版本库内）
- [ ] 第 2 段存在 `<!-- GATE:TOUCH -->`，条目归一有效（不含 `..`、不以 `/` 开头）
- [ ] 方案段引用的文档全部在 `docs/` 下、小节号存在（`req-guard approve --step solution` 通过即证明）
- [ ] 第 3 段 AC 编号连续且 Given/When/Then 齐备（`req-guard ac check` 绿）
- [ ] 不存在第二份文件清单：plan 侧的文件清单已并入 `GATE:TOUCH`
- [ ] `constitution.md` 中与流程相关的条款已并入 `docs/规范/`
- [ ] 索引同步：`docs/README.md`（目录结构 + 文档一览 + 阅读顺序）
- [ ] 门禁回归：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `python scripts/verify_gate.py`

---

## 10 命名澄清（易混）

README「规格绑定代码（**SDD 契约**）」条目里的 SDD **不是** SDD 工具，而是指清单 frontmatter
`review_policy = codebound` + `source_refs` 与 doc-guard FRS 族**逐字对齐**的约定（解析见
`core/src/specmeta.rs`，字段语义与 doc-guard 同源）。它保证的是「代码改了规格会被报出来」，
与本规范（SDD 产物往哪儿落）是两件事，但**同一条链**：本规范管落盘位置，该约定管时效性。

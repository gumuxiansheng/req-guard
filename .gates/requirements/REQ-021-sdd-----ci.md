---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-10
source_refs: [.gitignore, scripts, .github/workflows, .cnb.yml, docs/规范, docs, specs, .specify]
---

# REQ-021 SDD 产物纳入门禁视野：来源标注 + 路由校验 + CI 兜底

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。
>
> **本清单由 SDD 链路产出**：spec-kit 1.1.4.dev0 生成 `specs/021-sdd-gate-visibility/` 下的
> `spec.md` / `plan.md` / `tasks.md`，按 `docs/规范/SDD产物接入规范.md` §4 映射表路由进本清单三段
> （spec → 第 1 段、plan → 第 2 段、tasks 中可观测条目 → 第 3 段 AC）。
>
> **本清单是唯一真相**：SDD 产物**保留入库**以供团队评审，但每个文件顶部必须带
> `<!-- SDD-SOURCE: REQ-021 -->` 声明指向本清单；冲突时一律以本清单为准。

<!-- GATE:HEAD id=REQ-021 status=done created=2026-10-10_13:14:13 done=2026-10-10_18:52:06 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-10_17:20:57 sum=495872de9f999511dd6b79c234e680126d6bce8914d5c405a8f356b72dfeb78c -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-10_17:24:07 sum=23941e0067f35c2a6c57fc222e3e604eee51b07279ea62f94c965e146c755af1 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-10_17:24:18 sum=f15691880de34dcc84a73fffed74e61c30efe694182a465deb7650471c47ff75 -->

## 1. 需求分解

### 背景与问题

spec-kit 已在仓库初始化（`.specify/` + `.codebuddy/commands/`），SDD 产物落在
`specs/<feature>/`。而门禁对这条路是**看不见**的，两条证据（见 `docs/规范/SDD产物接入规范.md` §1）：

| 落盘位置 | 门禁行为 |
| --- | --- |
| `.gates/requirements/*.md` | `gate::pretool` 显式放行正文写入，但 `GATE:STEP` 状态行逐行锁死 |
| `*.comments.md` | 硬拦，必须走 `req-guard comment … --reply` |
| `specs/**` 等其它路径 | `pretool` 返回 `Continue`；`resolve` 的 G4 约定是「未声明的路径不额外拦」 |

于是 SDD 产物处于一个最糟的位置：**既不拦、也不审、也无人回看，却能进版本库**。
它不是「门禁外的另一份基线」，而是一份**完全没有基线的自由文本**——
团队享受了「我们用了 SDD」的名义合规，实际没有任何一步被裁决过。

三条实测复现（本仓库当前状态，可重跑）：

| # | 复现 | 结果 |
| --- | --- | --- |
| **1** | 在 `specs/021-sdd-gate-visibility/` 下写入 `spec.md` / `plan.md` / `tasks.md` | 三个文件均正常落盘，无拦截，无审计记录 |
| **2** | `git status --short` | 出现 `?? specs/`，可被 `git add .` 一并提交 |
| **3** | `req-guard check` | 输出「本次改动：（无受管路径）」——`specs/**` 根本不在视野里 |

### 目标

- **G1** `specs/**` **保留入库**（团队要在 PR 里评审 SDD 产物），但每个被 git 跟踪的 SDD 产物文件
  必须带机器可解析的来源声明 `<!-- SDD-SOURCE: REQ-<id> -->`，且该清单真实存在。
  缺失、位置不合规或指向不存在的清单 → 校验退出码 1 并列出精确路径。
- **G2** 上述判定**机械化并接入 CI**，不靠人记得跑。
- **G3** 区分三类目录的入库策略：`specs/**`（**入库**，带来源声明）、`.specify/**`（入库，工具资产）、
  `.codebuddy/**`（**不入库**，可再生 + 可能含凭据，spec-kit 自己也是这么建议的）。
- **G4** 清单 frontmatter 的 `codebound` 契约字段（`review_policy` + `source_refs`）齐备，
  使 doc-guard 装上即可接管「代码改了规格没同步」的判定；**本次不要求 doc-guard 在场**。
- **G5** 修订 `docs/规范/SDD产物接入规范.md` 的 **R3**：由「apply 后不留副本」
  改为「可留副本，但副本必须机械指向唯一真相」，并回填 §7 的启用状态与选型理由。

### 关键取舍：为什么「并行存在」不等于「脑裂」

现行 R3 禁止把 `specs/` 与清单**并行维护**。本需求认为该禁令的真正对象被写窄了：

> **R1 的实质是「唯一真相只有一个」，不是「同一件事只能被写一次」。**
> 脑裂的定义是「**不知道**哪份是准的」，而不是「存在两份文档」。

只要每个副本都**机械地**声明了它从属于哪份清单、且该清单真实存在，「哪份是准的」这个问题
就永远有唯一答案（声明里写着）。因此本需求把禁令改写为：
**禁止的是「无来源声明的并行维护」，不是「并行存在」本身。**

这个取舍换来的代价必须写明（见 §3 残留风险）：**内容漂移无法机械判定** ——
清单改了而 `specs/` 没改，脚本查不出来。守住的是「指向」，不是「一致」。

### 非目标

- **N1** **不改 `core/src/**`**：门禁判定逻辑一行不动。本需求只在仓库约定与 CI 侧收口。
- **N2** 不做内容一致性比对（不 diff `specs/` 与清单正文）—— 那需要语义判断，门禁不做（N5）。
- **N3** 不自动修复：脚本只报告，不删除文件、不改 `.gitignore`、不改清单、不补写声明。
- **N4** 不引入任何**运行时依赖**：脚本只依赖 `git` 与 POSIX `sh`（理由见 §2 设计 6）；不改动 `Cargo.toml`。
- **N5** 不校验 SDD 产物**内容**写得对不对（那是审核人的事，门禁不做语义判断）。

### 子任务拆解（编号 + 预估工时）

| 编号 | 任务 | 预估 |
| --- | --- | --- |
| **T1** | 为 `specs/021-sdd-gate-visibility/` 三个产物加来源声明块 | 0.2h |
| **T2** | 新增 `scripts/verify_sdd_routing.sh`：A 组 + B 组判定 | 1.0h |
| **T3** | 同脚本补 C 组（`codebound` 契约体检，只报告不改退出码） | 0.5h |
| **T4** | `.gitignore` 追加 `.codebuddy/`（仅此一项） | 0.1h |
| **T5** | `.github/workflows/ci.yml` 与 `.cnb.yml` 各加一步 | 0.4h |
| **T6** | 规范回填：R3 改写 + §7 启用状态 + `docs/README.md` 索引同步 | 0.6h |
| **T7** | 回归：`verify_gate.py` / `install --verify` / `ac check` / `cargo test --workspace` | 0.5h |

合计约 3.3h。

### 影响范围

| 层 | 位置 | 影响 |
| --- | --- | --- |
| 约定 | `specs/**` | **保留入库**，每个文件强制来源声明 |
| 约定 | `.gitignore` | 仅新增 `.codebuddy/` 一行 |
| 校验 | `scripts/verify_sdd_routing.sh` | **新增**，只读，零依赖 |
| CI | `.github/workflows/ci.yml`、`.cnb.yml` | 各新增 1 步 |
| 文档 | `docs/规范/SDD产物接入规范.md`、`docs/README.md` | R3 改写 + §7 回填 + 索引同步 |
| 判定 | `core/src/**` | **不改**（N1） |
| 前端 | `cli` / `tui` / `gui` | **不改** |

### 验收标准（可度量、可判定）

- 全部 `specs/**` 文件带合规声明时，校验脚本退出码 0；
- 任一个 `specs/**` 文件缺声明或指向不存在的清单 → 退出码 1 且输出含该路径；
- `git status --short` 中不含以 `.codebuddy/` 开头的行；
- `req-guard install --verify` 仍 PASS，不引入新的失败项；
- 脚本零第三方 import，且执行前后工作区 `git status` 逐字一致（只读证明）。

## 2. 技术方案

### 总体思路

一句话：**SDD 产物照常入库供团队评审，但每个文件都必须用一行机器可解析的注释声明
「我的唯一真相是 REQ-xxx」；CI 守住这个声明，让「哪份是准的」永远有唯一答案。**

### 选型：三条路的比较

| 判据 | A 草稿化（gitignore） | B 裸入库（现状） | **C 入库 + 强制来源标注（本方案）** |
| --- | --- | --- | --- |
| 团队能否在 PR 里评审 SDD 产物 | **否** | 能 | **能** |
| 第二份基线是否可能成立 | 物理上不可能 | **可能**（正是现状问题） | 不可能——每个副本都机械指向唯一真相 |
| R1 靠什么守 | 机器 | 人 | **机器**（声明 + CI） |
| 内容漂移能否机械判定 | 不适用（没有副本） | 不能 | **不能**（见残留风险 1） |

选 C 的判据：在「能评审」与「有唯一真相」之间，C 是唯一同时满足两者的；
它付出的代价（漂移查不出）在 A 方案里同样存在（A 根本没有副本可比），属于**不新增**的代价。

### 关键设计

#### 设计 1：脚本是只读体检，不是修复器

发现违规时**只列出路径并退出 1**，不自动补写声明、不删除文件、不改 `.gitignore`。
与界面「体检」四组同性质：**修改是人的决定**；脚本替人改文件会把一次「提醒」升级成一次
「破坏」，且事后无法区分「脚本改的」与「人改的」。

#### 设计 2：三组检查

| 组 | 判定 | 防的是什么 | 退出码 |
| --- | --- | --- | --- |
| **A** 来源声明在位 | 每个被 git 跟踪的 `specs/**` 文件，其**前 15 行内**含 `<!-- SDD-SOURCE: REQ-<3位数字> -->` | 有人新增 SDD 产物却没声明归属 → 又一份「没有基线的自由文本」 | 缺 → 1 |
| **B** 指向的清单存在 | 声明里的 `REQ-<id>` 能在 `.gates/requirements/`（含 `archive/`）解析到清单文件 | 声明指向一个不存在的清单（复制粘贴 / 手误 / 清单已归档删除） | 否 → 1 |
| **C** `codebound` 契约齐备 | 活跃清单（不含 `archive/`）frontmatter 含 `review_policy` 且 `source_refs` 非空 | 规格时效这条链断在最后一环：doc-guard 装上却接管不了 | 缺 → **只报告，不改变退出码** |

**A 组为什么限定「前 15 行内」**：声明若允许写在文件任意位置，它就会被人塞到文件末尾而失去
「打开就看见」的作用，机械判定也退化成「存在即可」。15 行足以容纳标题 + 一段说明性引用，
不足以把声明藏起来。

**B 组为什么把 `archive/` 也算存在**：归档区是只读历史，`req-guard status --archived` 仍可查。
一个需求归档后，它的 SDD 产物仍可保留在库里作为历史材料——把归档区排除掉会逼人删文件。

**C 组刻意不改变退出码**：`source_refs` 由 `GATE:TOUCH` 在 `approve` 时单向派生
（`requirement::ensure_touch_declared`），存量已批准清单不会自动补写。若 C 组判红，
等于让 20 份存量清单一夜之间把 CI 变红——那不是门禁，是噪音。

#### 设计 3：`git ls-files` 而非 `git diff`

要判的是「版本库里有没有」，不是「这次改没改」。`git diff` 只看增量：
一个**上个提交就已存在**的违规文件在本次 diff 里根本不出现，检查永远绿。

#### 设计 4：三类目录的入库策略

| 目录 | 处理 | 理由 |
| --- | --- | --- |
| `specs/**` | **入库 + 强制来源声明** | 团队评审对象；声明把它锚定到唯一真相 |
| `.specify/**` | **入库** | 项目 constitution 与模板，可再生性弱；语义上等价于 `.gates/`（工具资产，随仓库提交） |
| `.codebuddy/**` | **不入库** | spec-kit 每次 `init` 重新注入；且 spec-kit 在 init 结束时自行提示「agent 目录可能含凭据，建议 gitignore」 |

#### 设计 5：`specs/**` 必须写进 `GATE:TOUCH`（与初版相反，理由如下）

初版方案刻意不声明，实测后**推翻**。两条实测（同一批改动、两个钩子）：

| 实测 | 命令 | 结果 |
| --- | --- | --- |
| 暂存一个未被任何清单声明的 `specs/**` 文件 | `req-guard check --staged` | 退出码 1，`[Ambiguous]` |
| 同一次改动 | `sh .gates/hooks/req-guard-touch-check.sh` | 退出码 0，报「✅ 变更范围合规」 |

判读：

1. **不声明时提交会被 `Ambiguous` 拦** —— 而本需求现在要求 `specs/**` **入库**（团队要评审），
   被拦就完全没法工作。故必须声明，让改动能归属到 REQ-021。
2. 声明在这里**不再是**「把不该入库的东西合法化」——入库是本需求的既定目标，
   声明只是让这批改动有归属。约束改由 A/B 两组 + CI 承担。
3. 顺带记录一个既有裂缝：同一批改动两个钩子结论相反。`pre-commit` 先跑
   `req-guard-check.sh`（失败即 `exit 1`），所以实际是拦的，但 `touch-check` 单跑会放行——
   **不要把「没声明」当作稳定的约束手段**，它的根因是「多清单歧义」，不是「该路径不许改」。

#### 设计 6：为什么用 POSIX `sh` 而不是 Python

初版写的是 Python（与 `scripts/verify_gate.py` 同风格），实测两条 CI 事实后推翻：

| 事实 | 出处 | 内容 |
| --- | --- | --- |
| GitHub Actions 有 Python | `.github/workflows/ci.yml` 第 59 行 | 已有兼容写法 `PY=$(command -v python3 \|\| command -v python \|\| echo python)` |
| **CNB 没有 Python** | `.cnb.yml` 第 18–22 行 | 镜像 `rust:1-slim-bookworm` 无 `python3`；现有约定是 `if command -v python3` → **跳过并打印提示** |

沿用 Python 只有三条路，逐一否掉：

| 路 | 问题 |
| --- | --- |
| 跟随既有约定（探测 + 跳过） | CNB 侧**完全不校验**。本需求防的是「无来源声明的产物入库」，跳过 = 那一侧洞开；且「探测不到就跳过」正是本工具最忌的「看着在检查、其实没检查」 |
| 在 CNB 上 `apt-get install python3` | 镜像变重、流水线变慢，还引入「apt 源不可用」这个新的失败面 |
| 保留 Python 并让 CNB 失败 | 永久红。噪音，且团队会习惯性忽略 |

改为 POSIX `sh` 后：依赖降到 `git` + `sh`，与 `.gates/hooks/*.sh` 同档，
**两条流水线都能真跑，不需要任何「跳过」分支**。

代价必须写明：C 组的 frontmatter 解析用 `awk` / `sed` 而非真正的 YAML 解析器，
比 Python 脆弱。缓解是——frontmatter 由 `req-guard` 自己生成，格式固定
（键在行首、数组单行），不是任意 YAML；且脚本对**解析不出来**的清单按
「列入报告」处理，不静默放过。另需补执行位（0o755），
这是本仓库「由工具自动执行的落盘脚本」的既有纪律。

### 涉及的文件与模块清单

见下方 `GATE:TOUCH` 块。两处通配各有理由：`specs/**` 是本需求的作用对象（逐个具名会随
feature 增长而漏），`.specify/**` 是 21 个文件的整个工具资产目录；其余六条精确到文件。

### 兼容性、性能与安全影响

- **兼容性**：纯新增脚本 + 一行配置，不改 `core/`，对既有判定零影响。
- **性能**：一次 `git ls-files` + 若干次文件读，毫秒级，可安全放进每次 CI。
- **安全**：脚本只读，无网络、无写文件、无 subprocess 拼接用户输入（路径全部来自 git 与固定清单）。
- **必须点名的残留风险**：`.gitignore` 本身在 `touch.exempt` 豁免区内，AI 可自行删掉
  `.codebuddy/` 那一行而不被变更范围契约发现；同理 `specs/**` 已声明且批准后，
  AI 可自由改写 SDD 产物（这正是 SDD 的工作方式，属**预期**）——
  真正的兜底是 A/B 两组 + CI，且它们只守「声明」，不守「内容」。

### 风险点与回滚方案

| 风险 | 严重级 | 缓解 |
| --- | --- | --- |
| 内容漂移查不出（清单改了 `specs/` 没改） | **中** | 明确写入残留风险与规范 §7；doc-guard 装上后接管时效；人工评审时以清单为准（声明里已写明） |
| 团队无视声明直接照 `specs/` 实施 | 中 | 声明块是人可读的引用块，渲染后可见；规范 R3 改写后同步强调 |
| C 组报告被长期无视 | 中 | 输出写明「doc-guard 装上后此项即为 FRS004 的前置条件」，并写入规范 §9 验收清单 |
| `awk`/`sed` 解析 frontmatter 比 YAML 解析器脆弱 | 中 | 解析不出来的清单按「列入 C 组报告」处理，**不静默放过**；frontmatter 由 req-guard 自己生成、格式固定（键在行首、数组单行），不是任意 YAML |
| `.specify/` 体积随版本膨胀 | 低 | 当前 156K / 21 文件；必要时改为只入库 constitution |

**回滚**：撤掉脚本、还原 `.gitignore` 一行、撤掉 CI 两步、删掉三个文件的声明注释 ——
四处独立可逆，无状态迁移、无数据变更。

<!-- 技术方案批准前必须在上方 frontmatter 的 source_refs 声明本次要改的文件/模块，
     否则 doc-guard 的 FRS004（源已改而规格未同步）无法发现规格腐化。
     格式为行内数组，元素可用目录（向下递归）：source_refs: [backend/src/main/java/com/x]
     注意：source_refs 不支持 glob，且它是 req-guard 从下方 GATE:TOUCH 块**单向派生**的
     （见 docs/设计/AC与变更范围契约技术方案.md §3.8）——**不要手改 frontmatter，
     改了会在下次 approve 时被覆盖回去。要改声明范围，改下面的块。 -->

<!-- 变更范围契约：GATE:TOUCH 是本清单唯一的「要改哪些文件」人工声明源。
     req-guard touch-check 用它与 git 实际改动集比对（pre-commit + CI），
     并在 approve 时单向派生出 frontmatter 的 source_refs。
     语法：每行一条相对仓库根的路径或 glob，# 后为注释，空行忽略。
       core/src/**      跨层级
       core/src/*.rs    单层
       cli/src/cli.rs   精确文件
     要求：反斜杠会被归一为 /；不接受 .. 逃出仓库根的条目。
     块为空 → 技术方案批准被拒（连同 source_refs 一并不写，保持 fail-closed）。 -->
<!-- GATE:TOUCH -->
.gitignore
scripts/verify_sdd_routing.sh
.github/workflows/ci.yml
.cnb.yml
docs/规范/SDD产物接入规范.md
docs/README.md
specs/**
.specify/**
<!-- /GATE:TOUCH -->

## 3. 测试计划

### 端到端用例（编号 + 执行步骤）

- **E-01** `specs/021-sdd-gate-visibility/` 三个文件均带合规声明
  → `sh scripts/verify_sdd_routing.sh` 退出码 **0**，输出含字面量 `SDD 路由合规`
- **E-02** 新增 `specs/021-sdd-gate-visibility/probe.md`（**不带**声明）并 `git add`
  → 退出码 **1**，输出含完整相对路径 `specs/021-sdd-gate-visibility/probe.md`
- **E-03** 承接 E-02，删掉该探针文件后 → 退出码回到 **0**
- **E-04** 把某文件的声明改成 `<!-- SDD-SOURCE: REQ-999 -->` → 退出码 **1**，输出含字面量 `REQ-999`
- **E-05** 把声明从文件开头挪到第 20 行之后 → 退出码 **1**（位置不合规）
- **E-06** 执行前后各记录一次 `git status --short` → 两次输出**逐字一致**（只读证明）

### 边界 / 异常场景

- **B-01** `specs/` 目录不存在（还没用过 SDD）→ 退出码 0，**不得**报「目录缺失」
- **B-02** 声明指向一份**已归档**清单（在 `archive/` 下）→ 视为存在，退出码 0
- **B-03** `specs/` 下的非 Markdown 文件（如图片、生成的图表）同样受 A 组约束
  ——要么带声明，要么挪出 `specs/`；脚本不因扩展名豁免
- **B-04** doc-guard / spec-kit 未安装 → 不影响退出码，只在输出标注「未安装，跳过」
- **B-05** 归档区 `.gates/requirements/archive/**` 的清单不参与 C 组统计（只读历史）
- **B-06** 某活跃清单 frontmatter 缺 `review_policy` → 列入 C 组报告，但**退出码仍为 0**
- **B-07** 声明写成 `REQ-21`（不足 3 位）或 `req-021`（小写）→ 按**不合规**处理，
  不做大小写/位数容错（容错会让「看起来声明了」通过，正是要避免的失效）

### 回归范围与影响面

- `python scripts/verify_gate.py` 13 个固定场景不回归；
- `req-guard install --verify` 仍 PASS；
- `req-guard ac check` 对全部未归档清单零 Error；
- `cargo test --workspace` 全绿（本需求不改 `core/`，属确认性回归）。

### 验收门槛（全部可机械判定）

- E-01 / E-03 锁放行，E-02 / E-04 / E-05 锁拒绝——**只有放行用例等于没测**
- E-06 证明只读：脚本不得修改任何文件
- 脚本零第三方 import（源码中除标准库外无 `import`）
- 不新增依赖：`Cargo.toml` 与 `core/Cargo.toml` 均不动

### 残留风险（必须随 PR 一起披露）

1. **内容漂移无法机械判定（本方案最主要的代价）**：清单改了而 `specs/` 没改，脚本查不出来。
   守住的是「指向」，不是「一致」。缓解靠声明块写明「以清单为准」+ 人工评审 + doc-guard 时效。
   这与 A 方案（草稿化）是同一类代价——A 根本没有副本，也就无从谈一致。
2. **doc-guard 未安装**：C 组只保证契约字段齐备，**实际 FRS 判定本次未验证**——
   doc-guard 装上后须补跑一次。
3. **`specs/**` 声明且批准后 AI 可自由改写**：这是 SDD 工作方式的预期后果（AI 要生成草稿），
   不是漏洞；但意味着「SDD 产物被 AI 悄悄改掉」这条路没有门禁，只有 CI 的 A/B 两组兜底。
4. **R3 改写是规范级变更**：本需求把「禁止并行维护」放宽为「禁止无来源声明的并行维护」。
   团队须明确接受第 1 条代价，否则应退回 A 方案。

<!-- 验收标准：req-guard ac check 机械校验（规则见 docs/设计/AC与变更范围契约技术方案.md §2）。
     条目形态唯一：三行式 —— 编号独占一行，其下恰好三个子句列表项。
     硬约束：编号形如 `AC-<3位数字>`，须连续、无重复、无跳号；三个子句顺序固定
     Given → When → Then；子句不得少于 4 个非空白字符；Given 必须自包含（不得写
     「与上一条相同」这类外部指代，引用违规样例一律用行内代码标记包起来）；
     Then 必须可度量（含数字或字面量）。
     注意：本说明刻意不写出具体编号字面量 —— 块外出现 `AC-` + 3 位数字会被 A7 判违规。
     标记行只能由 req-guard 维护；块内正文可自由编辑。 -->
<!-- GATE:AC -->
### AC-001
- Given: 某仓库的 `specs/021-sdd-gate-visibility/` 下有 3 个被 git 跟踪的 Markdown 文件，且每个文件前 15 行内都含 `<!-- SDD-SOURCE: REQ-021 -->`
- When: 执行 `sh scripts/verify_sdd_routing.sh`
- Then: 退出码为 0，且输出含 1 处字面量 `SDD 路由合规`

### AC-002
- Given: 某仓库的 `specs/021-sdd-gate-visibility/probe.md` 已被 git 跟踪，且该文件中不含任何 `SDD-SOURCE` 字样
- When: 执行 `sh scripts/verify_sdd_routing.sh`
- Then: 退出码为 1，且输出含 1 处完整相对路径 `specs/021-sdd-gate-visibility/probe.md`

### AC-003
- Given: 某仓库的 `specs/021-sdd-gate-visibility/spec.md` 第 1 行为 `# Feature Specification`，而 `<!-- SDD-SOURCE: REQ-021 -->` 出现在第 20 行
- When: 执行 `sh scripts/verify_sdd_routing.sh`
- Then: 退出码为 1，且输出含 1 处字面量 `SDD-SOURCE`

### AC-004
- Given: 某仓库的 `specs/021-sdd-gate-visibility/spec.md` 含声明 `<!-- SDD-SOURCE: REQ-999 -->`，且 `.gates/requirements/` 下不存在编号 REQ-999 的清单
- When: 执行 `sh scripts/verify_sdd_routing.sh`
- Then: 退出码为 1，且输出含 1 处字面量 `REQ-999`

### AC-005
- Given: 某仓库的 `specs/021-sdd-gate-visibility/spec.md` 含声明 `<!-- SDD-SOURCE: REQ-003 -->`，且编号 REQ-003 的清单位于 `.gates/requirements/archive/` 下
- When: 执行 `sh scripts/verify_sdd_routing.sh`
- Then: 退出码为 0，且输出不含字面量 `REQ-003` 的违规条目

### AC-006
- Given: 某仓库工作区处于任意状态，且在执行脚本前已用 `git status --short` 记录一次输出
- When: 执行 `sh scripts/verify_sdd_routing.sh` 后再执行一次 `git status --short`
- Then: 两次 `git status --short` 的输出逐字一致，差异行数为 0

### AC-007
- Given: 某仓库有 1 份活跃清单，其 frontmatter 含 `review_policy` 但 `source_refs` 为空数组，且所有 `specs/**` 文件声明合规
- When: 执行 `sh scripts/verify_sdd_routing.sh`
- Then: 退出码为 0，且输出中该清单编号被列入 1 处「契约不齐备」提示

### AC-008
- Given: 存在文件 `scripts/verify_sdd_routing.sh`
- When: 读取该文件第 1 行，并统计全文含字面量 `python` 的行数
- Then: 第 1 行为 `#!/bin/sh`，且含字面量 `python` 的行数为 0

### AC-009
- Given: 某仓库已执行 spec-kit 初始化并生成 `.codebuddy/commands/speckit.plan.md` 与 `.specify/memory/constitution.md`
- When: 执行 `git check-ignore -v .codebuddy/commands/speckit.plan.md` 与 `git check-ignore -v .specify/memory/constitution.md`
- Then: 前者退出码为 0（被忽略），后者退出码为 1（未被忽略）

### AC-010
- Given: 存在文件 `.github/workflows/ci.yml` 与 `.cnb.yml`
- When: 分别读取两个文件的全部内容并统计含字面量 `verify_sdd_routing.sh` 的行数
- Then: 两个文件中该字面量各出现 1 次，合计 2 次

### AC-011
- Given: req-guard 自身仓库，且 `scripts/verify_sdd_routing.sh` 已按本清单实施完成
- When: 执行 `req-guard install --verify`
- Then: 退出码为 0，且输出不含字面量 `缺口`

### AC-012
- Given: 存在文件 `scripts/verify_sdd_routing.sh`，且该文件的权限位为 0644
- When: 由实施者执行 `chmod 755 scripts/verify_sdd_routing.sh` 后运行 `test -x scripts/verify_sdd_routing.sh`
- Then: `test -x` 的退出码为 0（执行位已补；CI 直接调用脚本时不会因无执行位而失败）
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-10_17:20:57 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-10_17:24:07 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-10_17:24:18 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

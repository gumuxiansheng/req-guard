---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-04
source_refs: [core/src, cli/src, scripts, .gitignore, docs/规范, README.md]
---

# REQ-005 install 生成物纳入变更范围豁免（派生而非手写）

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-005 status=approved created=2026-10-04_13:37:57 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_13:41:17 sum=c5e09d15ba3b13777503b713438d80347501f768efe4b3297200c24de3600e9b -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_13:41:23 sum=0f36a2b211590b76e4fc1266362723f6dd3c9872096c3d060378b75317c29e18 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_13:41:34 sum=0ddf05dec5d43396cdd12b6c422812f4d3b388ffa64bb493c6911b07351820c2 -->

## 1. 需求分解

### 背景与问题

`.claude/settings.json`、`.codebuddy/settings.json`、`.codex/hooks.json`、`.cursor/hooks.json`
这四个文件是 `req-guard install` 生成的 AI 工具 hook 配置。它们被提交时，
变更范围门禁（`touch-check`）报 `NotDeclared` —— **每一份**。

这不是误报，是清单漏了；但它也不该由人用 `touch --declare` 去解决，原因是：

1. **它们不是任何人写的代码**，是 `install` 从 `templates/` 派生的产物。声明它们
   等于声明"本次要改这些文件"，而实际上没有任何需求在改它们。
2. **每一条这样的声明都是一次性谎言**：下次 `install` 升级、换了工具清单、或者
   用户本地多装了一个 AI 工具，又会冒出同一个拦截。
3. 真正的病根在 `core/src/gate.rs` 的 `touch_exempt_patterns`：默认集是
   **手写的 5 条常量**，而 install 明明知道它写了哪些文件，却没把这个知识交给豁免判定。
   手写常量与实际写入目标之间没有派生关系 —— 这与 `source_refs` 要单向派生、
   `GATE:TOUCH` 为唯一声明源是同一类病：**两份真相源，注定漂移**。

顺带发现一个同源的尖角：配置了 `touch.exempt` 会**整体替换**那 5 条默认，而不是追加。
所以"加一条豁免"这个动作在配置层做是危险的 —— 必须把默认项逐条抄回去，
漏抄一条就把 `.gates/**` 或 `target/**` 变成了未声明文件。

### 目标

- **G1** AI 工具 hook 配置（`install` 写入的目标）默认豁免变更范围判定，
  且**由 install 的写入表派生**，不手写第二条真相源。
- **G2** 豁免的只是 install 真正写入的那几个文件，**不是整个目录** ——
  `.claude/` 下的其它文件（如 `CLAUDE.md`）仍是普通源文件，改动必须声明。
- **G3** 配置 `touch.exempt` 时**追加**而非替换默认集，并修掉"漏抄一条就破防"的尖角。
- **G4** 这四个文件进 `.gitignore`：它们是可再生派生物，入库即多一份真相源
  （改了 `templates/` 不会跟着变），且会让 `git status` 常带噪声。

### 非目标

- **N1** 不豁免任何"人写的源文件"。豁免范围严格等于 install 的写入目标。
- **N2** 不改 `templates/` 本身，也不改 install 的落盘逻辑。
- **N3** 不把豁免做成"目录级通配"（如 `.claude/**`）—— 那会让工具自己的
  `CLAUDE.md`、`settings.local.json` 之类绕过声明。
- **N4** 不动 `touch-check` 的比对语义（集合判定、`--base`、聚合口径），
  只改"哪些路径不参与判定"这一层。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | 抽出 install 的「工具 → 写入目标」映射为可复用的单一函数（派生源） | 2h |
| T2 | `touch_exempt_patterns` 默认集改为「5 条常量 + 由 T1 派生」 | 2h |
| T3 | 配置 `touch.exempt` 由替换改为追加（修尖角） | 2h |
| T4 | `.gitignore` 增加四个生成物的条目 | 0.5h |
| T5 | 单测（派生一致性 / 非生成物不豁免 / 默认项不丢）+ `verify_gate.py` 场景 | 2h |
| T6 | 规范与 README 说明"install 生成物"这一类 | 1h |

合计 9.5h。T2+T4 是最小可交付（消除拦截且不留脏状态）。

### 影响范围

- **模块**：`core/src/gate.rs`（豁免默认值与配置读取）、`core/src/touch.rs`
  （判定消费方，若需）、`cli/src/main.rs`（install 写入目标）、`scripts/verify_gate.py`、
  `.gitignore`、规范文档与 README。
- **配置**：`touch.exempt` 语义由"替换"改为"追加"（见风险 2）。
- **清单格式**：无。
- **接口 / 数据表 / 外部 API**：无。
- **向后兼容**：`touch.exempt` 配置了 `[]` 或某条目的项目，其**生效集合变大**
  （多了默认 5 条 + AI 工具生成物），不会变小；此前依赖"替换"来收窄豁免的用法会失效。

### 本次事故暴露的相邻缺陷（不在本需求范围）

`install` 对已存在但内容陈旧的工具配置是否会覆盖、备份还是跳过，未在本需求判定 ——
那是 install 的幂等性问题，与豁免无关。另：`touch.exempt` 是否也该支持
「显式关掉某条默认」的否定语法，属配置语言设计，不在此处。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量）。

## 2. 技术方案

### 总体思路

**让"install 写了哪些文件"成为豁免判定的唯一真相源。**

现在 core 里同一件事有两份表述：`install` 的工具 → 路径映射（真），与
`touch_exempt_patterns` 的 5 条手写常量（副本）。把副本删掉，让豁免集 = 默认常量 ∪
install 写入目标派生的路径，就不会再漂移 —— 与 `source_refs` 单向派生、
`GATE:TOUCH` 唯一声明源是同一手法。

### 关键设计

1. **派生而非枚举**：工具 → 目标路径的映射抽成一个可复用函数，install 用它落盘、
   豁免判定用它生成豁免项。两者共用一份，新增工具时**不可能只改一处**。

2. **精确到文件，不给目录通配**：派生结果形如 `.claude/settings.json`（具体文件），
   **不是** `.claude/**`。这是 G2 的落点 —— 目录级通配会让工具自己的 `CLAUDE.md`、
   `settings.local.json` 绕过声明，等于给自己开一个 N1 禁止的绕道。

3. **默认集与配置改为追加**：`touch.exempt` 从"替换默认"改为"追加到默认"。
   默认 5 条全是"本来就不该被声明"的项（门禁自己的运行态、构建产物、锁文件），
   没有人有理由主动收窄它们；替换语义只制造了一个陷阱 —— 加一条豁免时必须把
   5 条默逐条抄回，漏抄即破防。

4. **不入库 + 进 .gitignore**：这四个文件是可再生派生物。入库即多一份真相源
   （改 `templates/` 不会同步已入库的那份），且每次 `git status` 都带 4 个噪声项。
   新 clone 的人本来就得跑 `req-guard install`，否则门禁不生效 —— 这不是额外成本。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
core/src/gate.rs
core/src/touch.rs
cli/src/main.rs
scripts/verify_gate.py
.gitignore
docs/规范/AI工具合规保证规范.md
README.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：`touch.exempt` 生效集合变大（见影响范围），不会变小。
  依赖"替换以收窄"的项目需改为显式声明那些路径。
- **性能**：豁免集在每次 `touch-check` 构造一次，条目从 5 条增至 5 + 工具数（约 4~8），
  量级不变，无可测差异。
- **安全**：豁免只放宽"是否需要声明"，**不放宽任何内容判定** —— 这四个文件不含
  清单正文、不参与 `ac check` / `verify-content` / 审批锁。安全边界不变。

### 风险点与回滚方案

- **风险 1（中）**：派生函数若漏了某个工具，会退回 `NotDeclared` 拦截 ——
  表现为"拦截"而非"漏放行"，方向安全（fail-closed）。由 T1 的单测锁死。
- **风险 2（中）**：`touch.exempt` 改为追加，可能与既有项目的"替换以收窄"用法冲突。
  缓解：改动写进 README 与规范；这些项目需改为显式声明。
- **风险 3（低）**：`.gitignore` 增加条目后，已跟踪的同名文件不会被自动忽略 ——
  本仓它们本就未入库，无迁移问题；其他项目需 `git rm --cached`。
- **回滚**：T1+T2 可独立回退（回到 5 条常量），代价是拦截复现；T3 独立回退；
  T4 就是删几行 `.gitignore`。

## 3. 测试计划

### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | 派生的豁免项与 install 实际写入目标一致 | 逐项相等（同一份映射） |
| U2 | 默认豁免仍含 `.gates/**`、`target/**`、`dist/**`、`Cargo.lock`、`.gitignore` | 5 条全在 |
| U3 | 配置 `touch.exempt` 后默认项仍在 | 追加语义，默认 5 条一条不少 |
| U4 | 配置项与默认项同时生效 | 并集，无覆盖 |
| U5 | AI 工具生成物命中豁免 | `.claude/settings.json` 等放行 |
| U6 | **同目录下的非生成物不豁免** | `.claude/CLAUDE.md` 仍需声明 |
| U7 | 豁免不改变其它判定 | `ac check` / `verify-content` 结果不变 |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 已 `install` 的仓里改 `.claude/settings.json` 并 stage → `touch-check` | exit 0 |
| E2 | 同仓改 `.claude/CLAUDE.md` 并 stage → `touch-check` | exit 1，`NotDeclared` |
| E3 | `touch.exempt` 配置一条自定义路径后重跑 E2 | 追加生效，默认项未丢 |

### 边界 / 异常场景

- **B1** `install --tool none`（不写任何 AI 配置）→ 豁免集里不应出现 AI 路径。
- **B2** 未知工具名 → 派生跳过而非 panic。
- **B3** `.gitignore` 已跟踪同名文件 → 提示需 `git rm --cached`，不静默。
- **B4** 配置文件里 `touch.exempt` 为空列表 → 等同于只用默认集（追加后为空贡献）。

### 回归范围与影响面

- `core` 全量单元测试；`verify_gate.py` 全部场景（含 21 号豁免场景）。
- REQ-001~004 的 `check` / `ac check` / `touch-check` / `verify-content` 结论不变。
- `install --verify` 仍须报告资产就位。

### 验收门槛

- `cargo fmt --all --check` 与 scoped `clippy -- -D warnings` 零输出。
- `cargo test -p req-guard-core -p req-guard` 全绿。
- `python3 scripts/verify_gate.py` 全部场景通过。
- core 依赖清单不新增第三方依赖。

<!-- GATE:AC -->
### AC-001
- Given: 一个已执行 `req-guard install` 的仓，`touch-check` 判定集含默认项
- When: 暂存 `.claude/settings.json` 并执行 `req-guard touch-check`
- Then: 退出码为 0，`NotDeclared` 问题数为 0

### AC-002
- Given: 一个已执行 `req-guard install` 的仓
- When: 暂存 `.codex/hooks.json` 与 `.cursor/hooks.json` 并执行 `req-guard touch-check`
- Then: 退出码为 0，未声明文件数为 0

### AC-003
- Given: 一个已执行 `req-guard install` 的仓
- When: 暂存 `.claude/CLAUDE.md` 并执行 `req-guard touch-check`
- Then: 退出码为 1，问题类型为 `NotDeclared`

### AC-004
- Given: `.gates/req-guard.yaml` 的 `touch.exempt` 只写了 1 条自定义路径
- When: 读取生效豁免集
- Then: 该自定义路径与默认 5 条（`.gates/**`、`target/**`、`dist/**`、`Cargo.lock`、`.gitignore`）同时在集内

### AC-005
- Given: `.gates/req-guard.yaml` 显式配置了 `touch.exempt`
- When: 暂存 `.gates/requirements/REQ-001-ac.md` 并执行 `req-guard touch-check`
- Then: 退出码为 0，即默认的 `.gates/**` 豁免未被配置覆盖掉

### AC-006
- Given: install 的工具列表包含某 AI 工具
- When: 读取默认豁免集并与 install 的实际写入目标逐项比对
- Then: 未匹配项数为 0，且 install 列表新增 1 个工具后豁免集条数增加 1

### AC-007
- Given: 仓库根目录的 `.gitignore`
- When: 检查其中是否含 AI 工具生成物的条目
- Then: `.claude/settings.json`、`.codebuddy/settings.json`、`.codex/hooks.json`、`.cursor/hooks.json` 四项各命中 1 次

### AC-008
- Given: 豁免集新增了 AI 工具生成物
- When: 对同一份已批准的清单执行 `ac check` 与 `verify-content`
- Then: `ac check` 与 `verify-content` 的退出码均为 0，且改动前后问题总数之差为 0

### AC-009
- Given: 实现完成后的 `scripts/verify_gate.py`
- When: 执行该脚本
- Then: 全部场景通过，且新增的豁免场景数不少于 1

### AC-010
- Given: 实现完成后的 core 源码全量
- When: 检索 AI 工具目录的目录级通配豁免（如 `.claude/**`）
- Then: 命中数为 0，即豁免只精确到 install 写入的文件，不覆盖整个目录
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-04_13:41:17 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-04_13:41:23 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-04_13:41:34 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

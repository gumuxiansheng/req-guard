# 技术方案：验收标准（AC）机械校验与变更范围契约（GATE:AC / GATE:TOUCH）

> **状态：提案（P1–P4 均未实现）。** 本文定义格式契约与判定规则，作为后续实现的唯一依据。
> 配套：`../需求/需求分析报告.md`、`../规范/AI工具合规保证规范.md`、
> 本需求自身的清单 `.gates/requirements/REQ-001-ac.md`。
>
> **本文档自身即 dogfood 样本**：该清单的第 3 段已按本文 §2.1 的 `GATE:AC` 格式书写
> （52 条 AC，**全部三行式** —— 单行式已在 §2.1 废除），第 2 段已按 §3.1 的 `GATE:TOUCH` 格式书写。
> 当前两者**仅靠约定生效**（`ac check` / `touch-check` 尚未实现），
> 第 3 段的 AC 清单即 P0–P4 的验收基准。
>
> **dogfood 首个回合即产出两个 P0 缺陷**（见 §0.1）：`review` / `gate_lines` 用 `contains`
> 匹配 `GATE:STEP`，导致正文里合法提到该字面量的散文被静默改写 / 被误判为状态行。
> 这两个缺陷先于本需求存在，但**必须先修**——本需求引入的 `GATE:AC` 恰好要求正文讨论
> `GATE:STEP` 语义，不先修则新格式上线当天就大面积误拦 AI 写正文。

> **引用约定**：按 `docs/README.md` §5.3，代码位置一律写「文件名 + 符号名」，不写行号。

---

## 0. 现状核对（约束设计的既有事实）

| 事实 | 出处 | 对设计的约束 |
| --- | --- | --- |
| 三段清单只验**审批状态**，不验**内容** | `core/src/gate.rs` 的 `HOOK_SH` 第 3 段（`grep 'GATE:STEP'` + `status=approved`） | 新增校验必须落在**状态行之外**的正文上 |
| 分段靠二级标题数字，定位失败**回退整篇** | `core/src/requirement.rs` 的 `section_of` | AC/TOUCH 校验**不能**复用它（见 §2.2 A1 说明） |
| 追溯矩阵**不存在**（全仓无实现、无模板、无文档） | — | 本文不是扩展现有矩阵，而是**新建**这段契约 |
| `HOOK_STAGED_FILES` 不存在；pre-commit 是生成的一段 sh | `core/src/gate.rs` 的 `PRE_COMMIT_BLOCK` / `append_pre_commit` | 变更范围校验需自建 staged 文件来源 |
| GATE 标记行的防篡改只覆盖 HEAD/STEP | `core/src/gate.rs` 的 `gate_lines` / `doc_write_guard` | 新标记行**必须**纳入，否则 AI 删块即绕过（见 §2.6） |
| 审批是唯一解锁钥匙 | `core/src/requirement.rs` 的 `review` | 把内容校验挂在这里，才是真正的牙齿（见 §2.5） |
| 零依赖铁律 | `Cargo.toml`（core 无 `[dependencies]`） | 通配匹配、JSON、YAML 全部手写，不引入 crate |
| 判定唯一真相在 core | `core/src/lib.rs` 模块文档 | 判定逻辑只在 core 实现；脚本只负责取参与渲染 |

### 0.1 dogfood 发现的两个 P0 缺陷（v0.1.5 已存在，先于本需求存在）

REQ-001 第一次 `reject --step testplan` 就把它们打出来了 —— 正文里一句
``GATE:STEP name=testplan 的 status 变为 approved`` 被**原地改写成机器标记行**。

| # | 位置 | 现状 | 后果 |
| --- | --- | --- | --- |
| **P0-1** | `core/src/requirement.rs` 的 `review` | `line.contains("GATE:STEP") && token(line,"name")==step` —— **子串**匹配 | ① 正文里任何提到 `GATE:STEP name=<step>` 的散文被**静默销毁**；② 机器标记被**注入正文内部**；③ 注入行 `label=` 为空，违反 `safe_field` 的 `-` 约定 |
| **P0-2** | `core/src/gate.rs` 的 `gate_lines` | `l.contains("GATE:HEAD") \|\| l.contains("GATE:STEP")` | `doc_write_guard` 把散文当状态行 → AI 正常改动任何提及 `GATE:STEP` 的行都被**误判为自批并拦截**。即"正文合法地提到 `GATE:STEP` ⇒ 该清单此后再也写不动" |

P0-2 是本需求元问题在工具自身的镜像：**工具把自己的机器标记语法当成了散文的一部分**。
本需求引入的 `GATE:AC` 恰好要求正文讨论 `GATE:STEP` 语义 —— 不先修这两个 P0，
新格式上线当天就会大面积误拦。

**修法**（两处一致）：把"是否状态行"的判据从"包含 `GATE:*` 字面量"改为
"**以标记语法开头**"——

```rust
// 旧：l.contains("GATE:HEAD") || l.contains("GATE:STEP")
// 新：只认真正的标记行（`render` 与所有写入点产出的行恒以 `<!--` 开头，
//     合法 Markdown 正文不可能以 `<!-- GATE:STEP` 开头）
l.trim_start().starts_with("<!-- GATE:HEAD") || l.trim_start().starts_with("<!-- GATE:STEP")
```

`head_line` / `step_line` 同样收紧。顺带（**P2 可选**）：`HOOK_SH` / `HOOK_PS1` 的
`grep 'GATE:STEP'` 建议加 `^` 锚点 —— 当前靠"标记行在文件头、`head -1` 先命中"侥幸成立，
但注入行一旦存在就可能被误选为权威状态行。

回归见 REQ-001 的 U-16 / U-17 / E-06。

---

## 1. 设计原则（继承既有 doctrine，不新发明）

1. **人机双读**：正文仍可自由编辑，`GATE:*` 标记行仍只由 req-guard 命令改写。
2. **fail-closed**：判不准就拦。缺标记块、段落定位失败、二进制不在 PATH —— 一律拦，不放行。
3. **判定唯一真相在 core**：shell 只取参（staged 文件集）、只渲染（stderr 文案）。
4. **机械可判定**：规则里不出现"是否清晰""是否充分"这类需要人判断的谓词。
5. **误报是头号死因**：门禁一旦被习惯性 `--no-verify` 绕过就等于不存在。凡可能误报处，默认取宽口径并给出合法出路。

---

## 2. 需求一：AC 机械校验（消灭第三段空话）

### 2.1 文档格式契约

第 3 段内引入机读块（块扫描范式抄 `core/src/comment.rs` 的 `BEGIN`/`END` 标记）：

```markdown
## 3. 测试计划

<!-- GATE:AC -->
### AC-001
- Given: 已安装 req-guard 0.1.5，且某清单三段 GATE:STEP 均为 approved
- When: 执行 `req-guard check`
- Then: 退出码 0，stdout 含"放行"

### AC-002
- Given: 三段有任一未批
- When: 执行 req-guard check
- Then: 退出码 1 且 stderr 列出未批步骤
<!-- /GATE:AC -->
```

**条目形态唯一：三行式。** 编号独占一行，其下恰好三个 `Given:` / `When:` / `Then:` 前缀列表项。

**曾有过「单行式」（`编号｜Given｜When｜Then` 一行写完），已废除** —— 理由是可读性，不是判定难度：

1. 单行式在 Markdown 渲染里（TUI / GUI / GitHub / VSCode）是一整行长文本；
   三行式是「短标题 + 三个列表项」的标准结构，**结构本身就在说明"这是三个子句"**。
   审核人扫读时前者要逐字断句，后者一眼看出三段。
2. 分隔符带来判定歧义：ASCII `|` 是 GFM 表格列分隔符；全角 `｜` 又要求
   「子句内容里不得再出现 `｜`」—— 两条约束都是为迁就单行式而存在的额外负担。
3. 需求清单是**给人审、给机器判**的文档。人读的那一半优先。

废除后的残留由 A12 兜住：编号行的下一非空行若含 ≥2 个全角 `｜` 即报 `InlineEntry`。

**形式必须逐条判定**（看该条自己的下一非空行），不得按块内首条的形式套用全块。

### 2.2 机械规则

| 编号 | 规则 | 判据 | 严重级 |
| --- | --- | --- | --- |
| A1 | 第 3 段内**恰一对** `GATE:AC` 标记；缺失或未闭合即错 | 标记计数 | Error |
| A2 | 条目起始行为**编号行**，允许 `#`/列表符/`AC-001：` 前缀，但**行内只许有编号** —— 出现子句关键词或 `｜` 即错（防标题污染） | 编号行 token | Error |
| A3 | 编号自 `AC-001` 起**连续**，无重复、无跳号（编号是未来追溯矩阵的 join key） | 集合比对 | Error |
| A4 | 编号行之后连续的 `Given:`/`When:`/`Then:` 前缀行各**恰好一次**且顺序为 G→W→T（**三行式是唯一合法形态**） | 前缀行计数与顺序 | Error |
| A5 | 关键词大小写不敏感；中文别名 `Given∈{假设,假定,前置}`、`When∈{当,执行}`、`Then∈{则,那么,预期}`；**同一 AC 内不得中英混用**。中文别名**必须**跟 `：`（ASCII 关键词可省略），否则 `当前状态…` 这类散文会被误判成 `当` 子句 | 词表 + 一致性 | Error（混用 Warn） |
| A6 | 三个子句任一 < 4 个非空白字符即错（堵 `Given: -`） | 长度 | Error |
| A7 | 第 3 段内、块**外**出现 `AC-\d{3}` 编号即错（堵"AC 写在第 2 段充数"） | 全段扫描 | Error |
| A8 | 条目数 ≥ 1 | 计数 | Error |
| A9 | `Then` 不含数字或行内代码字面量 → 不可度量 | 正则 | **Warn** |
| **A10** | **每条 AC 的 `Given` 必须自包含**：不得出现"与上一条相同"/"同上"/"同 AC-NNN"这类外部指代；涉及的 glob、文件名、配置键须**原文写出**。**匹配前先剥离行内代码（`` `…` ``）** —— AC 要复现违规写法时必然要引用它，故约定：**引用违规样例一律用行内代码标记包起来**，被引用的字面量不算指代 | 剥离行内代码后的指代词表 | Error |
| **A11** | **块内不出现条目编号以外的分组标题混排**（分组小标题一律放块外，避免被误判为条目或噪声） | 标题扫描 | Error |
| **A12** | **单行式已废除**：编号行的下一非空行若含 ≥2 个全角 `｜` → `InlineEntry` | `｜` 计数 | Error |

A9 刻意不阻断：可度量性本质是启发式，误伤成本高于收益。
A2 / A10 / A11 / A12 是 dogfood 复核新增的**可读性硬约束** —— 每一条都对应一个真实踩过的坑：
标题污染、Given 不可自包含、分组标题混入、单行式残留。


### 2.3 判定落点

| 模块 | 内容 |
| --- | --- |
| `core/src/issue.rs`（新增） | `Severity`（从 `core/src/idcheck.rs` 的 `Severity` **迁入**）+ 通用 `Issue { severity, kind, message }` |
| `core/src/idcheck.rs`（改） | 改 `pub use crate::issue::Severity;`，删本地定义 —— 对外 API 不破 |
| `core/src/ac.rs`（新增） | `AcItem`、`AcIssueKind`、`AcIssue`、`parse()`、`lint()`、`check()` |
| `core/src/touch.rs`（新增） | `TouchIssueKind`、`declared()`、`glob_match()`、`staged_files()`、`diff_files()`、`check()` |

**`AcIssueKind` 全量枚举**（A1–A12 的 kind 一一对应；**枚举即契约**，新增规则必须新增变体，
由 §6 的枚举覆盖门槛强制其被测）：

```rust
pub enum AcIssueKind {
    MissingBlock,    // A1 第 3 段无 GATE:AC 标记
    UnclosedBlock,   // A1 标记未闭合
    NoItem,          // A8 块内条目数为 0
    BadIdFormat,     // A2 编号格式非法（`### AC-1`、`### AC1`、行首 `AC` 却不成编号）
    DuplicateId,     // A3 编号重复
    SeqGap,          // A3 编号跳号 / 非 AC-001 起
    MissingClause,   // A4 三行式条目缺 Given/When/Then 中某一项（或子句数 ≠ 3）
    OrderClause,     // A4 三行式条目子句顺序非 G→W→T
    EmptyClause,     // A6 子句 < 4 个非空白字符
    OutsideBlock,    // A7 块外出现 AC 编号
    AliasMix,        // A5 同一条目内中英混用（**Warn**）
    Unmeasurable,    // A9 Then 无数字/退出码/字面量（**Warn**）
    DirtyIdLine,     // A2 编号行内混入子句（标题污染）
    NonSelfContained,// A10 Given 出现"与上一条相同"/"同上"/"同 AC-NNN"外部指代
    StrayHeading,    // A11 块内混入非条目用的分组标题
    InlineEntry,     // A12 单行式残留（已废除的形态）
    SectionNotFound, // 实现约束：第 3 段二级标题定位失败
}
```

**`TouchIssueKind` 全量枚举**：

```rust
pub enum TouchIssueKind {
    MissingBlock,     // T1 第 2 段无 GATE:TOUCH 标记
    UnclosedBlock,    // T1 标记未闭合
    EmptyDeclaration, // T1/T2 块存在但无有效条目
    BadPath,          // T3 路径无法归一（空串、含 `..` 逃逸出仓库根）
    NotDeclared,      // T4 实际改动未被任何声明覆盖
    SectionNotFound,  // 实现约束：第 2 段二级标题定位失败
}
```

`parse()` / `lint()` 必须是**纯函数**（入参一段正文，出参条目与问题），这样 A1–A12 可用
table-driven 单测覆盖，不需要临时目录。

`Severity` 不得在 `ac.rs` / `touch.rs` 各复制一份 —— 判定语义只有一份是本项目的核心不变式
（已落地：`core/src/issue.rs` 承载 `Severity`，`idcheck` 改 `pub use` 重导出，对外 API 不破）。

### 2.4 CLI 契约

照抄 `token` 子命令范式（`cli/src/cli.rs` 的 `Action::Token` + `"token"` 分支 + `validate()` + `help()`）：

```
req-guard ac check              # 默认：全部未归档清单
req-guard ac check REQ-001      # 指定清单
req-guard ac check --all        # 含归档区（审计用，只读）
```

**输出格式**（渲染复用 `ids --check` 的房屋风格：逐条 `{标记} [{严重级}] {message}`，message 自含上下文）：

| 情形 | stdout / stderr | 退出码 |
| --- | --- | --- |
| 全部合规 | `✅ 验收标准合规：<ID> 共 N 条 AC` + 每条一行 `   AC-00N \| <Then 子句原文>` | 0 |
| 有 Error | 逐条 `✗ [错误] [<Kind>] <message>`，**Error 全部排在 Warn 之前**；末尾 stderr 汇总 `共 E 项硬伤 / W 项告警；修复硬伤后重跑 req-guard ac check` | 1 |
| 仅 Warn | 逐条 `⚠️ [警告] [<Kind>] <message>`（**一条都不许静默丢弃**）；末尾 stdout `共 W 项告警（不阻断）` | 0 |

`<message>` 必须含：清单 ID、`AC-00N` 编号（若有）、**1-based 行号**、缺失项名或未声明文件名、修复指引。

**Warn 通路是门禁唯一的静默降级口**（A5 `AliasMix`、A9 `Unmeasurable`），
"仅 Warn → exit 0 且照打" 这条行为本身就是必须被验收的契约（见 REQ-001 的 AC-014 / AC-015）。

### 2.5 ★ 强制点：挂在审批上（本需求成立与否的分水岭）

在 `core/src/requirement.rs` 的 `review()` 内：

- `pass == true && step == "testplan"` → 先跑 `ac::lint`，有 Error 则 `Err(GateError::Validation(...))`，
  错误信息内嵌**可直接粘贴的 `AC-001` 骨架片段**；
- `pass == true && step == "solution"` → 校验 `GATE:TOUCH` 块存在且非空（见 §3.2 T1）。

理由：审批是唯一能解锁编码的动作。把校验挂在钥匙上，"三段都是空话"在物理上无法通过 ——
**不依赖任何人记得跑 `ac check`**。`ac check` 只是把同一判据提前暴露给 AI 与 CI，用于早失败。

### 2.6 AI 写权边界（不做则整条链有洞）

`core/src/gate.rs` 的 `gate_lines()` 当前只收集 `GATE:HEAD` / `GATE:STEP`，`doc_write_guard()`
靠它逐行比对来防自批。新块若不在其中，**AI 删掉整个 `GATE:AC` 块即可让校验永久失效** ——
这正是本工具最坏的失效模式（"看着在拦、其实没拦"）。

改法：把 `GATE:AC` / `GATE:TOUCH` 的**标记行**纳入 `gate_lines`（正文可写、标记行不可增删）。
语义与现状一致（AI 本就可在清单正文里写 AC 与路径清单），代价两行。

### 2.7 存量迁移

P1/P2 上线后，所有存量清单都会 `ac check` 报错。必须先给迁移路径再开 Error，二选一（建议 A）：

- **A（建议）**：`req-guard ac init <REQ-ID>` 幂等注入骨架块（标记行由 req-guard 写，合法），AI 只填内容；
- **B**：`ac check` 的 Error 内嵌完整待粘贴片段，且 `install` 时对存量清单输出一次 note。

---

## 3. 需求二：变更范围契约（GATE:TOUCH）

### 3.1 格式契约

第 2 段内：

```markdown
## 2. 技术方案
...
<!-- GATE:TOUCH -->
core/src/ac.rs              # 精确文件
core/src/**                 # 模块 / 目录（** 跨层级）
cli/src/cli.rs
<!-- /GATE:TOUCH -->
```

### 3.2 规则

| 编号 | 规则 | 严重级 |
| --- | --- | --- |
| T1 | 第 2 段内恰一对标记，闭合；块为空即错（"方案没说改哪儿"） | Error |
| T2 | 每行一条路径 / glob；`#` 后为注释；空行忽略 | Error |
| T3 | 路径相对**仓库根**；反斜杠归一为 `/`；去 `./` 前缀 | Error |
| T4 | **反向包含**判定：`实际改动集 ⊆ 声明集（多份清单时取并集）` | Error |
| T5 | exempt 清单内的路径不参与比对（配置 `touch.exempt`，默认最小集） | — |
| T6 | 实际改动集为空（无 staged 文件 / 非 git 场景）→ 显式说明后放行 | — |

T4 的方向是刻意选的：`声明 ⊇ 实际`。要求"实际 ⊆ 声明"意味着**多声明无罪、漏声明有罪** ——
这与"方案可以写得比实现细，但不能比实现窄"一致，且不会因为方案里多列了两个文件而拦截。

### 3.3 core 侧与 glob 语义

`core/src/touch.rs`（新增，零依赖）：

| 符号 | 说明 |
| --- | --- |
| `declared(section)` | 纯函数：抽 TOUCH 声明 |
| `glob_match(pattern, path)` | 零依赖通配匹配（递归回溯，~40 行） |
| `staged_files(root)` | 优先读 env `HOOK_STAGED_FILES`（换行分隔，供测试与 gates-toolkit 传参）；回落 `git -c core.quotePath=false diff --cached --name-only --diff-filter=ACMRD -z`，按 `\0` 切分 |
| `diff_files(root, base)` | `git diff --name-only <base>...HEAD`（L3 用） |
| `check(root, staged, scope)` | 判定 + 问题列表 |

`glob_match` 语义必须在 doc comment 与单测表中写死：

- `*` **不跨** `/`；`**` 跨 `/`，且 `core/src/**` 命中 `core/src/gate/x.rs`；
- `**/*.rs` **匹配零段**（即也命中根级 `ac.rs`）——用户一定会这么写；
- 支持 `?`；
- `{a,b}` 花括号：**要么实现，要么在格式契约里明文声明不支持**。禁止"写了却静默不匹配"。

`--diff-filter=ACMRD` 含 `D`：删除未声明的文件同样应被拦（AI 删了方案外的东西）。

### 3.4 挂载点：独立脚本，不动 HOOK_SH

新增 `.gates/hooks/req-guard-touch-check.sh`（常量 `HOOK_TOUCH_SH_REL`，由 `gate::install` 落盘 +
`ensure_executable`），由 `PRE_COMMIT_BLOCK` 第二步调用：

```sh
sh .gates/hooks/req-guard-check.sh || exit 1
if command -v req-guard >/dev/null 2>&1; then
  HOOK_STAGED_FILES="$(git -c core.quotePath=false diff --cached --name-only --diff-filter=ACMRD 2>/dev/null)" \
    req-guard touch-check || exit 1
else
  echo "✗ req-guard 不在 PATH，无法校验变更范围，提交已被阻止（fail-closed）。" >&2
  exit 1
fi
```

**为什么不加进 `HOOK_SH`（而是新脚本）**：

1. `HOOK_SH` 同时服务两个上下文 —— AI PreToolUse（stdin 有 payload、无 staged 集）与
   pre-commit（靠 `[ ! -t 0 ]` 猜上下文）。第 0 段那段启发式不能作为新校验的前提。
2. 任何加进 `HOOK_SH` 的判定都必须在 `HOOK_PS1` 里**逐行镜像一遍** —— 纯负债。
3. 独立脚本 = 单一职责、单一上下文、无 Windows 镜像负担（pre-commit 本来就只注入 sh，
   Windows 走 Git Bash）。

### 3.5 exempt 与多需求聚合（两个最容易埋雷的点）

**exempt**（`.gates/req-guard.yaml` 新增 `touch.exempt`，默认最小集）：

| 默认 exempt | 理由 |
| --- | --- |
| `.gates/**` | AI 必须能改自己的需求文档、评论、台账；否则门禁自锁 |
| `target/**`、`dist/**` | 构建产物 |
| `Cargo.lock`、`.gitattributes` | 锁文件与行尾属性不该占用方案篇幅 |

`.github/workflows/**` **不**默认豁免：改 CI 编排就该被声明。

**路径归一（T3）**：反斜杠 → `/`；去 `./` 前缀；拒绝含 `..` 且逃出仓库根的条目
（`BadPath`）—— 否则 `../secrets` 形式的声明会静默匹配到仓库外。

**多需求聚合**：默认取**全部未 done 清单的声明并集**；`HOOK_REQ=REQ-002` 可收窄到单份。

理由：仓库可能并行多份未归档清单。若按"当前活跃需求"单条比对（与 `HOOK_SH` 第 2 段同规则），
AI 在 REQ-002 上写代码却撞上 REQ-001 的声明集 —— 必然产生误报，而**误报会催生习惯性绕过**。
并集口径仍能消灭真实失效模式："改了 40 个方案里从未提过的文件"。
严格口径作为可选收紧项：`touch.scope: strict`（只比最新活跃需求），默认 `false`。

**`done`（已归档）清单的声明一律不参与并集** —— 否则一份几个月前的归档清单会把声明集撑成
"什么都允许"，等于关掉这道墙（归档是生命周期终点，见 `requirement::done`）。

### 3.6 ★ L3 抵消（不做则闭环不成立）

pre-commit 是本机墙，`git commit --no-verify` 一穿而过。必须有服务端对应物：

```
req-guard touch-check --base origin/main      # 内部 git diff --name-only origin/main...HEAD
```

判定与 §3.3 完全同一份代码。CI 模板（`templates/ci/req-guard-ci.yml`）加一步。

**★ 模板与命令的一致性必须被机械校验**：只改模板不改判定、或只实现命令忘了加模板步骤，
两者都会让 L3 静默失效 —— 而"看起来在拦、其实没拦"是本工具最坏的失效模式。
故 `verify_install` 必须新增一项：`.gates/ci/req-guard-ci.yml` 须同时含
`touch-check --base` 与 `ac check --all` 两个步骤，缺任一即报缺口（与现有 L2/L3 缺口同口径）。
这正是 `verify_install` 当初为防"只装不生效"而存在的同一类检查。

### 3.7 范围扩张的合法路径（否则只会催生绕过）

```
req-guard touch --declare <glob>... [--reason <原因>]
```

1. 向活跃清单的 `GATE:TOUCH` 块**追加**声明（去重、保序）；
2. 走 `auth::ensure_human` + `identity::bind` —— **AI 不得自己扩范围**，否则 `--declare` 就是绕过；
3. 写 `gate::audit` + `gate::audit_ledger`（事件 `TOUCH.EXTEND`，PR 可复核）；
4. 默认（`touch.reapprove: true`）把 `GATE:STEP name=solution` 打回 `pending` 并 `recompute_head`
   → `changes_requested`：**范围扩张必须重新过审**。

第 4 步才是"plan 从散文变成契约"的实质：变更范围不是 AI 可单方面编辑的字段。

拦截时的 stderr 必须一次给全三张牌：超出清单（逐条列出）、`touch --declare` 改方案、`--no-verify`。
既有的 `.gates/.bypass` 仍是唯一总逃生阀（有痕、有时效、入审计）。

---

### 3.8 ★ 与 `source_refs` 的收口：TOUCH 为源，单向派生

本仓库同期存在另一项能力（`core/src/specmeta.rs` + frontmatter `source_refs`），
它与 `GATE:TOUCH` **在目的上重叠**（都声明"要改哪些文件"），且都挂在 `approve --step solution` 上。
两处都要求人工维护 → 同一份文件清单要写两遍 → 必然漂移，而漂移的声明等于没有声明。

**决策：`GATE:TOUCH` 是唯一人工维护的声明源；`source_refs` 由 req-guard 单向派生写入。**

| | `GATE:TOUCH` | frontmatter `source_refs` |
| --- | --- | --- |
| 谁维护 | **人 / AI（唯一声明源）** | **req-guard 自动派生，禁止手改** |
| 位置 | 第 2 段内（人读的位置） | 文件最前（`doc-guard` 的 `matter::extract` 要求首个非空行是 `---`） |
| 表达力 | glob（`**` / `*` / `?`） | 目录前缀 + 向下递归，**不支持 glob**（见 `specmeta::missing_source_refs_hint`） |
| 用途 | **单次提交**：pre-commit 比对实际改动集 + CI `--base` | **长期绑定**：doc-guard FRS003 / FRS004 / DRF001 复核规格腐化 |

两者**互补而非重复**：`source_refs` 管"这份规格长期绑定哪些代码"（跨提交、跨时间、服务端复核）；
`TOUCH` 管"本次实现实际改了哪些文件"（单次提交、本机强制）。

**关键事实：req-guard 侧对 `source_refs` 只卡"非空"，从不比对实际改动集** ——
真正做「源改了而规格没改」比对的 FRS004 在 `doc-guard` 侧，而本仓库没有该 crate。
所以「实际改动 ⊆ 声明」这件事**在本仓库内目前无人管**，那正是 `TOUCH` 的位置。

#### 派生规则

`approve --step solution` 时执行，方向**单向**：

1. 读第 2 段的 `GATE:TOUCH` 块（§3.3 的 `declared()`）。
2. 归一每条路径（反斜杠 → `/`、去 `./`），去重、**保序**（顺序影响 diff 稳定性）。
3. **一律降级为目录前缀**（`source_refs` 不支持 glob，且**元素按「目录 + 向下递归」理解**
   —— 此点已与 `doc-guard` 侧确认，FRS004 对文件路径元素同样按目录处理）：
   - `core/src/**` → `core/src`（截到通配符前）
   - `core/src/*.rs` → `core/src`
   - `cli/src/cli.rs` → **`cli/src`**（取所在目录）
   - `docs/README.md` → `docs`
   - `Makefile`（根级文件）→ 保留原样：无父目录可退，退成仓库根等于声明「整个仓库」
   - `**/*.rs`（通配符在首段）→ **丢弃并告警**：它的目录语义是整个仓库，
     而 `source_refs` 表达不出「仓库根」；静默丢弃会让人以为已声明
4. 写入 frontmatter `source_refs: [a, b, c]`（行内数组，与 `specmeta` 的解析子集一致）。
5. 派生结果与 frontmatter 现值不同 → **覆盖**，并写审计事件
   `DERIVE_SOURCE_REFS changed=<old> → <new>`（PR diff 里可直接复核谁改了声明范围）。
6. `TOUCH` 块为空 → 按 `specmeta` 既有逻辑拒绝批准（不写 frontmatter，保持 fail-closed）。

#### 精度损失（诚实披露）

带 glob 的条目降级成目录后**范围变宽**：声明 `core/src/ac.rs` 与声明 `core/src/**`
派生结果相同，`doc-guard` 的 FRS004 会把 `core/src` 下**任何**改动都视为"源已改"。

这是**有意偏保守**：误差方向是「多报规格腐化」而非「漏报」。多报是安全侧失效（吵但不出事），
漏报是危险侧失效（看起来配好了、实际永远不判过期）。

**已确认（2026-10-04，与 `doc-guard` 侧）**：FRS004 对 `source_refs` 的元素**一律**按
「目录前缀 + 向下递归」理解，文件路径元素不按精确匹配。

这条确认改变了一处实现：原先打算「无通配符的条目原样保留」是**错的** ——
`cli/src/cli.rs` 原样写进去，FRS004 会去找以 `cli/src/cli.rs/` 为前缀的改动，
**永远匹配不到**，等于一份静默失效的声明。故降级改为无条件。

#### 与 P0 的叠加

并发改动同样在 `review()` 里插代码，而 §0.1 的 P0-1 也要改 `review()`。
两处**必须由同一人合并完成**，否则容易只改一处、把子串匹配的漏洞留下。


## 4. 改动清单

| 文件 | 符号 / 位置 | 改动 |
| --- | --- | --- |
| `core/src/issue.rs` | 新增 | `Severity`（自 `idcheck` 迁入）+ 通用 `Issue` |
| `core/src/idcheck.rs` | `Severity`、`has_errors` | 改 `pub use crate::issue::Severity;` |
| `core/src/ac.rs` | 新增 | `parse` / `lint` / `check` + table-driven 单测 |
| `core/src/touch.rs` | 新增 | `declared` / `glob_match` / `staged_files` / `diff_files` / `check` + 单测 |
| `core/src/lib.rs` | 模块声明 | 加 `issue` / `ac` / `touch` |
| `core/src/requirement.rs` | `section_of` | 抽 `section_span`；`section_of` 基于它，对外行为不变 |
| `core/src/requirement.rs` | `review` | **P0**：标记行判据 `contains` → `starts_with("<!-- GATE:STEP")`；**P2**：testplan 审批前跑 `ac::lint`，solution 审批前校验 TOUCH 非空 |
| `core/src/requirement.rs` | `head_line` / `step_line` | **P0**：同上收紧（子串 → 标记语法前缀） |
| `core/src/gate.rs` | `gate_lines` | **P0**：`contains` → `starts_with("<!-- GATE:")`；**P1**：纳入 `GATE:AC` / `GATE:TOUCH` 标记行（防删块） |
| `core/src/requirement.rs` | `render` / `TESTPLAN_BODY` / `SOLUTION_BODY` | 模板加 `GATE:AC` / `GATE:TOUCH` 骨架与示例 |
| `core/src/gate.rs` | `gate_lines` | 纳入 `GATE:AC` / `GATE:TOUCH` 标记行（防删块） |
| `core/src/gate.rs` | `HOOK_TOUCH_SH_REL`（新常量）、`install` | 落盘 + 执行位 |
| `core/src/gate.rs` | `PRE_COMMIT_BLOCK` | 追加 touch 段（含 fail-closed） |
| `core/src/gate.rs` | `verify_install` | 新增两项缺口校验：①"pre-commit 已含 touch 段"（否则 L2 静默缺口）；②"`.gates/ci/req-guard-ci.yml` 含 `touch-check --base` 与 `ac check --all` 两步"（否则 L3 静默失效，见 §3.6） |
| `core/src/gate.rs` | `REQ_GUARD_YAML` + 读取器 | 加 `touch.{exempt,scope,reapprove}`；读取器照抄 `archive_after_days` 的块级零依赖解析 |
| `cli/src/cli.rs` | `Action`、`"ac"`/`"touch"` 分支、`validate`、`help` | 新子命令 |
| `cli/src/main.rs` | `run()` 的 match | `Action::Ac` / `Action::Touch` 臂 |
| `templates/ci/req-guard-ci.yml` | 末 | 加 `ac check --all` 与 `touch-check --base` 两步 |
| `scripts/verify_gate.py` | `CASES` / `verify_pre_commit_fail_closed` | 新增 touch 场景 + "pre-commit 含 touch 段"断言 |
| `.cnb.yml` / `.github/workflows/ci.yml` | 自举段 | 补 `ac check` / `touch-check` 断言 |
| `docs/规范/AI工具合规保证规范.md` | §2.2 / §4 / §6 | **P4 落地时**同步（门禁能力边界变化，按 `docs/README.md` §5.4） |

**明确不改**：`HOOK_SH` / `HOOK_PS1`（新校验不进 AI 写路径）、`core/Cargo.toml`（零依赖铁律）。

---

## 5. 分阶段落地（每阶段独立可合、可回滚）

| 阶段 | 内容 | 风险 |
| --- | --- | --- |
| **P0** 先行修复 ✅ | §0.1 两个 P0：`review` / `gate_lines` / `head_line` / `step_line` 的标记行判据由 `contains` 收紧为 `starts_with("<!-- GATE:…")`，+ U-16/U-17/E-06 | 低（改动面小、语义收紧），但**必须先于 P1** |
| **P1** 格式契约 ✅ | `section_span` + `issue.rs` 抽取 + 模板骨架 + `gate_lines` 纳入新标记行 | 极低（无新命令、无新拦截） |
| **P2** AC 校验 ✅ | `ac.rs` + `ac check` + **审批挂钩** + CI 一步 + 单测 | 低（存量迁移需先落 `ac init`） |
| **P3** TOUCH 契约 ✅ | `touch.rs`（含 `glob_match`）+ `touch --declare` + 单测。此时**只读不拦** | 低 |
| **P4** TOUCH 拦截 ✅ | 独立脚本 + `PRE_COMMIT_BLOCK` + `verify_install` + `verify_gate.py` 新场景 + `--base` L3 + CI | 中（误报面最大，须先跑只读期） |
| **P5**（可选）追溯矩阵 | AC 编号 + TOUCH glob 已是 join key，加只读 `req-guard trace <REQ-ID>` | — |

---

## 5bis. `ac check` 实现计划（本轮不写代码，先把规格钉死）

### 5bis.1 为什么判定必须落在 core，不能落脚本

「自动格式检测」最自然的做法是再写个 Python 脚本 —— **本项目明确拒绝**：
判定逻辑只有一份在 core（`core/src/lib.rs` 模块文档），脚本只负责取参与渲染。
判定散落到第二处的那一刻起，它就与 core 版本漂移；而门禁最坏的失效模式
不是"报错"而是"看着在拦、其实没拦"。若用脚本，则 CI 跑 core 版本、本地跑脚本版本，
两者行为不同却无从察觉。故：`ac::lint` 是唯一判定，脚本与 CI 只调 `req-guard ac check`。

### 5bis.2 已就位与待建

| 项 | 状态 |
| --- | --- |
| `core/src/issue.rs`（`Severity` 迁入，供三类检查共用） | ✅ 已落地 |
| `core/src/idcheck.rs` 改 `pub use crate::issue::Severity` | ✅ 已落地（对外 API 未破） |
| **P0** `is_marker_line()` + `review`/`head_line`/`step_line`/`set_head_status`/`gate_lines` 收紧 | ✅ 已落地（含 4 条回归） |
| `requirement::section_span()`（严格分段，返回 `Option` 行区间） | ✅ 已落地 |
| 模板骨架（`GATE:AC` / `GATE:TOUCH`） | ✅ 已落地 |
| `core/src/ac.rs`（`parse` / `lint` / `check`，A1–A12 + 17 个 kind） | ✅ 已落地 |
| `cli` 的 `ac check [<ID>] [--all]` 子命令 | ✅ 已落地 |
| `review()` 审批挂钩（testplan 必须 AC 合规） | ✅ 已落地 |
| CI 模板 `ac check --all` 一步 + `.cnb.yml` 自举断言 | ✅ 已落地 |
| `core/src/touch.rs`（`declared`/`glob_match`/`staged_files`/`diff_files`/`check_changes`/`declare`/`to_source_refs`，7 个 kind） | ✅ 已落地 |
| `touch-check [--base <ref>]` 与 `touch --declare --glob <p>` | ✅ 已落地 |
| 独立脚本 `.gates/hooks/req-guard-touch-check.sh` + `PRE_COMMIT_BLOCK` 两段 + `verify_install` 三项缺口 | ✅ 已落地 |
| `verify_gate.py` 场景 19–26 + CI 模板 `--base` 一步 | ✅ 已落地 |
| **`source_refs` 派生回写 frontmatter**（§3.8 写侧：`ensure_touch_declared` + `set_frontmatter_list`；`ensure_source_refs` 已被取代并删除） | ✅ 已落地 |

### 5bis.3 判定分层（决定可测性）

```
scan_section()            ← 纯函数：段正文 → 标记定位 + 条目 + 问题
  ├─ 定位 GATE:AC 标记对           （A1）
  ├─ 逐条扫描编号行                （A2/A3/A11/A12/BadIdFormat）
  ├─ 收集子句行归属到条目          （A4/A6）
  ├─ 条目内一致性                  （A5/A9/A10）
  └─ 块外编号扫描                  （A7）
lint(section, first_line)  ← 纯函数：以上全部，返回 Vec<AcIssue>
check(root, target)       ← 薄壳：读文件 → section_span → lint
```

`lint` 必须是纯函数（入参一段正文，出参问题列表），A1–A12 才能用 table-driven 单测覆盖，
不需要临时目录、不需要 git 仓库 —— 这是它可测的唯一前提。

### 5bis.4 关键实现约束（都是实测踩出来的）

| 约束 | 原因 |
| --- | --- |
| 段落定位用 `section_span` 的 `Option`，**不得**用 `section_of` | `section_of` 定位失败时回退整篇（保界面不白屏）；拿回退整篇去判 AC 会**误判通过** |
| 子句识别：ASCII 关键词（`given`/`when`/`then`）可省略冒号；**中文别名必须跟 `：`** | 否则 `- 当前状态…` 会被误判成 `当` 子句 |
| A10 匹配前**剥离行内代码** | 验收 A10 本身的 AC 必须引用违规写法；约定引用样例一律用 `` ` `` 包起 |
| 条目编号行**独占**该行 | 标题行混入子句会污染 Markdown 渲染与目录 |
| 只认以 `<!-- GATE:…` 开头的行是标记行 | §0.1 的 P0：子串匹配会改写正文 / 误判状态行 |
| `Severity` 从 `issue` 取，不本地复制 | 同词不同义会让"错误"在三处含义漂移 |

### 5bis.5 CLI 与渲染

```
req-guard ac check              # 默认：全部未归档清单
req-guard ac check REQ-001      # 指定清单
req-guard ac check --all        # 含归档区（审计用，只读）
```

渲染按 §2.4 的输出格式契约，`message` 统一含：清单 ID、AC 编号（若有）、1-based 行号、缺失项名、修复指引。
**Error 行全部排在 Warn 之前**；仅 Warn 时退出码 0 且一条不丢（门禁唯一的静默降级口，必须可验收）。

---


## 6. 测试矩阵

### 6.1 门槛：枚举覆盖，**不用覆盖率工具**

本项目 core 零依赖、CI 只有 fmt/clippy/test/`verify_gate.py`/build —— **没有任何覆盖率测量手段**
（无 tarpaulin / llvm-cov / grcov / kcov）。因此"行覆盖 ≥ 90%"是一条**不可判定的门槛**，
与本需求"消灭不可判定验收标准"的立项理由直接矛盾，不得写入验收。

改用**零依赖、CI 现成可跑**的枚举覆盖门槛：

> 一条单测遍历 `AcIssueKind` 与 `TouchIssueKind` 的**全量变体**，断言每个变体至少被一个
> case 命中（测试内维护一张 `kind → 用例名` 映射表）。**新增 kind 而忘了写测试时该用例必红。**

这条门槛同时解决两件事：可机械判定，且把"规则与 kind 一一对应"这条契约本身也纳入强制。

性能门槛（可测）：1000 条声明 × 200 个暂存文件，`touch-check` 匹配耗时
**本机 < 200ms、CI < 2s**（用已存在的计时手段断言，不引 benchmark 框架）。

### 6.2 `core` 单测 —— AC

合法三行式；中文别名（含「中文别名必须跟冒号」这条防误判）；缺 `Given`（`MissingClause`）；顺序颠倒（`OrderClause`）；
跳号（`SeqGap`）；重复（`DuplicateId`）；非 `AC-` 前缀（`BadIdFormat`）；空子句（`EmptyClause`）；
块外编号（`OutsideBlock`）；未闭合（`UnclosedBlock`）；缺块（`MissingBlock`）；零条目（`NoItem`）；
中英混用（`AliasMix`）；Then 无数字（`Unmeasurable`）；编号行混入子句（`DirtyIdLine`）；
Given 外部指代（`NonSelfContained`）；块内混入分组标题（`StrayHeading`）；
单行式残留（`InlineEntry`）；编号行混入子句（`DirtyIdLine`）；
`## 3` 标题写坏 → `section_span` 返回 `None` 且判 `SectionNotFound`。

### 6.3 `core` 单测 —— TOUCH

`glob_match` 表驱动（`core/src/**` 命中 `core/src/gate/x.rs`；`core/src/*.rs` 不命中子目录；
`**/*.rs` 命中根级；`?` 单字符）；声明 ⊇ 实际（命中 / 超出各一）；`.gates/**` 豁免；
`..` 逃逸（`BadPath`）；无块（`MissingBlock`）；空块（`EmptyDeclaration`）；
并集 vs `strict`；`done` 清单不参与并集；第 2 段标题写坏（`SectionNotFound`）。

### 6.4 脚本层场景矩阵（`scripts/verify_gate.py`，固定场景表）

`verify_gate.py` 是**唯一**能验证 sh 脚本层的手段（§3.4 的判定挂在脚本上，不在 Rust 里），
故场景必须定死，不允许只写"新增 touch 场景"：

| 编号 | 场景 | 期望退出码 |
| --- | --- | --- |
| 19 | touch 脚本缺失（pre-commit 调不到） | 1 |
| 20 | `HOOK_STAGED_FILES` 含未声明文件 | 1 |
| 21 | `HOOK_STAGED_FILES` 全为已声明文件 | 0 |
| 22 | 未设 `HOOK_STAGED_FILES` → 回落 `git diff --cached` | 按回落结果 |
| 23 | 非 git 仓库执行 pre-commit | 0 + stderr 说明 |
| 24 | `PRE_COMMIT_BLOCK` 含 touch 段（正则抽取断言） | 断言存在 |
| 25 | `.gates/**` 已暂存（豁免） | 0 |

场景 20/21/23 依赖已实现的 `touch-check`，故随 P4 一并落地；P1–P3 阶段这些场景标记为跳过。

**CI 自举**：`.cnb.yml` 与 `.github/workflows/ci.yml` 的 gate-selfcheck 补 `ac check` / `touch-check` 断言。

---

## 7. 风险与边界（诚实披露）

| 风险 | 处置 |
| --- | --- |
| **误报**是头号死因（门禁被习惯性绕过 = 没有门禁） | 默认并集口径、exempt 最小集、`--declare` 有合法出路、拦截信息一次给全三张牌；P3→P4 之间留只读期 |
| **晚失败**：AI 写完 40 个文件才在 pre-commit 被拦 | 这是墙的性质而非 bug；靠 `touch --declare` + 打回 solution 重审把它变成"改方案"循环而非"绕过"循环 |
| 强度依赖 `auth.level`：`touch --declare` 走 `ensure_human`，L0（无审批锁）下 AI 可自行扩范围 | 与 `core/src/auth.rs` 的信任模型同源，文档写明，不额外兜底 |
| 存量清单迁移 | 先落 P1 的 `ac init`，再开 P2 的 Error |
| 追溯矩阵仍是零 | 本次只把 join key（AC 编号 + 文件 glob）落到文档里；矩阵本体不存在，别在需求描述里当已有能力 |
| **`source_refs` 派生的精度损失**（§3.8） | 元素按「目录 + 向下递归」理解（**已与 doc-guard 侧确认**），故 glob 与文件路径一律降级成目录 → 声明范围变宽 → FRS004 多报。有意偏保守（误差在安全侧）。无法表达的声明（`**/*.rs` 这类通配符在首段的）**丢弃并告警**，不静默丢 |
| **并发改动的合流风险** | `specmeta` 那条线正在改 `review()` / `gate.rs`，本方案的 P0-1 与派生逻辑也要改同一批文件。**必须同一人合并完成**，否则易只改一处、把 `contains("GATE:STEP")` 的子串匹配漏洞留下 |
| 单行式废除后仍可能有历史残留 | A12 `InlineEntry` 会把残留判为 Error；存量清单升级前需先跑一遍迁移（与 §2.7 的 `ac init` 同一时机） |

---

## 8. 与追溯矩阵的关系

未来矩阵（AC × 文件 × 测试）的两个 join key 已由本文定义：

- **AC 编号**（A3 保证连续、无重复、无跳号）→ 行键；
- **TOUCH glob**（T2/T3 保证可归一）→ 列键。

因此本文的所有编号规则都不是格式洁癖，而是**为可 join 而设**。
矩阵本体（`req-guard trace` 只读渲染）留待 P5。
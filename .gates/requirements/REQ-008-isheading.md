---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-04
source_refs: [core/src, scripts, docs/规范]
---

# REQ-008 段落边界判定排除代码围栏（is_heading 缺陷修复）

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-008 status=approved created=2026-10-04_19:39:24 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_19:43:18 sum=a4167c631cb1cf3f5038c8d446777869b30fe4a28c67ebac82f81af3f4f845ba -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_19:43:27 sum=f5edd57359bf7c1853f279bcec9c61ac667ff70d99867f3bff3d30c73c569527 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-04_19:43:44 sum=64d9bc8213ecafc320676758dfcfd14e36d582effa4d0b8acb21f7e3ae7c8070 -->

## 1. 需求分解

### 背景与问题

`section_span` 是清单分段的唯一实现：它决定「哪几行属于第 N 段」，因而决定了
`GATE:TOUCH` 声明读哪一段、内容摘要覆盖哪一段、`EmptySection` 统计哪一段。
它的边界判定 `is_heading` 是：

```rust
fn is_heading(line: &str) -> bool { line.trim_start().starts_with("## ") }
```

**判据只有「`trim_start()` 后以 `## ` 开头」**，因此：

1. **代码围栏内的 `## xxx` 会被当成真标题**。而文档里展示格式示例是极常见的写法 ——
   例如起草 REQ-007 时在技术方案段写了「草稿区分隔格式形如 `## solution`」，结果
   `touch-check` 报 `GATE:TOUCH` 块「消失」。
2. **`trim_start()` 让缩进的 `##` 也命中**，围栏内的示例几乎总是缩进的（放在列表项下）。

症状极具迷惑性：声明块、摘要、`EmptySection` 判定会**一起**指向错误的段落，
而清单本身看起来完全正常。报错信息（`MissingBlock`）指向「你没写 TOUCH 块」，
但作者明明写了 —— **报错与真实原因完全无关**，排查成本极高。

这不是假想：REQ-007 起草当场触发，四条判定里只有 `touch-check` 报了错，
另外三条**静默地判错了段落**（摘要覆盖范围、`EmptySection` 统计范围）。

### 目标

- **G1** `is_heading` 排除**代码围栏内**的行（``` 与 ~~~ 两种围栏、含 ```` ```` ````
  四反引号形式），且不改动任何其他判定行为。
- **G2** 段落边界判定不受**缩进**影响地保持现状（缩进的 `## ` 仍算标题 ——
  这是既有行为，可能有人靠它做嵌套展示）。
- **G3** 围栏内若**未闭合**（文档写了 ```` ``` ```` 但没有收尾），按「围栏持续到文末」
  处理（fail-closed：宁可少切一段，也不把围栏内容当标题）。

### 非目标

- **N1** 不做完整 CommonMark 解析。围栏的**嵌套/缩进变体**（如列表项内的 ```` ``` ````）
  不追求逐字节正确，只要求「不会因为常见写法而误判」。
- **N2** 不改 `mask_html_comments` 的既有语义（REQ-004 的交叉引用已依赖它），
  只把围栏逻辑**提取为可复用函数**并让 `section_span` 一并使用。
- **N3** 不改清单格式，不要求任何存量清单迁移。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | 提取 `mask_fenced`（围栏掩码为等长空格，保行号）：支持 ``` 与 ~~~、变长围栏 | 2h |
| T2 | `is_heading` 改用掩码后的行判定 | 1h |
| T3 | `section_span` 对全文掩码后再切段 | 1h |
| T4 | `touch.rs` 改用同一份 `mask_fenced`（去重） | 1h |
| T5 | 单测：围栏内 `##`、缩进围栏、未闭合围栏、四反引号、既有行为不变 | 2h |
| T6 | `verify_gate.py` 场景 + 规范文档 | 1h |

合计 8h。T1+T2 是最小可交付。

### 影响范围

- **模块**：`core/src/requirement.rs`（`section_span` / `is_heading`）、
  `core/src/section.rs` 或新模块（掩码函数）、`core/src/touch.rs`（去重）、
  `scripts/verify_gate.py`、规范文档。
- **配置**：无。
- **清单格式**：无变更。
- **接口 / 数据表 / 外部 API**：无。
- **向后兼容**：**这是本需求最大的风险点** —— 若某份存量清单**依赖**了围栏内的
  `## ` 被当成标题（即靠这个错误行为划分段落），修复后边界会变。由 T5 的
  「既有行为不变」用例与 REQ-001~007 的回归兜底。

### 本次事故暴露的相邻缺陷（不在本需求范围）

`mask_html_comments` 把 HTML 注释与围栏**混在一个函数**里（靠 `in_fence` 标志），
两者语义不同、翻转条件也不同。本需求只做提取与复用，不拆语义。
另：`is_heading` 对 `#### 标题` 的处理（`#### ` 不以 `## ` 开头 → 不算边界）符合
预期（只认二级），但未显式测试锁定。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量）。

## 2. 技术方案

### 总体思路

**把围栏内容掩成等长空格，再判标题。**

`touch.rs` 里已有 `mask_html_comments`，它同时处理围栏与注释（`in_fence` 标志）。
把其中的围栏部分**提取**为独立函数 `mask_fenced`，`section_span` 与
`touch.rs` 共用一份。

为什么是「掩码」而不是「解析」：掩码保持**字符位置与行号不变**，而 `section_span`
的返回值 `(start, end)` 就是行号 —— 一旦掩码改变了行数，所有错误信息里的行号
就会指向错误位置（REQ-004 的交叉引用已经因为这个理由选择掩码而非删除）。

### 关键设计

1. **掩码保留换行**：围栏内的非换行字符换成空格，换行保留。行号因此不变。

2. **两种围栏都支持**（``` 与 ~~~），且**变长围栏**（四反引号包裹含三反引号的示例）：
   开围栏取连续反引号/波浪号的**长度**，闭围栏要求长度**不少于**开围栏。
   只认长度 3 会让「文档里展示三反引号的文档」再次切错边界 —— 与本缺陷同类。

3. **未闭合围栏按持续到文末处理**（G3）：fail-closed。若按「未闭合则不生效」处理，
   少了一个收尾标记就会让围栏内的所有 `## ` 变回标题 —— 正是要消灭的形态。

4. **只判行首缩进后的 `## `**（G2 保持现状）：不动既有行为，降低风险。

5. **去重**：`touch.rs` 的 `mask_html_comments` 改为调用 `mask_fenced`，
   两处不再各写一份围栏逻辑。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
core/src/requirement.rs
core/src/section.rs
core/src/touch.rs
scripts/verify_gate.py
docs/规范/AI工具合规保证规范.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：见影响范围的风险点。缓解手段是回归测试 + 五条既有需求全绿。
- **性能**：`section_span` 每次调用扫一遍全文并分配一份掩码副本。清单是
  数百行量级、`section_span` 每次判定调 1~3 次 —— 可忽略。若后续成为热点，
  可缓存掩码结果，**不在本需求范围**。
- **安全**：判定变严格（少切错段落），方向是 fail-closed。

### 风险点与回滚方案

- **风险 1（中）**：有存量清单靠围栏内 `## ` 划分段落 → 修复后边界变化。
  缓解：T5 用例锁定既有行为 + REQ-001~007 回归；万一发现，`git revert` 即回退。
- **风险 2（低）**：变长围栏的边界条件（长度比较）写错 → 引入新的切错。
  缓解：四反引号用例专门锁它。
- **回滚**：单文件核心改动，`git revert` 即可，无数据迁移。

## 3. 测试计划

### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | 围栏内的 `## ` 不算段边界 | `section_span` 返回的段内含该行 |
| U2 | 缩进围栏内的 `## ` 同上 | 同 U1 |
| U3 | 未闭合围栏持续到文末 | 围栏后的 `## 3.` 不被当作新段 |
| U4 | 四反引号包裹三反引号 | 内层三反引号不结束围栏 |
| U5 | `~~~` 围栏 | 同样被排除 |
| U6 | 围栏外的 `## ` 仍算边界 | 既有行为不变 |
| U7 | 掩码保持行数与行号 | 掩码后行数等于原文行数 |
| U8 | `touch.rs` 与 `requirement.rs` 共用同一掩码 | 同一输入两处结果一致 |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 技术方案段含围栏示例 `## solution` 的清单 → `touch-check` | exit 0（TOUCH 块被正确读到） |
| E2 | 同一清单 → `verify-content` | exit 0 且摘要覆盖正确段落 |
| E3 | 同一清单 → `ac check` | exit 0，`EmptySection` 统计范围正确 |

### 边界 / 异常场景

- **B1** 围栏紧邻 `## 2.` 标题行（无空行）→ 不吞掉真标题。
- **B2** 围栏标记有缩进（列表项内）→ 视为围栏开始。
- **B3** 文档以围栏开头 → 文首即进入围栏态。
- **B4** 连续两个围栏 → 各自独立判定。
- **B5** 围栏语言标注（```rust）→ 不影响判定。

### 回归范围与影响面

- `core` 全量单元测试；`verify_gate.py` 全部场景。
- REQ-001~007 的 `check` / `ac check` / `touch-check` / `verify-content` / `seal`
  结论一律不变。

### 验收门槛

- `cargo fmt --all --check` 与 scoped `clippy -- -D warnings` 零输出。
- `cargo test -p req-guard-core -p req-guard` 全绿。
- `python3 scripts/verify_gate.py` 全部场景通过。

<!-- GATE:AC -->
### AC-001
- Given: 一份清单，其技术方案段内有缩进三反引号围栏，围栏内含一行 `## solution`
- When: 调用段落定位求第 2 段的行号区间
- Then: 返回的区间包含围栏内那一行，且终点在第 3 段二级标题之后

### AC-002
- Given: 一份清单，其技术方案段内有未闭合的三反引号围栏
- When: 调用段落定位求第 2 段的行号区间
- Then: 围栏之后的 `## 3. 测试计划` 行号大于该区间终点，即未被当作第 2 段的一部分

### AC-003
- Given: 一份清单，其正文用四反引号包裹一段含三反引号的示例
- When: 调用段落定位求段区间
- Then: 内层三反引号未被当作围栏结束，示例内的 `## ` 行未被当作段边界

### AC-004
- Given: 一份清单，其正文含 `~~~` 围栏且围栏内有 `## ` 行
- When: 调用段落定位求段区间
- Then: 该围栏内的 `## ` 行位于返回区间内，未成为边界

### AC-005
- Given: 一份不含任何围栏的合法三段清单
- When: 分别调用三次段落定位
- Then: 三次返回的区间与修复前基线逐项相同，差异项数为 0

### AC-006
- Given: 一段含三反引号围栏的文本
- When: 对其做围栏掩码
- Then: 掩码结果行数与原文行数之差为 0，且逐行字符数与原文之差的绝对值之和为 0

### AC-007
- Given: 一份清单，其技术方案段内有围栏示例 `## solution`
- When: 执行 `req-guard touch-check`
- Then: 退出码为 0，MissingBlock 问题数为 0

### AC-008
- Given: 与 AC-007 相同的清单
- When: 执行 `req-guard verify-content`
- Then: 退出码为 0，ContentChanged 问题数为 0

### AC-009
- Given: 实现完成后的 core 源码全量
- When: 检索围栏掩码的实现
- Then: `mask_fenced` 的定义出现次数为 1，即两处共用一份而非各写一份

### AC-010
- Given: 五份已批准的清单 REQ-001 至 REQ-005
- When: 逐份执行 `req-guard ac check`
- Then: 五份的硬伤数均为 0，与本需求实施前一致
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-04_19:43:18 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-04_19:43:27 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-04_19:43:44 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

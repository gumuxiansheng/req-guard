---
doc_type: proposal
tier: critical
owner: -
review_policy: codebound
verified_at: 2026-10-10
source_refs: [core/src, .gates/hooks, packaging/docs]
---

# REQ-022 门禁脚本支持 REQ_GUARD_BIN 显式指定二进制路径

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-022 status=approved created=2026-10-10_19:11:31 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-10_19:19:14 sum=f796953f5b225b174c87bb5bd7578ea5a7732658b69afc4913c8e586ab7d5154 -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-10_19:19:33 sum=d7a962d4a5797753716ae72a0b2c47aa562c3c371610b5b6d4f92a0398227bf3 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-10_19:19:40 sum=d6cae3ef2540a637b98776c952e05ee46e616cac311af00c75f52b6656aa08aa -->

## 1. 需求分解

### 背景与问题

在 VSCode 里点提交被硬拦：

```
⛔ 拦截：无法裁决（req-guard 不在 PATH），本次写/提交已被阻止。
判定在 core，缺二进制即无从判定 —— fail-closed，不猜。
```

同一份仓库、同一份脚本，在终端里 `git commit` 却是正常的。三条实测（今天，可重跑，非代码走读推测）：

| # | 复现命令 | 实测结果 |
| --- | --- | --- |
| **1** | `which req-guard` | `not found`；`target/release/req-guard` 也不存在（**二进制压根没装**）。`cargo build --release -p req-guard` + 软链 `~/.cargo/bin/req-guard` 后，`req-guard --version` → `0.1.7`，终端侧恢复 |
| **2** | `launchctl getenv PATH` | 输出为空 → GUI 应用回落 launchd 默认 `PATH`（`/usr/bin:/bin:/usr/sbin:/sbin`），**`~/.cargo/bin` 不在其中**。VSCode 由 launchd 派生，它 spawn 的 `git` 继承的就是这份 PATH |
| **3** | 读 `.gates/hooks/req-guard-check.sh` 第 59 行 / `packaging/scripts/selfcheck.sh` 第 33 行 | 门禁脚本只有 `command -v req-guard`，**不认** `REQ_GUARD_BIN`；而 selfcheck 脚本**认**。同一个变量名，两处语义不一致 |

根因链：

1. 门禁脚本把「怎么找到二进制」完全托付给 `PATH`；
2. `PATH` 是**进程继承**的，GUI 应用与终端拿到的不是同一份 —— 这是 macOS/Windows 桌面环境的既有事实，任何靠 `PATH` 定位的方案都要撞；
3. 脚本没有第二条定位通道，于是这类环境下用户只剩 `git commit --no-verify` 一条路，而那是**逃逸阀**，不是解法（绕一次，门禁就少保护一次）。

补一条通道即可解，且是低成本：**门禁脚本支持 `REQ_GUARD_BIN` 环境变量**，让「二进制在哪」由环境显式声明，不再只靠继承来的 `PATH`。

### 目标

- **G1** 脚本支持 `REQ_GUARD_BIN`：指向一个可执行文件（`-x`）时，**优先于** `PATH` 使用它。
- **G2** `REQ_GUARD_BIN` 已设置但指向的文件**不可用**（不存在 / 不是文件 / 无执行位）→ fail-closed 拦截，并把该路径原样打进 stderr，便于一眼看出拼错在哪。
- **G3** POSIX 与 PowerShell 两份脚本**逐条镜像**（现状就是这样的一一镜像关系，本需求不打破它）。
- **G4** 「找不到二进制」的拦截文案补一行提示：可用 `REQ_GUARD_BIN` 指定路径。
- **G5** **默认行为逐字不变**：未设置该变量时，判定结果、退出码、审计行与今天完全一致。

### 非目标

- **N1** 不改 `install`：不自动安装二进制、不修改系统 `PATH`。`install` 的语义仍是「把门禁装进仓库」，装二进制是人的事（这也是本次不在 `install` 里动手的原因）。
- **N2** **不给脚本追加 `PATH` 兜底目录**（如 `$HOME/.cargo/bin`）—— 安全理由见设计 2，这条是被**明确否决**的，不是遗漏。
- **N3** 不做仓库内二进制回退（`./req-guard`、`target/{release,debug}/req-guard`）。那要求把二进制带进仓库/CI，是另一件事（其查找顺序 `core/src/gate.rs` 的 `resolve_gate_binary` 里已有，本次不动它）。
- **N4** 不改 `packaging/scripts/selfcheck.{sh,ps1}`：它们早已支持该变量，本需求不扩张它们的校验强度。
- **N5** 不改 `deny` 包装与 `touch-check`：二者不自行定位二进制（`deny` 只转退出码；`touch-check` 另有一套脚本）。

### 子任务拆解（编号 + 预估工时）

| 编号 | 任务 | 预估 |
| --- | --- | --- |
| **T1** | `HOOK_SH`：抽出二进制定位段（变量优先 → `PATH` → 不可用），第 0 段与第 1 段统一改用解析结果 | 0.5h |
| **T2** | `HOOK_PS1` 逐条镜像（含 `$env:REQ_GUARD_BIN` 与存在性判定） | 0.3h |
| **T3** | 拦截文案：不可用分支分两种措辞（变量不可用 / 不在 PATH），并补提示行 | 0.2h |
| **T4** | 单测 U-01…U-08：常量层断言 + **真跑脚本**的放行/拒绝/兜底三类 | 1.0h |
| **T5** | 本仓自举：`install` 重生成 `.gates/hooks/*`，实跑变量生效与不设变量不回归 | 0.3h |
| **T6** | 文档：`packaging/docs/常见问题.md` 排查表补一条 | 0.2h |
| **T7** | 回归：`cargo test --workspace` / `clippy -D warnings` / `ac check` / `install --verify` | 0.4h |

合计约 2.9h。

### 影响范围

| 层 | 位置 | 影响 |
| --- | --- | --- |
| 脚本常量 | `core/src/gate.rs`（`HOOK_SH` / `HOOK_PS1`） | 新增定位段；第 0/1 段调用点改用解析结果 |
| 落盘产物 | `.gates/hooks/req-guard-check.{sh,ps1}` | 由 `install` 重新生成（本仓自举即覆盖） |
| 文档 | `packaging/docs/常见问题.md` | 排查表补 `REQ_GUARD_BIN` 一条 |
| 判定 | `core/src/resolve.rs` | **不改**：裁决逻辑与「二进制从哪来」无关 |
| 前端 | `cli` / `tui` / `gui` | **不改** |

### 验收标准（可度量、可判定）

- 未设变量时，既有用例（含「不在 PATH 恒拦」）逐字不回归；
- 变量指向可用二进制 → 走它（真跑验证，非文本比对）；
- 变量指向不可用路径 → 退出码 1，stderr 含该路径字面量；
- 两份脚本的 `REQ_GUARD_BIN` 分支数量一致（镜像不被打破）；
- `cargo test --workspace` 全绿，`cargo clippy --workspace --all-targets -- -D warnings` 零告警，`req-guard ac check` 对本清单零 Error。

## 2. 技术方案

### 总体思路

一句话：**脚本顶部先解析出「用哪个二进制」（变量优先 → `PATH` → 定位失败），后面所有调用点统一用这个结果**；定位失败的情形细分为「没配」和「配了但不可用」，两种都 fail-closed，只是报错文案不同。

### 关键设计

#### 设计 1：定位序与「配了就必须可用」

| 情形 | 行为 | 防的是什么 |
| --- | --- | --- |
| 未设置变量 + `PATH` 命中 | 用 `PATH` 上的（**今天的行为**） | — |
| 已设置 + 指向可执行文件 | 用它（**优先于 PATH**） | PATH 上的旧版本覆盖显式指定的新版本 → 判定口径漂移 |
| 已设置 + 指向不存在 / 目录 / 无执行位 | **拦截**，stderr 打出该路径 | 拼错路径却静默回退 → 「我明明指定了」与「实际跑的是谁」不一致，排查成本最高 |
| 都不可用 | 拦截（**今天的行为**） | — |

为什么「配了但不可用」不回退 `PATH`：显式声明是**环境契约**。契约落空时静默改用另一份二进制，会让同一台机器上的裁决结果取决于「PATH 里恰好有什么」—— 而版本漂移直接改变裁决口径（`check` 的规则随版本演进）。fail-closed 与项目其余部分一致：宁可拦下来让人修环境，也不猜。

#### 设计 2：为什么**否决**「脚本内追加 PATH 兜底」

直觉方案是在脚本开头加一行 `PATH="$PATH:$HOME/.cargo/bin"`，一行就能解决本次的 VSCode 症状。**不做**，理由是它是新的攻击面：

- 门禁的安全性建立在「被调用的 `req-guard` 是可信的那一个」之上。往查找链里塞**用户可写目录**，等于允许任何能写该目录的主体（含被约束的 AI）放一个同名文件替换裁决者 —— 这正是本项目定义的最坏失效「看着在拦、其实没拦」；
- 而且它只是把「PATH 不够全」这个环境问题的症状压下去：GUI 场景的正解是**配置环境**（`launchctl config user path` / 从终端启动 / CI 里 `PATH` 显式写全），不是让门禁去猜路径放哪。

本需求因此只开一条**显式、可审计**的通道：不猜目录，只认人写明的路径。

#### 设计 3：只接受「可执行文件路径」，不接受命令串

`REQ_GUARD_BIN="req-guard --flag"` 这类带参形式在 POSIX 侧 `-x` 判不过（按不可用处理），在 PowerShell 侧 `Test-Path` 也判不过 —— 变量不会被拼进 shell 求值。目的：环境变量不成为注入面。

#### 设计 4：第 0 段的正则兜底必须保留（顺序不动）

现状第 0 段是「二进制可用 → 交给 `req-guard hook-check` 真解析 JSON；不可用 → 退回正则只保护 `*.comments.md`」。若把「不可用即 `exit 1`」提到第 0 段之前，正则兜底将永远不可达，审计行会从 `BLOCK-AI-WRITE-COMMENTS` 变成 `BLOCK no-binary` —— 拦得更早，但**丢掉了「证据被篡改」这一条精确归因**。

因此定位段只产出结果，不提前 `exit`：

```sh
# 定位二进制：变量优先 → PATH；定位不到则 RG_BIN 为空。
# 空 = 不可用，但**不在本段拦截**：第 0 段要靠这份空值走正则兜底，
# 第 1 段才 fail-closed —— 与今天「二进制不在 PATH」的行为逐字一致。
RG_BIN=""
RG_BAD=""
if [ -n "${REQ_GUARD_BIN:-}" ]; then
  if [ -x "${REQ_GUARD_BIN}" ]; then
    RG_BIN="${REQ_GUARD_BIN}"
  else
    RG_BAD="${REQ_GUARD_BIN}"
  fi
elif command -v req-guard >/dev/null 2>&1; then
  RG_BIN="req-guard"
fi
```

第 0 段改为 `if [ -n "$RG_BIN" ]; then`（真解析）`else`（正则兜底，逐字保留）；第 1 段改为 `if [ -n "$RG_BIN" ]; then`（裁决）`else`（拦截，文案按 `RG_BAD` 是否非空分两种）。调用点一律 `"$RG_BIN"` 带引号（B-04 的空格路径）。

#### 设计 5：PowerShell 侧镜像

```powershell
$rgBin = ''
$rgBad = ''
if ($env:REQ_GUARD_BIN) {
  if (Test-Path -LiteralPath $env:REQ_GUARD_BIN -PathType Leaf) { $rgBin = $env:REQ_GUARD_BIN }
  else { $rgBad = $env:REQ_GUARD_BIN }
} elseif (Get-Command req-guard -ErrorAction SilentlyContinue) {
  $rgBin = 'req-guard'
}
```

Windows 没有 POSIX 执行位，故只判 `Leaf`（是文件）。**这是 POSIX 与 Windows 侧唯一的语义差**，写入残留风险第 2 条。

#### 设计 6：文案

- 变量不可用：`[req-guard] ⛔ 拦截：REQ_GUARD_BIN 指向的文件不可用（<路径>），本次写/提交已被阻止。` + 「请确认该路径存在且有执行位（POSIX）」
- 未设变量且不在 PATH：保留既有四行，末尾追加一行 `亦可用 REQ_GUARD_BIN=<绝对路径> 显式指定（见常见问题）。`
- 审计行：变量不可用记 `BLOCK no-binary-badenv`（新字面量，与既有 `BLOCK no-binary` 区分，便于台账上区分「没装」与「装了但指错」）。

#### 设计 7：两处自检判据必须跟着收敛（**实施期发现**，本次修订的唯一原因）

命令名一旦变量化，凡是「脚本里必须有 `req-guard check --stdin` 这行字」的判据都会误报。实测：

| 位置 | 实施后实测 | 性质 |
| --- | --- | --- |
| `scripts/verify_gate.py` R6_g6 | `FAIL 委托接线[req-guard check --stdin]: 不存在（期望存在）`—— 两侧 CI 都跑它，**不改即红** | Error（会让 CI 失败） |
| `core/src/gate.rs::verify_warnings` | `install --verify` 刷两条「主门禁脚本未委托 core 判定」 | 告警（不影响退出码，但**说谎**） |
| 既有单测 `hook_脚本优先调用rust解析且保留兜底` | 断言 `req-guard hook-check` 不再命中，单测红 | Error |

三处判据同步收敛为**两条**（不是简单删掉命令名）：

1. `check --stdin` / `check --staged` —— 不再写死命令名，只锁子命令；
2. 且脚本必须含 `REQ_GUARD_BIN` 或 `command -v req-guard` —— 证明它**确实在定位一个二进制**。

第 2 条不能省：只留第 1 条的话，脚本里写一句含 `check --stdin` 的**注释**就能骗过 ——
那正是 REQ-012 记下的教训「存在性 ≠ 行为」（当时在脚本顶部插一行 `exit 0`，
`install --verify` 照样报 PASS；本需求若只做第 1 条，等于把那道墙又拆一次）。

因此 `GATE:TOUCH` 在原范围之外增加 `scripts/verify_gate.py`。

### 涉及的文件与模块清单

见下方 `GATE:TOUCH` 块（精确到文件；不用 `core/src/**` 宽口径 —— 那会把本需求不碰的模块一并圈进来，正是 REQ-020 反对的做法）。

### 兼容性、性能与安全影响

- **兼容性**：未设变量时行为逐字不变。脚本由 `install` 生成，**存量仓库不重跑 `install` 就仍是旧脚本** —— 旧脚本不认变量，但行为是 fail-closed 恒拦（今天就这样），**不构成安全缺口**，只是拿不到新能力；需在文档写明「升级后重跑 `req-guard install`」。
- **性能**：定位段是常数时间，无新进程（`-x` 与 `command -v` 都是内建）。
- **安全（必须点名）**：能设置 `REQ_GUARD_BIN` 的主体，本来也能改 `PATH` —— 二者权限等价，**本需求不新增攻击面**；它新增的是**可审计性**（变量值会出现在拦截文案与审计行里，PATH 不会）。
- **core 零依赖**不变：改的是字符串常量与单测。

### 风险点与回滚方案

| 风险 | 严重级 | 缓解 |
| --- | --- | --- |
| 定位段把「不可用」提前 `exit`，吃掉第 0 段正则兜底 | **高** | 设计 4 明确不提前 `exit`；U-07 / 见 GATE:AC 中「正则兜底未被吃掉」那条用例锁死审计行字面量 |
| PowerShell 侧只判存在、不判可执行 | 中 | 写入残留风险；Windows 上指到 `.ps1` 等非可执行文件会调用失败 → 仍在第 1 段 fail-closed，不产生放行 |
| 存量仓库不重跑 `install` → 用户以为支持变量其实不支持 | 低 | 文档写明需重跑 `install`；`install --verify` 的就位检查不因此变化 |
| 变量值含空格导致调用被 split | 低 | 调用点一律 `"$RG_BIN"`；B-04 / 单测覆盖带空格路径 |
| 两份脚本镜像漂移 | 中 | U-01 / U-02 断言两侧变量分支数量一致 |
| 设计 7 的判据收敛后比原判据宽松 | 中 | 由第 2 条定位判据补上；真正的守卫是实跑语义自检（`verify_hook_behaviour` 与 `verify_gate.py` 的 R6_g7/g8），存在性快筛只负责指方向 |

**回滚**：T1–T3 是同一处常量的三个面，整体 revert 即回到今天的行为（只认 `PATH`）；T6 文档独立可 revert。

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
core/src/gate.rs
.gates/hooks/req-guard-check.sh
.gates/hooks/req-guard-check.ps1
packaging/docs/常见问题.md
scripts/verify_gate.py
<!-- /GATE:TOUCH -->

## 3. 测试计划

### 单元测试用例（编号 + 断言点）

**常量层（T1 / T2 / T3）**

- **U-01** `HOOK_SH` 含字面量 `REQ_GUARD_BIN` 且出现次数 ≥ 3（定位 / 第 0 段判空 / 第 1 段报错），且含 `RG_BAD`
- **U-02** `HOOK_PS1` 含字面量 `$env:REQ_GUARD_BIN` 且 `Get-Command req-guard` 只出现在变量为空的分支（`elseif`）—— 证明「变量优先」而非「PATH 优先」
- **U-03** 两份脚本中 `-x "$RG_BIN"`（POSIX）与 `Test-Path ... -PathType Leaf`（Windows）各出现 ≥ 1 次，且调用点均为带引号的 `"$RG_BIN"` / `"$rgBin"`

**真跑脚本（T4，写在临时目录，用 `sh` 实跑，非文本比对）**

- **U-04** 临时 git 仓库 + `install` 门禁；`REQ_GUARD_BIN` 指向一个 `exit 0` 的假二进制；无 stdin payload → 脚本退出码 **0**（变量命中并被实际调用）
- **U-05** 同上仓库，`REQ_GUARD_BIN=/tmp/definitely-not-exist` → 退出码 **1**，stderr 含字面量 `/tmp/definitely-not-exist`
- **U-06** 同上仓库，未设变量且子进程 `PATH` 被清空为 `/usr/bin:/bin` → 退出码 **1** 且 stderr 含 `req-guard 不在 PATH`（**不回归锁**）
- **U-07** 同上仓库，未设变量、无可用二进制，stdin payload 的 `file_path` 为 `<root>/.gates/requirements/REQ-001.comments.md` → 退出码 1，且 `.gates/audit/gate-audit.log` 新增行含 `BLOCK-AI-WRITE-COMMENTS`（**证明第 0 段正则兜底没被本次重构吃掉**，对应设计 4 的高危项）
- **U-08** 同上仓库，`REQ_GUARD_BIN` 指向「把 argv 追加到文件后 `exit 0`」的假二进制，stdin payload 的 `file_path` 为非 comments 路径 → 退出码 0，且记录文件含 1 行字面量 `hook-check`（**证明变量在 AI PreToolUse 场景生效**，而非只在 pre-commit 生效）

### 端到端用例（编号 + 执行步骤）

- **E-01** 本仓自举（T5）：`req-guard install` 重新生成 `.gates/hooks/*` → `REQ_GUARD_BIN="$(command -v req-guard)" sh .gates/hooks/req-guard-check.sh` 与不带变量执行的**退出码相同**（自举仓库下证明新逻辑与旧逻辑等价）
- **E-02** `REQ_GUARD_BIN=/nonexistent/req-guard sh .gates/hooks/req-guard-check.sh` → 退出码 1 且 stderr 含该路径
- **E-03** `env -i PATH=/usr/bin:/bin HOME="$HOME" sh .gates/hooks/req-guard-check.sh` → 退出码 1 且含 `不在 PATH`
- **E-04** 未设变量跑 `req-guard check --staged` 与 `req-guard install --verify`，结论与改动前一致
- **E-05** 本仓改动后 `git commit` 通过（`pre-commit` 两道钩子放行）

### 边界 / 异常 / 并发场景

- **B-01** `REQ_GUARD_BIN=""`（空串）→ 等同未设置，走 `PATH`（`[ -n ... ]` 判空在前）
- **B-02** 变量指向一个**目录** → 判不可用（`-x` 对目录为假；`PathType Leaf` 亦为假），拦截并打出该路径
- **B-03** 变量为**相对路径** → 按脚本工作目录解析（git hook 的 cwd 是仓库根）；文档建议写绝对路径，不额外做解析（避免第二处路径真相）
- **B-04** 变量值**含空格** → 调用点带引号，不得被词分割；U-03 与单测覆盖
- **B-05** Windows：变量指向 `.ps1` / `.txt`（存在但不可执行）→ `Test-Path` 通过、调用失败 → 第 1 段仍 fail-closed（**不产生放行**）；此差异写入残留风险
- **B-06** 变量指向**旧版本** req-guard → 正常执行（本需求不做版本校验；版本一致性由 `install --verify` 与 CI 承担）

### 回归范围与影响面

- `core/src/gate.rs` 全部既有用例（含脚本生成、install、verify_install 相关）必须全绿；
- `req-guard ac check` 对全部未归档清单零 Error；
- `packaging/scripts/selfcheck.{sh,ps1}` 自检项不回归（本需求不改它，但要确认它的 `REQ_GUARD_BIN` 语义与新脚本不冲突：它是「指定被测二进制」，脚本是「指定裁决二进制」，同名但作用域不同 → 写入残留风险第 3 条）；
- `req-guard install --verify` 仍 PASS。

### 验收门槛（全部可机械判定）

- **放行与拒绝各有用例**：U-04（放行）/ U-05、U-06（拒绝）；只有一类等于没测（与 `scripts/mutation-manifest.txt` 的收录标准一致）
- **真跑脚本的用例 ≥ 4 条**（U-04…U-08）：仅断言常量文本无法证明脚本真的会执行到那个分支
- **不回归**：U-06（不在 PATH 恒拦）与 U-07（正则兜底不被吃掉）是本需求**唯一可能削弱既有保护**的两处，缺任一条即视为不合格
- **不新增依赖**：`core/Cargo.toml` 保持零依赖
- **零新增 `unwrap()`**：脚本是字符串常量，不涉及；单测沿用 `testutil` 既有风格

### 残留风险（必须随 PR 一起披露）

1. **变量与 `PATH` 权限等价**：能设 `REQ_GUARD_BIN` 就能改 `PATH`，故本需求不缩小攻击面，只提升可审计性。真正兜底仍需 CODEOWNERS 覆盖 `.gates/hooks/**`（REQ-020 已提出同一条）。
2. **Windows 侧只判「是文件」、不判可执行**：指到非可执行文件会在调用时才失败，报错不如 POSIX 侧精确（仍 fail-closed，不放行）。
3. **同名变量两处语义不同**：`packaging/scripts/selfcheck.{sh,ps1}` 的 `REQ_GUARD_BIN` 指「被自检的二进制」，本需求的指「执行裁决的二进制」。二者作用域不重叠，但同名易混淆 —— 文档须写明。
4. **存量仓库需重跑 `install`** 才能拿到新脚本；旧脚本不认变量，行为维持今天的 fail-closed（不放行，也不报错提示变量被忽略）。

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
- Given: 某临时 git 仓库已执行 `req-guard init` 与 `req-guard install`，且子进程环境中未设置 `REQ_GUARD_BIN`、`PATH` 被设为 `/usr/bin:/bin`（其中没有 `req-guard`）
- When: 执行 `sh .gates/hooks/req-guard-check.sh` 且 stdin 为空
- Then: 退出码为 1，且 stderr 含 1 处字面量 `req-guard 不在 PATH`

### AC-002
- Given: 某临时 git 仓库已执行 `req-guard init` 与 `req-guard install`，且 `/tmp/rg-fake-ok` 是一个内容仅为两行（`#!/bin/sh` 与 `exit 0`）并已设执行位的文件，`REQ_GUARD_BIN` 设为 `/tmp/rg-fake-ok`
- When: 执行 `sh .gates/hooks/req-guard-check.sh` 且 stdin 为空
- Then: 退出码为 0

### AC-003
- Given: 某临时 git 仓库已执行 `req-guard init` 与 `req-guard install`，且 `REQ_GUARD_BIN` 设为 `/tmp/definitely-not-exist-req-guard`
- When: 执行 `sh .gates/hooks/req-guard-check.sh` 且 stdin 为空
- Then: 退出码为 1，且 stderr 含 1 处字面量 `/tmp/definitely-not-exist-req-guard`

### AC-004
- Given: 某临时 git 仓库已执行 `req-guard init` 与 `req-guard install`，未设置 `REQ_GUARD_BIN`，`PATH` 中无 `req-guard`，且 stdin 传入的 payload 含 1 处 `"file_path": "<仓库根>/.gates/requirements/REQ-001.comments.md"`
- When: 执行 `sh .gates/hooks/req-guard-check.sh` 后读取 `<仓库根>/.gates/audit/gate-audit.log`
- Then: 脚本退出码为 1，且该日志本次新增行中含 1 处字面量 `BLOCK-AI-WRITE-COMMENTS`

### AC-005
- Given: 某临时 git 仓库已执行 `req-guard init` 与 `req-guard install`，`/tmp/rg-rec.sh` 是一个把每次调用的首个参数追加写入 `/tmp/rg-calls.txt` 后 `exit 0` 的可执行脚本，`REQ_GUARD_BIN` 设为 `/tmp/rg-rec.sh`，且 stdin 传入的 payload 含 1 处 `"file_path": "<仓库根>/core/src/a.rs"`
- When: 执行 `sh .gates/hooks/req-guard-check.sh` 后读取 `/tmp/rg-calls.txt`
- Then: 脚本退出码为 0，且该文件含 1 行字面量 `hook-check`

### AC-006
- Given: 某临时 git 仓库已执行 `req-guard init` 与 `req-guard install`，`/tmp/rg-fake-ok` 是内容为两行（`#!/bin/sh` 与 `exit 0`）并已设执行位的文件；情形 A 的 `PATH` 含 `/tmp` 且不设 `REQ_GUARD_BIN`，情形 B 设 `REQ_GUARD_BIN=/tmp/rg-fake-ok` 且 `PATH` 不含 `/tmp`
- When: 两种情形各执行一次 `sh .gates/hooks/req-guard-check.sh` 且 stdin 均为空
- Then: 两次退出码均为 0

### AC-007
- Given: 读取 `core/src/gate.rs` 中 `HOOK_SH` 与 `HOOK_PS1` 两个常量的内容
- When: 分别统计二者含 `REQ_GUARD_BIN` 的行数
- Then: 两个常量各至少含 2 行含该字面量，且 `HOOK_PS1` 中 `Get-Command req-guard` 出现在 `elseif` 分支而非 `if` 分支

### AC-008
- Given: 读取 `packaging/docs/常见问题.md` 的常见问题表格
- When: 检索该文件中字面量 `REQ_GUARD_BIN` 的出现次数
- Then: 出现次数至少为 1
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-10_19:19:14 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-10_19:19:33 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-10_19:19:40 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->

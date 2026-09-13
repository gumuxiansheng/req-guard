# 保证 AI 工具合规实施规范（req-guard）

> 配套：《技术方案.md》《需求分析报告.md》《UI架构细化方案.md》。
> 本文定义 req-guard 在"多 AI 工具共存"环境下的**保证模型**与**部署/验收清单**，
> 明确：门禁保证什么、不保证什么、每一层必须如何配置才能算"真正生效"。
> 适用范围：任何接入 req-guard 的项目、任何会写代码的 AI 工具（Claude Code / CodeBuddy / Codex / Cursor 及未来工具）。

---

## 1. 目的与范围

req-guard 是 gates-toolkit 的流程门禁，管"AI **该不该写**"。本规范解决一个具体问题：

> **如何保证"每一个"接入项目的 AI 工具都遵守三段审核流程，而不是只管住部分工具？**

答案不是"把 L1 hook 注入得更多更全"，而是**把保证建立在工具无关、AI 绕不过的边界上**，并把 L1 明确定位为"体验层"而非"保证层"。

---

## 2. 信任模型（必须先读）

### 2.1 req-guard 保证什么

| 保证项 | 含义 | 依赖 |
|---|---|---|
| 状态不可被"正文编辑"骗过 | 正文随便改，脚本只读 `GATE:*` 标记行 | 三段不全 `approved` 即拦 |
| 评论证据不可被 AI 直接篡改 | 独立 `*.comments.md` 文件 + L1 第 0 步拦截 | L1 生效时 |
| 绕过有痕、有时效 | `.gates/.bypass` 带 `expires_epoch`，命中即审计 | — |
| 缺失即最严 | `strict_order`、`StepState` 未知值一律按最严解释（fail-closed） | — |

### 2.2 req-guard **不**保证什么（关键边界）

> **req-guard 是"纪律/流程强制工具"，不是"密码学/OS 级安全边界"。**
> 其硬边界建立在以下信任假设上；任一假设不成立，对应保证即失效：

| 假设 | 不成立时的后果 | 失效层级 |
|---|---|---|
| AI 只通过被 hook 管控的 `Write\|Edit\|MultiEdit\|NotebookEdit` 写文件 | AI 改用 Shell（`cat >`/`tee`）或 `req-guard approve` 自批 → 绕过 | L1 失效 |
| 审核人是**真实人类**，在 AI 执行上下文之外操作 | AI 自批/自结（`--reviewer` 仅字符串，`resolve` 仅拒 `author=ai` 字面量） | 命令层失效 |
| L2 pre-commit / L3 CI 真正执行 | 脚本缺失被跳过（`if [ -f ]` 静默 fail-open）、CI 默认 `enforce.ci:false` | L2/L3 失效 |
| 各 AI 工具确实加载了注入的 hook | 工具 schema 不识别、已存在配置未合并 → 静默无 hook | L1 静默缺失 |

**设计推论**：要让"每个工具"守规矩，必须做到——

1. **状态核验放在工具无关的边界**（git pre-commit + 服务端 CI），不依赖具体工具；
2. **审批动作必须鉴权**（人类持有、AI 环境拿不到的凭据），否则状态可被欺诈性置为 `approved`，L2/L3 照单全收；
3. **L1 仅作 UX 快失败**，其缺位不得削弱 L2/L3。

> **落地进展（2026-09）**：§4.2 L2 fail-closed、§4.3 `enforce.ci` 默认 true、
> §4.4 **方案 A + 方案 B**（`REQ_GUARD_AI_CTX` 软标记 + `token issue/status/revoke` 审批令牌）、
> §4.5 `install --verify`、§4.6 入库台账 + `audit-digest` 已实现；Claude/CodeBuddy/Codex/Cursor
> 已各自注入**原生 hook schema**（Codex/Cursor 走 deny 包装适配其 exit 2 拦截语义）——见第 8 章附录。
> 方案 C（带外审批）为并发多机的最终保证，待做。

---

## 3. 保证架构：三层定位

```
AI 工具 A / B / C / ...（未知工具也算）
        │
   [L1 PreToolUse]  ── 体验层（快失败，非保证）
   matcher: Write|Edit|MultiEdit|NotebookEdit
        │  越靠前越硬，但只覆盖"用这些工具写文件"的路径
        ▼
   AI 改写源码 / 清单
        │
   [L2 git pre-commit]  ── 墙①（工具无关，fail-closed）
   重跑同一脚本，未过审 exit 1
        │
   [L3 CI / 合并门]  ── 墙②（工具无关，服务端独立，必需检查）
   req-guard check 作为 required status
        │
   合并入主干
        ▲
        │
   [锁] 审批动作鉴权：approve/resolve/bypass 须人类凭据
        │             ← 保证"状态是被人类合法置为 approved"，而非 AI 自批
```

**一句话**：L1 是提醒，L2/L3 是墙，审批鉴权是锁。墙只认状态、锁才认人——两者缺一不可。

---

## 4. 各层部署规范与校验清单

### 4.1 L1 —— 体验层（不做"保证"要求，但要做"厚"）

**定位**：AI 一写就被拦，反馈及时；**不计入保证**，其缺失由 L2/L3 兜底。

**部署要求**：

| 项 | 规范 | 代码/配置锚点 |
|---|---|---|
| 支持工具 | 至少覆盖 `claude` / `codebuddy` / `codex` / `cursor` | `core/src/gate.rs` `TOOL_PROFILES` |
| matcher | 各工具原生工具名：Claude/CodeBuddy/Cursor `Write\|Edit\|MultiEdit\|NotebookEdit`；Codex `apply_patch` | `gate.rs::hook_json()` 按工具渲染 |
| 拦截编码 | Claude/CodeBuddy 直连 `check`（exit 0/1）；Codex/Cursor 走 `deny` 包装（exit 2，因这二者把非 exit 0 视为 fail-open） | `gate.rs::DENY_SH/DENY_PS1` |
| 幂等 | 已含该工具 marker（`req-guard-check` 或 `req-guard-deny`）则跳过 | `inject_tool()` + `marker_of()` |
| 冲突 | 已存在配置**不含** hook 时，**不得静默跳过** | 提示手工合并 + `install --verify` 在 CI 检出（见 4.5） |

**校验清单（L1）**：
- [ ] 每个在用 AI 工具的配置文件含对应 hook marker（`check` 或 `deny`），且路径为该工具原生位置
- [ ] 配置 schema 与该工具原生格式一致（非仅 Claude 风格）——见第 8 章附录
- [ ] 未知/新工具出现时，CI 能检出"缺 hook"并红（见 4.5）
- [ ] 明确记录：L1 **不覆盖** AI 经 Shell 写文件 / 自调 `req-guard approve` 的路径

### 4.2 L2 —— pre-commit 真 fail-closed（墙①，工具无关）

**现状（已实现）**：`core/src/gate.rs` 的 `PRE_COMMIT_BLOCK` 为
```sh
if [ ! -f .gates/hooks/req-guard-check.sh ]; then
  echo "✗ req-guard 门禁脚本缺失……提交已被阻止。" >&2
  exit 1
fi
sh .gates/hooks/req-guard-check.sh || exit 1
```
脚本缺失即**拦截提交并提示初始化**（fail-closed），与 gates-toolkit `030-reqguard`
片段语义一致；真机回归见 `scripts/verify_gate.py` 场景 11。

**规范**：
- 脚本/清单缺失 → `exit 1` 并提示初始化；
- 追加时幂等、不覆盖既有 hook（保留）；
- 与 gates-toolkit `030-reqguard` 片段语义对齐（片段侧应为 fail-closed）。

**校验清单（L2）**：
- [ ] 在干净仓库删除 `.gates/hooks/req-guard-check.sh` 后 `git commit` → **必须失败**
- [ ] 三段未批时 `git commit` → `exit 1`，无提交产生
- [ ] 本机 `git commit --no-verify` 仍被**服务端 push rule** 抵消（见 4.3）

### 4.3 L3 —— CI 强制门禁（墙②，服务端独立）

**现状（已实现）**：`REQ_GUARD_YAML` 模板默认 `enforce.ci: true`，`req-guard install`
输出 CI 接入提示，`.gates/README.md`（随项目生成）含部署指引；流水线侧的
必需状态检查与分支保护仍需各项目按指引自行配置。

**规范**：
- 默认 `enforce.ci: true`；流水线显式调用 `req-guard check`；
- 该检查为**必需（required）**状态检查，不通过禁止合并；
- 分支保护：禁止直推 `main`、禁止绕过本机 hook 的提交（服务端再次跑 check）；
- 本机环境无关——开发者用任何工具、任何 OS，服务端都独立重跑。

**校验清单（L3）**：
- [ ] 三段未批的 PR → CI 红、无法合并
- [ ] 直接 push 到受保护分支 → 被服务端拒绝
- [ ] CI 镜像内含 `req-guard` 二进制（见构建产物矩阵）且版本与 tag 一致

### 4.4 锁 —— 审批动作鉴权（最关键，堵自批）

**现状（方案 A + 方案 B 已实现）**：`core/src/auth.rs` 提供 `ensure_human()`，在
`requirement::review`（approve/reject）、`comment::resolve`、`gate::bypass` 入口
统一判定（core 层生效，CLI/TUI/GUI 同约束）：
- **方案 A**：`install` 给支持会话 env 的工具（claude 与 codebuddy）注入
  `"env": {"REQ_GUARD_AI_CTX": "1"}`；检测到该标记即拒；
- **方案 B**：`core/src/token.rs` 签发短期令牌，原文仅打印一次、由人类带外持有
  （存密码管理器/自身会话）；库文件只存 **SHA-256 哈希**于 `~/.config/req-guard/guard.cfg`（0600）。
  **一旦启用令牌，`approve/reject/resolve/bypass` 必须携带有效令牌**（`--token <值>` 或
  环境 `REQ_GUARD_TOKEN`），否则 fail-closed 拒绝——AI 不知道令牌值，无法自批。
- 未启用令牌时自动回退方案 A；`token status/revoke` 查询与撤销。

方案 A 可被 `env -u` 剥离；方案 B 强度来自"令牌值只由人类知道"。残留边界：
codex/cursor 原生配置**无 env 段**，方案 A 无法覆盖其 Shell 路径；且无 OS keychain
（受"零外部依赖"约束），令牌文件与 AI 同用户可读——并发多机 / 强隔离仍建议方案 C。

**规范（任选其一，强度递增）**：

| 方案 | 机制 | 强度 | 实现要点 |
|---|---|---|---|
| A. `REQ_GUARD_AI_CTX` 软标记 | AI 的 shell 会话自动带此变量，`approve/resolve/bypass` 检测到即拒 | 中（提高门槛，AI 可 `env -u` 剥离） | 包装器/启动脚本注入；命令层读取并拒绝 |
| B. reviewer token | 签发短期令牌，存于 AI 不可读位置（OS keychain / 受保护文件）；命令需带令牌 | 高 | 令牌生成/校验；服务端可复核 |
| C. 带外审批 | 人类在独立终端/Web 台签批，门禁只信任带人类签名的审批记录 | 最高 | 审批与 AI 执行上下文完全隔离 |

> 纯字符串身份（现状）**不构成保证**。推荐 B 或 C 作为生产保证；A 可作为开发期快速加固。

**校验清单（锁）**：
- [ ] AI 上下文内执行 `req-guard approve` → 被拒（路径 A 失效）
- [ ] AI 上下文内执行 `req-guard resolve` → 被拒（路径 B 失效）
- [ ] AI 直接写 `.gates/.bypass`（超长 `expires_epoch`）→ 被凭据/审计机制识别（路径 C 失效）
- [ ] 真实人类在带外/带令牌审批 → 正常通过

### 4.5 L1 注入器：未知工具默认拒绝（消除静默缺口）

**现状（已实现）**：提供 `req-guard install --verify`——核心资产缺失、在用工具配置
缺 `req-guard-check`、env 注入型工具缺 `REQ_GUARD_AI_CTX`、`.git` 存在但 pre-commit
缺拦截，任一命中即退出码 1；可直接作为 CI 步骤（见 4.1/4.3 校验清单）。
注入器对"已存在但不含 hook"的配置仍不覆盖（避免破坏既有配置），改为提示手工合并 +
`--verify` 在 CI 检出。

**规范**：
- 注入器在 `req-guard install` 后，提供 `req-guard install --verify` 或 CI 步骤，扫描所有已知工具配置是否含 `req-guard-check`；
- 任一在用工具缺 hook → **CI 报错**（而非本机提示）；
- 未知 AI 工具出现时，默认视为"未受管控"，CI 红并要求显式登记。

**校验清单**：
- [ ] CI 跑 `req-guard install --verify` → 全部在用工具配置含 hook 才绿
- [ ] 新增未知 AI 工具 → CI 红并提示登记

### 4.6 审计可信化

**现状（已实现）**：关键事件（approve / reject / resolve / bypass / 阻塞性评论新增）
同步写入**入库**的 `.gates/audit/ledger.md`（Markdown 台账，PR diff 可复核）；
`req-guard audit-digest` 对本机 `gate-audit.log` 计算 SHA-256，追加到入库的
`.gates/audit/DIGEST`，与本地日志比对即可发现事后篡改。

**规范**：
- 审计摘要或哈希定期提交到可审阅位置（版本控制 `audit/` 或中央日志）；
- PR 中可复核"谁在何时批了什么、是否带阻塞评论"。

**校验清单**：
- [ ] 关键审批/绕过事件在 PR 可见
- [ ] 审计摘要哈希与本地日志可比对

---

## 5. 部署总检查清单（上线前逐项勾选）

工具无关保证（核心，必须全绿）：
- [ ] **L2 真 fail-closed**：脚本缺失时 `git commit` 失败
- [ ] **L3 强制开启**：`enforce.ci:true` + 必需状态检查 + 分支保护
- [ ] **锁已上**：`approve/resolve/bypass` 在 AI 上下文被拒（方案 B/C）
- [ ] **审计入库**：PR 可审阅审批轨迹

L1 体验层（做厚，但不计入保证）：
- [ ] 所有在用 AI 工具配置含 hook（CI `--verify` 全绿）
- [ ] 未知工具触发 CI 红
- [ ] 已向团队声明：L1 不覆盖 Shell 写文件 / 自调命令路径

---

## 6. 验收标准（如何证明"每个工具都守规矩"）

给出**可证伪**的验收方法，而非"看起来能拦"：

| 验收项 | 操作 | 期望 |
|---|---|---|
| 未批即写 | 任一 AI 工具尝试写源码（无论用 Write 还是 Bash `cat >`） | 提交流程被 L2/L3 拦截，无法合并 |
| 自批失效 | AI 上下文调 `req-guard approve` | 被锁拒绝（4.4） |
| 绕过失效 | AI 直写 `.bypass` 超长过期 | 被凭据/审计识别 |
| 跨工具一致 | 用 Claude / CodeBuddy / Codex / Cursor 各跑同一未批需求 | 行为一致，均被 L2/L3 拦 |
| 缺失即严 | 删脚本后 commit | L2 fail-closed 失败 |
| 审计可溯 | 走完一轮审批+绕过 | PR 可见完整轨迹 |

> 若某条验收不通过，说明对应层未达"保证"要求，需回到第 4 章对应小节补齐。

---

## 7. 残留风险与边界（诚实披露）

1. **L1 对 Shell 写文件天然覆盖不到**：这是 hook 机制的固有限制，靠 L2/L3 + 锁兜底，不靠 L1。
2. **锁的强度取决于凭据隔离**：方案 A（软标记）可被 `env -u` 绕过，仅作开发期加固；生产用 B/C。
3. **人类凭据本身泄露**则保证崩塌——属"人类侧"风险，需配合凭据轮换与最小授权。
4. **`done` 状态归档后不再拦截**：属预期行为（已交付需求），非绕过。
5. **hook 执行失败语义依赖 AI 工具**：若工具将 hook 非零视为"仅警告"，L1 可能 fail-open——故 L2/L3 必须独立存在。

---

## 8. 附录：各 AI 工具 hook 配置形态（参考）

注入点（`core/src/gate.rs::TOOL_PROFILES`），每工具注入**原生** schema：

| 工具 | 配置文件 | 事件 | matcher | 拦截命令 | 审批锁 env |
|---|---|---|---|---|---|
| claude | `.claude/settings.json` | `PreToolUse` | `Write\|Edit\|MultiEdit\|NotebookEdit` | `check`（exit 0/1） | ✅（`env` 段） |
| codebuddy | `.codebuddy/settings.json` | `PreToolUse` | `Write\|Edit\|MultiEdit\|NotebookEdit` | `check`（exit 0/1） | ✅（顶层 `env`） |
| codex | `.codex/hooks.json` | `PreToolUse` | `apply_patch` | `deny` 包装（exit 2） | ✗ |
| cursor | `.cursor/hooks.json` | `preToolUse` | `Write\|Edit\|MultiEdit\|NotebookEdit` | `deny` 包装（exit 2） | ✗ |

命令形态（Windows 用 ps1）：
- 直连：`sh .gates/hooks/req-guard-check.sh`（Claude/CodeBuddy）
- deny 包装：`sh .gates/hooks/req-guard-deny.sh`（Codex/Cursor，内部调 check 并把 exit 1 转成工具可识别的 exit 2；因这二者把非 exit 0 视为 fail-open）

matcher 区分大小写、匹配**工具名**；返回非零即阻断对应写路径。

> 新工具接入：在 `TOOL_PROFILES` 增加条目（含其原生事件名/matcher/拦截编码/是否支持 env）+ 提供该工具原生 schema 的注入片段 + 登记到 4.5 的 CI 校验白名单（`marker_of()` 决定该校验的 marker 子串）。

---

*本规范随 req-guard 演进维护；任何"保证"相关改动须同步更新第 2.2、4、6 章并回归验收清单。*

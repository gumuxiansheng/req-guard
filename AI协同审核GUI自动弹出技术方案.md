# AI 协同审核 GUI 自动弹出技术方案（req-guard）

> 配套：《技术方案.md》《UI架构细化方案.md》《AI工具合规保证规范.md》。
> 本文解决一个新增问题：**当多种不同的 AI 系统检测到"审核 GUI 已存在且可用"时，如何自动复用同一 GUI 完成人工审核，且不重复弹窗、不互相冲突，结果可靠回传。**
> 聚焦**架构设计与组件交互**，不提供具体代码实现。所有新增能力均**复用现有 req-guard 基础设施**（CLI 自动探测、core 结构化 API、L1/L2/L3 三层、审批"锁"），不引入并行判定逻辑。

---

## 0. 结论先行

引入三件套，把"多种 AI → 一个 GUI → 一次人工决策 → 全局生效"串成闭环：

| 组件 | 职责 | 复用现有 |
| --- | --- | --- |
| **审核请求桥** `req-guard ui --request` | AI 与 GUI 之间的唯一入口；要么抢锁启动 GUI，要么向运行实例发请求 | `req-guard ui` 自动探测逻辑 |
| **审核 GUI 单实例**（egui） | 持有单实例锁 + IPC 信道；维护"审核请求注册表"；渲染需求并落盘决策 | 现有 GUI 门禁管理台 + `core` API |
| **单实例锁 + IPC 信道** | 原子抢锁保证只有一个 GUI；socket/命名管道做请求与结果通道 | 新增（轻量、文件级） |
| **唯一真相 `.gates/`** | 需求/评论状态全在文件；GUI 与 AI 都经 `core` 读写 | 现有"唯一真相在 core"原则 |

核心设计判断：**不让每个 AI 各自 spawn 一个 GUI**。所有"弹出"统一经由一个协调器（GUI 单实例 + 请求桥），以 `(需求ID, 段落)` 为去重键，一次人工决策对全部 AI 生效。GUI 只做"展示 + 由人类落盘"，任何审批动作仍受"锁"（方案 B 令牌 / 方案 C 带外）约束——AI 无法借 GUI 自助审批。

---

## 1. 总体架构与组件交互

```
                    ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
   多种 AI 工具      │  Claude/Code │ │ CodeBuddy/   │ │ Codex/Cursor │
   (各带 L1 hook)   │  /其他工具 A │ │  工具 B      │ │  工具 C ...  │
                    └──────┬───────┘ └──────┬───────┘ └──────┬───────┘
                           │ 被拦 / pre-flight  │            │
                           │  拿到 block reason │            │
                           ▼                    ▼            ▼
                    ┌────────────────────────────────────────────┐
   审核请求桥        │  req-guard ui --request <req_id> [--step]  │
   (统一入口)        │  ① 抢锁：失败→client  成功→启动 GUI        │
                    └───────────────┬────────────────────────────┘
                                    │ 抢锁成功                        │ 抢锁失败
                                    ▼                                ▼
                    ┌───────────────────────┐          ┌─────────────────────────┐
   单实例锁文件      │ ~/.cache/req-guard/    │          │ IPC 信道(同路径 socket) │
   + IPC 信道        │   gui.lock  (原子抢锁) │◄─────────│  向运行实例推送 raise    │
                    │   gui.sock  (请求/结果)│          └─────────────────────────┘
                    └───────────┬───────────┘
                                ▼
                    ┌───────────────────────┐
   审核 GUI 单实例   │ egui 门禁管理台        │
   (持有 review      │ · 审核请求注册表        │
    session)         │   key=(req,step)       │
                     │ · 多 AI 共指同一 key    │
                     │ · 聚焦/置顶/去重        │
                     └───────────┬───────────┘
                                 │ 人类操作(批准/打回/resolve)
                                 │  → 须过"锁"(token/OOB)
                                 ▼
                    ┌───────────────────────┐        ┌──────────────────────┐
   唯一真相 core     │ core: req_review()     │        │ .gates/ 文件          │
   + 文件            │       comment_resolve()│──────► │  GATE:* 标记 / 评论    │
                     │       gate_check()     │        │  audit/ledger + log   │
                     │       audit_tail()     │        └──────────────────────┘
                     └───────────┬───────────┘
                                 │ 经 IPC 回传结构化结果
                                 ▼
                    ┌───────────────────────┐
   结果回传          │ 各 requester 收到      │
                     │ {verdict, reviewer,    │
                     │  remaining_blockers,   │
                     │  gate_exit_code}       │
                     └───────────────────────┘
```

**交互时序（一个 AI 被拦 → 人类审核 → 解封）：**

1. AI 的 L1 hook 触发 `req-guard check`，返回非零 + 结构化 block reason（含 `req_id`/`step`/阻塞原因）。
2. AI 调用**审核请求桥** `req-guard ui --request <req_id> --step <s> --requester <工具标识>`。
3. 请求桥做**三层探测**（见 §2）：有能力、有图形环境、是否需要人工决策。
4. 若 GUI 未运行 → 原子抢锁启动 GUI 并附带该 review key；若已运行 → 经 IPC 发 raise 请求。
5. GUI 在"审核请求注册表"中以 `(req_id, step)` 去重登记该 requester，聚焦/置顶对应需求。
6. 人类在 GUI 点击批准/打回（须过"锁"），GUI 调 `core::req_review()` 落盘 + 写审计。
7. GUI 经 IPC 向所有注册 requester 推送结构化结果；无活跃连接者以 `.gates/` 状态为最终真值，AI 自跑 `gate_check` 感知。
8. AI 收到结果 → 重跑 `gate_check` 确认 → 解封则继续，驳回则 back-off。

---

## 2. AI 检测审核 GUI 存在性的机制

"存在且可用"需拆成**三层探测**，任一层不成立即不可用，禁止弹出：

### 2.1 能力探测（GUI 二进制/特性是否存在）
- 请求桥执行能力探针（概念命令 `req-guard ui --probe`），返回结构化 JSON：
  - `has_gui_binary`：PATH 中是否存在带 `gui`/`full` feature 的 `req-guard` 二进制；
  - `has_display`：图形环境是否就绪（Windows/macOS 恒真；Linux 看 `DISPLAY`/`WAYLAND_DISPLAY` 且非 SSH）；
  - `instance_running`：是否已有一个 GUI 实例在跑（见 2.3）；
  - `endpoint`：运行实例的 IPC 地址（socket 路径或 localhost 端口）。
- 与现有 `ui_mode::detect()` 探测决策表（桌面→GUI、SSH→TUI、GUI 失败→回退 TUI）共用同一判定，避免行为漂移。

### 2.2 环境探测（图形栈是否可起 GUI）
- 复用 `UI架构细化方案.md` §3.1 决策表：无 `DISPLAY` 的 Linux tty、SSH 会话 → GUI 不可起，应走 TUI 或 CLI 指引，**不弹 GUI**。
- 这一步是"可用性"的关键闸门：CI/服务器环境天然无图形栈，探针直接判否。

### 2.3 活性探测（GUI 实例当前是否在跑）
- 运行中的 GUI 在固定路径持有**单实例锁文件**并监听 **IPC 信道**。
  > **修订（2026-09-17 代码核查）**：IPC 传输必须是 **回环 TCP（`127.0.0.1`）**，不能是 Unix socket / Windows 命名管道——
  > 前者不是 Windows 原生能力，后者不在 Rust 标准库里，而 `core` 必须保持零依赖。
  > 两端都在 `std` 里的跨平台 IPC 只有 `std::net::TcpListener`。端口**不固定**，由 GUI 绑定后写入锁文件，客户端读锁文件拿端点。
- 活性探测 = 尝试连该信道：连上即"已在跑"，且可顺带拿到它**当前已打开的 review key 集合**（用于去重与聚焦）。
- 连不上（锁文件存在但进程已死）→ 视为"未在跑"，请求桥可回收锁并启动新实例（防僵尸锁：用 PID 存活校验 + 心跳）。

**探测时机**：仅在"AI 确实需要人工决策"的事件点调用（L1 被拦拿到 block reason、或主动 pre-flight `gate_check` 发现可人工消解项），**不做轮询**，避免无谓开销与误触发。

---

## 3. 触发 GUI 自动弹出的条件与时机

### 3.1 触发事件源
| 事件 | 说明 | 是否应弹 GUI |
| --- | --- | --- |
| L1 `check` 拦截（写被拦） | 拿到 block reason，含 pending 段 / blocking 评论 | ✅（需人工决策类） |
| AI 主动 pre-flight `gate_check` | 提交前自检，发现可人工消解项 | ✅ |
| 新增阻塞评论 / `changes_requested` | 状态机进入"需人工处理" | ✅（聚焦/通知，非必新窗） |
| 拦截原因是"无活跃需求，请先 create" | 这是 AI 的活，非人工审核 | ❌（给 CLI 指引即可） |

### 3.2 准入条件（必须同时满足，否则降级）
1. **存在需人工决策的拦截**：`gate_check` 返回 blocked 且原因属于"某段 pending 待批准"或"存在 open+blocking 评论待 resolve"——而非"请先建需求"等 AI 自处理项；
2. **图形环境就绪**：2.2 探测通过；
3. **非 CI / 非纯非交互上下文**：检测到 `CI`、`REQ_GUARD_AI_CTX` 或 `enforce.ci` 强制且无人工 → **不弹窗**，只回传结构化 block reason 让 AI 停止并等待人工；
4. **无人工正在处理同一 key**：实例已在跑且该 `(req,step)` 已聚焦 → 仅 raise/置顶，不重复开。

### 3.3 触发路径（两种）
- **实例未运行**：请求桥原子抢锁成功 → 启动 GUI 进程，启动参数携带首个 review key；
- **实例已运行**：请求桥作为 client 经 IPC 发送 `raise {req_id, step, requester}` → GUI 聚焦/置顶该需求并高亮，写入注册表。

### 3.4 降级（不弹 GUI 时）
- 无 `DISPLAY` → 尝试 TUI 单实例（同一去重/注册逻辑），或回退 CLI 指引（打印"请人工运行 `req-guard ui`"）；
- CI / 无人工 → 仅输出结构化 block reason，命令以非零退出，让 AI 知道"必须停下等人"，由 L2/L3 兜底。

---

## 4. 多 AI 协调机制（防重复弹窗 / 冲突）

核心：**单实例 + 注册表 + 文件为唯一真相**，把"多个 AI 的请求"收敛为"一次人工决策"。

### 4.1 单实例 + 原子锁
- GUI 启动前先在固定路径**原子抢锁**（目录锁 `O_EXCL`/mkdir，或锁文件写 PID）。
- 唯一胜出者成为 GUI 进程并持有 review session；其余请求桥一律转 client，不 spawn 第二窗口。
- 锁竞争失败即 client，无"抢不到就自建"的旁路 → 杜绝双开。

### 4.2 审核请求注册表（去重核心）
- GUI 进程内维护 **review request registry**，键为 `(req_id, step)`：
  - 多 AI 对同一 `(req,step)` 只产生**一个窗口/一个决策**；各 requester 以 `requester_id`（工具名+会话）登记，用于结果路由；
  - 不同 `(req,step)` → GUI 以列表/分页/tab 呈现，人类逐条处理，每条独立回传。

### 4.3 去重与防抖
- 同一 key 在去重窗口（如 30s）内不重复 raise；跨 AI 的同 key 请求直接合并到现有条目。
- 人类正在交互时（窗口聚焦、正在填审核人/原因），抑制新 raise 的弹窗抖动，仅更新"还有哪些 AI 在等"。

### 4.4 冲突消解（请求 ≠ 决策）
- **真相在文件**：GUI 每次渲染与每次动作前都 re-read `.gates/`，以最新状态为准；AI B 在 AI A 等待期间新增 blocking 评论，则人类下一次动作看到的是最新 truth，仍可能维持 blocked。
- **决策全局生效**：人类批准 `(req,step)` 即对所有 requester 解封（状态写文件，谁跑 `gate_check` 谁解封），requester 仅用于把"结果消息"发给对应 AI，不持有独立状态 → 无跨 AI 状态不一致。
- **并发安全**：对单 requirement 的写经 `core` 串行化；必要时加文件级乐观锁/易变标记，避免两 AI 的"请求"与人类的"决策"产生竞态。

### 4.5 雪崩与边界
- 多 AI 同时被同一需求拦 → 单实例序列化 + 去重窗口吸收，最终只一个窗口；
- 人类离线/长时间不处理 → 请求在注册表堆积，AI 侧以 `gate_check` 持续非零表示"仍被拦"，不超时自批（fail-closed）。

---

## 5. 用户审核完成后的结果回传与处理

### 5.1 决策落地（仍受"锁"约束）
- 人类点击批准/打回/resolve → GUI 调 `core::req_review()` / `core::comment_resolve()` 写 `GATE:*` 标记 + 评论状态；
- **必须经审批锁**（`AI工具合规保证规范.md` §4.4）：若启用方案 B 令牌 / 方案 C 带外，GUI 必须索取人类凭据（token 输入 / OOB 声明）后才落盘——**AI 无法借 GUI 可编程自助审批**；
- 关键事件写入入库 `audit/ledger.md` + `gate-audit.log`（与 CLI 完全一致）。

### 5.2 结果回传（两条通道）
- **主通道（IPC）**：GUI 向每个注册 requester 推送结构化结果：
  `{ event, req_id, step, verdict: approved|rejected, reviewer, ts, remaining_blockers, gate_exit_code }`；
- **兜底通道（文件）**：若某 requester 已退出/断连，结果以 `.gates/` 状态为最终真值——AI 在下次动作前自跑 `gate_check` 自主感知，不依赖长连接存活。

### 5.3 AI 侧处理
- 收到结果 / 感知状态变化 → 重跑 `req-guard check` 确认；
- `approved` 且无其余 blocker → AI 继续写入/提交；
- `rejected` → AI back-off / 重新规划，不得强行绕过；
- 处理完从注册表移除自身 pending request（或标记 satisfied）。

### 5.4 闭环与再通知
- 决策即全局；若人类处理后**又出现新阻塞**（如新增 blocking 评论），GUI 仅**聚焦/通知已有窗口**，不重复弹窗；
- 同一 `(req,step)` 在多 AI 间始终收敛到同一窗口、同一决策。

---

## 6. 与现有 req-guard 机制的衔接

| 现有能力 | 本方案如何复用 |
| --- | --- |
| `req-guard ui` 自动探测 / GUI→TUI 回退 | 直接作为"触发路径"与"无 display 降级"的基础 |
| `core` 结构化 API（`gate_check`/`req_review`/`comment_resolve`/`audit_tail`） | GUI 与 AI 共用，杜绝"GUI 与 CLI 行为不一致" |
| L1 PreToolUse 拦截点 | 作为"AI 被拦 → 触发请求桥"的信号源，不改拦截语义 |
| L2 pre-commit / L3 CI（墙①/墙②） | 仍独立兜底；GUI 弹不出 / 无图形环境时，提交依旧被墙拦下 |
| 审批"锁"（方案 B 令牌 / 方案 C 带外） | GUI 落盘审批动作同样强制 human 凭据，封住"AI 借 GUI 自批" |
| `install --verify` / 入库审计 | 新增的"单实例锁路径、IPC 信道"可纳入 verify 与审计留痕 |

---

## 7. 关键风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| AI 借 GUI 自助审批 | 自批绕过门禁 | 审批锁（token/OOB）在 GUI 落盘路径同样强制；GUI 不向 AI 暴露可编程驱动接口 |
| 锁竞争 / 脑裂双 GUI | 重复弹窗、状态分歧 | 原子抢锁 + PID 心跳；抢锁失败一律转 client |
| `DISPLAY` 误判 | 无图形环境硬起 GUI 崩溃 | 2.2 显式探测；失败回退 TUI / CLI 指引 |
| 结果丢失（requester 已退出） | AI 不知已解封 | 以 `.gates/` 文件状态为最终真值，AI 自跑 `gate_check` 感知 |
| 多 AI 雪崩同请求 | 抖动 / 多窗 | 单实例 + 去重窗口 + 同 key 合并 |
| 人类长期不处理 | AI 卡死等待 | 保持 `gate_check` 非零即可，fail-closed，不自动放行 |
| 实例僵尸锁（崩溃未释放） | 再也起不了 GUI | 启动前校验 PID 存活 + 心跳超时回收 |

---

## 8. 验收要点（可证伪）

| 验收项 | 操作 | 期望 |
| --- | --- | --- |
| 多 AI 同需求被拦 | 两个 AI 工具同时被同一 REQ 拦截并请求 GUI | 仅**一个** GUI 窗口；一次人类批准**解封两者** |
| 无图形环境 | 在纯 tty / SSH / CI 触发 | **不弹 GUI**；CLI 指引且 `check` 仍非零 |
| CI 上下文 | 带 `CI`/`REQ_GUARD_AI_CTX` 触发 | 不弹窗，AI 收到结构化 block 并停止等待 |
| GUI 批准生效 | 人类经 GUI 批准某段 | 两个 AI 的后续 `gate_check` **均通过** |
| AI 借 GUI 自批 | AI 试图经 GUI 路径审批 | 被审批"锁"拒绝 |
| 重复触发 | 同一 AI 30s 内重复请求同 key | 仅聚焦/置顶，不重复开窗口 |

> 本方案所有新增组件均围绕"**一个协调器、一份真相、一把锁**"展开，不新增门禁判定逻辑，不削弱 L2/L3 兜底，保持 req-guard"零依赖核心 + 文件为唯一真相 + fail-closed"的立身之本。

---

## 9. 审查修订记录（2026-09-17，逐条对照源码核查）

> 结论：**方向成立、可以落地，但有 3 个硬阻塞必须先补**。以下均给出代码证据。

| # | 问题 | 证据 | 处置 |
| --- | --- | --- | --- |
| **B1** | **拦截原因无结构化字段**，方案 §3.1 的分类与"去重键含 req_id/step"今天拿不到 | `GateVerdict::Block{detail}` 是脚本 stderr 原文（`gate.rs:495,606`）；脚本只输出人类可读中文（`gate.rs:1327-1363`），唯一机器可读标记是 `REQ_GUARD_BYPASS=1`（`gate.rs:1310`） | 前置 P0：给脚本加 `REQ_GUARD_BLOCK_KIND/REQ/STEP` 标记，`gate_check` 解析为结构化字段。**禁止解析中文文案**（项目既有铁律） |
| **B1'** | 去重键口径：脚本的"活跃需求"是**文件名** `$ACTIVE`（`gate.rs:1318`），与 `GATE:HEAD id=` 的需求 ID 不是一回事 | `gate.rs:1316-1321` | 去重键必须用**需求 ID**，不能用文件名，否则 `REQ-001-xxx.md` 与 `REQ-001` 对不上 |
| **B2** | **零依赖下没有跨平台 IPC**（原方案写成 socket/命名管道） | `core` 零依赖是立身之本；Windows 命名管道不在 `std`；Unix socket 非 Windows 原生 | 改为**回环 TCP + 端口写锁文件**（见 §2.3 修订） |
| **B3** | **★ AI 启动的 GUI 会继承 `REQ_GUARD_AI_CTX`，人类点批准会被"锁"拒绝 → 功能自废** | `requirement.rs:197` → `auth::ensure_human` → 读进程 env `REQ_GUARD_AI_CTX`（`auth.rs:59`）；该变量被注入 claude/codebuddy 会话（`gate.rs:831`） | 请求桥/GUI 启动时**显式清除**该变量；代价是方案 A 对 GUI 路径失去区分力 → **生产必须启用方案 B（令牌）或方案 C** |
| D1 | `ui --request` / `--probe` 不存在 | `Action::Ui` 只有 `--gui/--tui`（`cli.rs:78-81`）；`run_ui` 直接起界面（`main.rs:348`） | 清单含 CLI 扩展 |
| D2 | `ui_mode::detect` 无 CI 判定 | `EnvFacts` 只有 override/ssh/display/windows/macos（`ui_mode.rs:61-72`） | 加 `ci` 事实 + 更新决策表（可单测） |
| D3 | 单实例锁 / 注册表 / IPC **全仓不存在** | 全仓无 `TcpListener` / 锁 / instance | 纯新增 `core::instance` 模块，复用 `token::guard_dir()` 目录约定（`token.rs:25,40`） |
| D4 | GUI `run()` 同步阻塞 | `gui/lib.rs:22-39` | 监听放后台线程 + `mpsc`，App 每帧消费；注册表作为 App 状态 |

### 方案里**不需要改**、且最稳的一环
结果回传的"文件兜底通道"与现有 `gate_check` 完全兼容（AI 自跑 `check` 感知状态），无需任何改动——这是本方案风险最低的部分。

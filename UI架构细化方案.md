# req-guard UI 架构细化方案（egui + TUI 双模）

> 决策已定：**egui 与 TUI 都要**；图形桌面走 egui，纯命令行走 TUI；
> workspace 拆分；GUI 定位为**纯门禁管理台**；eframe **0.36.1**。
> 本文是《GUI跨平台方案选型.md》S2 的细化落地设计，**本轮不含代码改动**。

---

## 1. 总体架构

### 1.1 workspace 结构

```
req-guard/                        # workspace，resolver = "2"
├── Cargo.toml                       # workspace 根（只列 members + profile）
├── core/   req-guard-core        # ★ 零依赖 lib：全部门禁业务逻辑
│   └── src/  error requirement comment gate ui_mode
├── cli/    req-guard              # ★ 唯一 bin：参数解析 + 文本输出
│   └── Cargo.toml                   # feature 门控引入 tui / gui
├── tui/    req-guard-tui         # lib：暴露 run(root) —— ratatui 0.30
└── gui/    req-guard-gui         # lib：暴露 run(root) —— eframe 0.36.1
```

**关键设计：`tui/` 与 `gui/` 是 lib 而非 bin。**

这样 `cli` 可用 cargo feature 把它们链进**同一个二进制**，实现"单文件分发 + 自动选界面"，
同时又能按需裁剪体积：

```toml
# cli/Cargo.toml
[dependencies]
req-guard-core = { path = "../core" }
req-guard-tui  = { path = "../tui", optional = true }
req-guard-gui  = { path = "../gui", optional = true }

[features]
default = []                              # 纯 CLI：零依赖，秒级构建
tui  = ["req-guard-tui"]               # CLI + TUI
gui  = ["req-guard-gui"]               # CLI + GUI
full = ["tui", "gui"]                     # ★ 三合一，单二进制自动探测
```

| 构建命令 | 产物能力 | 依赖体量 |
| --- | --- | --- |
| `cargo build` | 仅 CLI | **0**（保持现有优势） |
| `cargo build --features tui` | CLI + TUI | 小（ratatui + crossterm） |
| `cargo build --features gui` | CLI + egui | 大（~18–64MB / ~1M SLoC） |
| `cargo build --features full` | **三合一，自动探测** | 大 |

> 推荐分发形态：**Windows / macOS / 桌面 Linux 发 `full` 版**（用户无感知自动切换）；
> CI/服务器场景发 `default` 轻量版。

### 1.2 依赖原则

- `core` **永远零依赖**——这是本项目的立身之本，也是 CLI 能秒级构建的前提；
- `tui` / `gui` 只依赖 `core` + 各自 UI 库，**不得反向依赖**；
- **三个前端共用同一套 core API**，杜绝"GUI 与 CLI 门禁行为不一致"。

---

## 2. core lib API 设计（关键重构）

### 2.1 从"打印"改为"返回结构化数据"

现状 `requirement::print_status()` 直接 `println!`，UI 无法复用。lib 化后改为返回结构体，
由各前端自行渲染——**这是本次重构的核心改动**。

```rust
// ---------- 状态模型（三个前端共用） ----------
pub enum StepState { Pending, Approved, Rejected }

pub struct StepStatus {
    pub key: &'static str,        // decomposition / solution / testplan
    pub label: &'static str,      // 需求分解 / 技术方案 / 测试计划
    pub state: StepState,
    pub reviewer: Option<String>,
    pub updated: Option<String>,
}

pub enum ReqState { Draft, InReview, Approved, ChangesRequested }

pub struct ReqStatus {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub state: ReqState,
    pub unlocked: bool,           // 三段全 approved
    pub steps: Vec<StepStatus>,
}
```

### 2.2 core 公开 API

| 分类 | 函数 | 说明 |
| --- | --- | --- |
| 清单 | `req_create(root, id, title) -> Result<Requirement>` | |
| 清单 | `req_review(root, id, step, reviewer, pass, reason, strict) -> Result<Requirement>` | approve/reject 合一 |
| 评论 | `comment_add(root, req_id, step, author, text, quote, blocking) -> Result<Comment>` | 审核人留意见 |
| 评论 | `comment_list(root, req_id) -> Result<Vec<Comment>>` | 含行号锚点与 state |
| 评论 | `comment_resolve(root, req_id, cid, author) -> Result<()>` | **拒绝 `author=ai`** |
| 评论 | `comment_refresh_anchors(root, req_id) -> Result<usize>` | 行号漂移重算，返回 stale 数 |
| 清单 | `req_list(root) -> Result<Vec<ReqStatus>>` | 供列表渲染 |
| 清单 | `req_get(root, id) -> Result<ReqStatus>` | 供详情渲染 |
| 门禁 | `gate_install(root, tools) -> Result<Vec<PathBuf>>` | |
| 门禁 | `gate_check(root) -> Result<GateVerdict>` | 返回结构化结论（放行/拦截+原因） |
| 门禁 | `gate_bypass(root, reason, actor, ttl) -> Result<PathBuf>` | |
| 审计 | `audit_tail(root, n) -> Result<Vec<String>>` | 审计日志尾部 N 行 |
| 探测 | `detect_ui_mode() -> UiMode` | 见 §3 |

### 2.3 迁移清单（保持 CLI 行为不变）

现有 `src/*.rs` 整体迁入 `core/src/lib.rs`：
`error.rs` `requirement.rs` `comment.rs` `gate.rs`
→ `cli/src/main.rs` 只保留参数解析 + 调用 core + 打印。

**回归标准**：迁移后 `req-guard` 全部 12 个命令输出与迁移前一致，
且拦截脚本行为不变（脚本由 core 生成，与 UI 无关）。

---

## 3. 界面模式探测（`ui` 子命令）

```bash
req-guard ui            # 自动探测：桌面 → egui；纯命令行 → TUI
req-guard ui --gui      # 强制 egui
req-guard ui --tui      # 强制 TUI
req-guard ui -p <项目根>
```

### 3.1 探测决策表（按优先级短路）

| 序 | 条件 | 结论 |
| --- | --- | --- |
| 1 | `DEV_SCAFFOLD_UI=gui\|tui` | 直接采用 |
| 2 | 命令行显式 `--gui` / `--tui` | 直接采用 |
| 3 | 编译期未包含对应 feature | 退到另一个；都无则报"需以 --features 重新构建" |
| 4 | Windows / macOS | **Gui** |
| 5 | Linux 且 `SSH_CONNECTION` 或 `SSH_TTY` 存在 | **Tui** |
| 6 | Linux 且 `WAYLAND_DISPLAY` 或 `DISPLAY` 存在 | **Gui** |
| 7 | 其他（无 DISPLAY 的 tty） | **Tui** |
| 8 | **Gui 启动失败**（缺图形栈/wgpu 初始化失败） | **回退 Tui** 并提示原因 |

> 第 8 条是**必须的兜底**：egui 在无 X11/Wayland 环境会直接失败，不能让整个工具崩掉。
> 实现上捕获启动错误后降级，而非 `unwrap`。

---

## 4. GUI 设计（eframe 0.36.1）

### 4.1 定位

**纯门禁管理台**：管理需求清单、执行审核、查看审计。
**不**重复实现拦截判定——所有状态读写都走 `core`，与 CLI 完全等价。

### 4.2 布局（1000×680）

```
┌──────────────────────────────────────────────────────────────┐
│ req-guard 门禁管理台        项目: C:/Dev/xxx   [切换目录] │
├───────────────┬──────────────────────────────────────────────┤
│ 需求列表       │ REQ-001 用户登录改造      状态: 未解锁 🔴    │
│               │ ┌──────────────────────────────────────────┐ │
│ ▶ REQ-001 🔴  │ │ [✓] 1. 需求分解   approved  寇工 22:10   │ │
│   REQ-002 🟢  │ │     正文（多行编辑）                     │ │
│               │ │     [批准] [打回]                        │ │
│               │ └──────────────────────────────────────────┘ │
│               │ ┌──────────────────────────────────────────┐ │
│               │ │ [ ] 2. 技术方案   pending                │ │
│               │ │     （前置未通过时按钮置灰 + 提示）       │ │
│               │ └──────────────────────────────────────────┘ │
│               │ ┌ 3. 测试计划 … ┐                            │
├───────────────┴──────────────────────────────────────────────┤
│ [创建需求] [刷新] [执行门禁检查] [应急绕过] [审计日志]        │
└──────────────────────────────────────────────────────────────┘
```

### 4.3 交互

| 操作 | 行为 |
| --- | --- |
| 点选需求 | `req_get()` 加载详情 |
| [批准]/[打回] | 弹窗填**审核人**（必填）+ 原因（打回必填）→ `req_review()` |
| 前置未通过 | 按钮置灰，tooltip 说明"需先批准需求分解" |
| [创建需求] | 弹窗填标题 → `req_create()` |
| [执行门禁检查] | `gate_check()`，弹窗显示放行/拦截与原因 |
| [应急绕过] | 弹窗填原因 + TTL → `gate_bypass()`，二次确认 |
| [审计日志] | 打开面板显示 `audit_tail(200)` |
| [切换目录] | `rfd` 选择项目根 |

**刷新策略**：操作后自动刷新；另提供手动 [刷新] 与 3s 轻量轮询（不做文件系统监听，避免过度设计）。

### 4.4 技术要点

| 项 | 决策 |
| --- | --- |
| 渲染后端 | 默认 **wgpu**；若目标环境 OpenGL 更稳则 `Renderer::Glow`（如 musl/老机器，但本项目不走 musl） |
| 中文字体 | **必须内嵌**。egui 默认字体无 CJK → 内嵌 Noto Sans SC 子集（常用 3500 字，约 2–3MB，SIL OFL） |
| 状态持久化 | eframe `persistence` feature：记住窗口大小与最近项目根 |
| 文件对话框 | `rfd`（官方推荐，跨平台） |
| 配色 | 未解锁=红 🔴 / 已解锁=绿 🟢 / 待审=灰；遵循系统深浅色（egui 自动） |

字体接入示例（**以 0.36.1 实际 API 为准，落地时验证**）：

```rust
let mut fonts = egui::FontDefinitions::default();
fonts.font_data.insert(
    "noto_sc".into(),
    egui::FontData::from_static(include_bytes!("../assets/NotoSansSC-Regular.otf")).into(),
);
fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "noto_sc".into());
fonts.families.entry(egui::FontFamily::Monospace).or_default().push("noto_sc".into());
cc.egui_ctx.set_fonts(fonts);
```

---

## 5. TUI 设计（ratatui 0.30 + crossterm 0.29）

### 5.1 版本事实（2026-09 核实）

- **ratatui 0.30.0**（2026-02，官方称"最大版本"）：新增 **no_std 支持**、**模块化架构**
  （拆出 `ratatui-core` / `ratatui-crossterm` / `ratatui-widgets` / `ratatui-macros`）；
- **crossterm 0.29.0**：OSC52 剪贴板、rustix 1.0。

依赖建议：`ratatui = "0.30"` + `crossterm = "0.29"`（全量 crate 起步）；
若要裁剪可改为只依赖 `ratatui-core` + `ratatui-crossterm` + `ratatui-widgets`（feature 名落地时核实）。

### 5.2 布局

```
┌─ req-guard 门禁管理台 ─ 项目: ./ ───────────────── 未解锁 ─┐
│ 需求列表                    │ REQ-001 用户登录改造            │
│ ▶ REQ-001  🔴 未解锁        │                                 │
│   REQ-002  🟢 已解锁        │ 1. 需求分解  [✓ approved] 寇工   │
│                             │ ┌─────────────────────────────┐ │
│                             │ │ 正文（只读，滚动）           │ │
│                             │ └─────────────────────────────┘ │
│                             │ 2. 技术方案  [ pending ]        │
│                             │ 3. 测试计划  [ pending ]        │
├─────────────────────────────┴─────────────────────────────────┤
│ a批准 r打回 n新建 g检查 b绕过 ↑↓选择 ←→切段 q退出 ?帮助       │
└───────────────────────────────────────────────────────────────┘
```

### 5.3 键位

| 键 | 动作 |
| --- | --- |
| `↑/k` `↓/j` | 选择需求 |
| `←/h` `→/l` 或 `Tab` | 切换三段（同时切换右侧正文） |
| `a` | 批准当前段 → 弹输入审核人 |
| `r` | 打回当前段 → 弹输入审核人 + 原因 |
| `n` | 新建需求 → 弹输入标题 |
| `g` | 执行 `gate_check()`，底部显示结论 |
| `b` | 应急绕过 → 弹输入原因 + TTL |
| `L` | 查看审计日志（覆盖层） |
| `R` | 刷新 |
| `?` | 帮助覆盖层 |
| `q` / `Esc` | 退出（弹窗时 Esc 取消） |

### 5.4 技术要点

| 项 | 决策 |
| --- | --- |
| 终端模式 | alternate screen + raw mode；**必须**在退出/panic 时恢复（用 `panic hook` 兜底） |
| 事件循环 | `crossterm::event::poll(200ms)` + 键盘事件；支持 `Event::Resize` 重绘 |
| 中文宽度 | ratatui 基于 unicode-width，中文占 2 列；布局用 `Constraint::Percentage` 避免硬编码列宽 |
| 输入弹窗 | 自制单行输入组件（`ratatui::widgets::Paragraph` + 光标） |
| 编辑正文 | **只读**（TUI 不适合长文本编辑；正文由 AI/用户在编辑器里写） |
| 无鼠标依赖 | 全部键盘可达（SSH 场景可能无鼠标事件） |

> TUI 只读正文是个刻意取舍：清单正文由 AI 编辑，TUI/GUI 只负责**审核决策**，职责清晰。

---

## 6. 构建与分发矩阵

| 平台 | 目标三元组 | `default`(CLI) | `tui` | `full`(含 GUI) |
| --- | --- | --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | zigbuild ✅ | ✅ | ✅ **本机可构建**（MSVC 2022 已装） |
| Windows x64 | `x86_64-pc-windows-gnu` | zigbuild ✅ | ✅ | ✅ |
| Linux x64 | `x86_64-unknown-linux-gnu` | zigbuild ✅ | ✅ | ⚠️ CI runner 原生（需 libxcb/libxkbcommon） |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | zigbuild ✅ | ✅ | ⚠️ 同上（glibc，非 musl） |
| Linux musl | `*-unknown-linux-musl` | ✅ | ✅ | ❌ 不建议承载 GUI |
| macOS x64/ARM64 | `x86_64/aarch64-apple-darwin` | ✅ | ✅ | ⚠️ 需 macOS runner + 签名 |

**CI 建议**（复用你现有的 CNB / GitHub Actions 经验）：
- `cli-only` job：`cargo zigbuild` 交叉出 5 个目标的轻量版；
- `full` job：`windows-latest` / `ubuntu-latest` / `macos-latest` 各**原生**构建
  （GUI 交叉编译需 libxcb 头文件，zigbuild 解决不了，原生构建最省心）。

---

## 7. 落地清单（分阶段）

| 阶段 | 内容 | 产出 | 验证 |
| --- | --- | --- | --- |
| **P0** | workspace 化：建 `core`/`cli`/`tui`/`gui` 骨架；`src/*.rs` 迁入 core；`resolver="2"` | 目录 + Cargo.toml | `cargo build` 后所有 CLI 命令输出与迁移前一致 |
| **P1** | core API 结构化：新增 `ReqStatus`/`StepStatus`/`GateVerdict`；`print_status` 改为返回数据 | core API | 单测：状态机与解析 |
| **P2** | **TUI 先行**：ratatui 0.30 + crossterm 0.29，列表/详情/批准/打回/审计 | `req-guard ui --tui` | 真机键盘全流程 |
| **P3** | GUI：eframe 0.36.1 + Noto Sans SC 子集 + rfd | `req-guard ui --gui` | 真机截图 + 中文无豆腐块 |
| **P4** | 合一：`ui` 子命令自动探测 + GUI 失败回退 TUI + `--features full` | 单二进制 | 桌面走 GUI、SSH 走 TUI、无图形的 Linux 会回退 |
| **P5** | CI 多平台矩阵 + 文档更新 | 产物 + 文档 | 三平台可下载 |

### 为什么 TUI 先行（P2 早于 P3）

1. 依赖小、构建快，能在**无图形环境**（含当前沙箱思路）验证 core API 是否好用；
2. 提前暴露"结构化数据是否够 UI 用"的问题，GUI 阶段可少返工；
3. SSH/服务器场景本来就是刚需，先交付先受益。

---

## 8. 风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| **egui 中文字体缺失** | 满屏豆腐块，不可用 | 内嵌 Noto Sans SC 子集；P3 验收标准明确写"中文清晰无缺字" |
| GUI 在无声环境崩溃 | 用户体验断崖 | §3.1 第 8 条回退 TUI + 明确提示 |
| 依赖体量膨胀 | CLI 失去"零依赖"优势 | feature 门控；`default` 不含任何 UI 依赖 |
| core 迁移引入回归 | 现有功能被破坏 | P0 以"CLI 输出与迁移前完全一致"为验收硬标准 |
| ratatui 0.30 API 变化 | 示例代码不适配 | 落地时以 0.30 官方文档为准；先写最小骨架跑通 |
| 三前端行为漂移 | GUI 与 CLI 判定不一致 | **唯一真相在 core**，前端只渲染不判定 |
| 沙箱环境限制 | 无法构建验证 | 需在有链接器 + 可联网拉 crate 的环境执行；当前仍只能出方案 |

---

## 9. 需要你确认的细节

1. **`ui` 子命令默认行为**：无参数时自动探测（推荐），还是默认 TUI？
2. **TUI 是否允许编辑清单正文**？方案建议**只读**（正文交给 AI/编辑器），你若希望可编辑则工作量增加。
3. **中文字体**：用 Noto Sans SC 子集（2–3MB，推荐），还是跟随系统字体（体积小但 Linux 可能缺失）？
4. **分发形态**：只发 `full` 单二进制（简单），还是 `cli` + `full` 双产物（CI 场景用轻量版）？
5. **是否保留 `req-guard-gui` / `req-guard-tui` 独立二进制**？（方案推荐不做，统一走 `req-guard ui`）

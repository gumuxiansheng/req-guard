# req-guard 跨平台 GUI 方案选型

> 问题：需要跨平台支持，能否用 **egui** 实现？
> 本文只做方案对比与选型，**不含代码改动**。版本事实基于 2026-09 核实。

---

## 0. 结论先行

| 问题 | 结论 |
| --- | --- |
| egui 能实现跨平台 GUI 吗？ | **能**。Windows / Linux / macOS / WASM 官方均支持 |
| 该不该现在上？ | **可以上，但必须拆 crate**：核心逻辑保持零依赖 lib，GUI 作为**独立 bin**，否则会毁掉"开箱即 build"的最大优势 |
| 最大代价 | 依赖树从 **0** 变成 **~100+ crates / ~1M SLoC / 18–64MB**，构建从秒级到分钟级 |
| 最大风险 | Linux 目标需 X11/Wayland 系统库（交叉编译是硬骨头）；**egui 默认字体不含中文** |
| 对门禁的影响 | **无影响**——硬拦截靠 hook 脚本（sh/ps1）跨平台生效，GUI 只是管理台 |

---

## 1. 先厘清"跨平台"的两层含义

| 层次 | 现状 | 是否需要 GUI |
| --- | --- | --- |
| **① 门禁在多平台生效**（生成的 hook 能在 Win/Linux/macOS 拦住 AI） | **已支持**：`ai-gate-check.sh`（POSIX，覆盖 Linux/macOS）+ `.ps1`（Windows） | ❌ 不需要 |
| **② 工具自身跨平台可用**（req-guard 命令在各平台跑得起来） | CLI 已基本支持（纯 Rust，仅平台分支差异） | ❌ 不需要 |
| **③ 提供图形界面**（可视化建清单、点按钮批准、看状态） | 无 | ✅ 需要 |

**关键判断**：如果诉求只是 ① 和 ②，**不需要 egui**，现状已覆盖。
只有当诉求是"给审核人员一个不用敲命令的管理台"时，才值得引入 GUI。

---

## 2. egui 可行性评估（基于 2026-09 核实）

### 2.1 版本与生态事实

| 项 | 事实 |
| --- | --- |
| 最新版本 | **eframe 0.36.1**（2026-08-07）；0.35.0（2026-06-25）；0.33.3（2025-12，你现用） |
| 渲染后端 | 默认 **wgpu**；可选 **glow**（OpenGL，musl/老机器上更稳） |
| 官方支持目标 | Win(msvc/gnu/i686)、macOS(x86_64/aarch64)、Linux(gnu/musl)、WASM、Android/iOS(有限) |
| 依赖体积 | **~18–64MB，~1M SLoC** |
| MSRV | 1.84（你的 stable 满足） |
| 工程要求 | 需 `edition = "2024"` 或 workspace `resolver = "2"` |
| 赞助方 | Rerun（活跃维护，可信） |

### 2.2 与本项目现状的冲突

| 维度 | 现状 | 引入 egui 后 |
| --- | --- | --- |
| 依赖 | **零依赖**（最大卖点） | ~100+ crates |
| 构建 | `cargo check` 2.4s | release 首次构建数分钟 |
| 供应链 | 无 | 需审计（winit/wgpu/glow/…） |
| 可交叉编译 | zigbuild 5 目标轻松 | Linux/macOS GUI 交叉需 Docker 或目标机 |

> **这是选型的核心矛盾**：egui 会带来与"零依赖"完全相反的代价。

### 2.3 egui 特有风险（必踩坑）

| 风险 | 说明 | 对策 |
| --- | --- | --- |
| **默认字体不含中文** | `default_fonts` 只有拉丁字形，中文显示豆腐块 | 必须内嵌 CJK 字体（见 §5.3） |
| **Linux 系统库依赖** | 需 `libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev`；Wayland 另需 wayland 库 | GNU(glibc) 目标 + 目标机构建或 cross/Docker |
| **musl 静态承载 GUI** | 与你的既有经验一致：**不建议**。X11/Wayland 是运行时依赖，静态链接坑多 | ARM Linux GUI 走 **`aarch64-unknown-linux-gnu`（glibc）** |
| **无原生文件对话框/托盘** | egui 不提供 | 加 `rfd`（跨平台文件对话框） |
| 长文本/表格能力一般 | 你在 diff-guard 已体会（scrollbar 对齐等需手写） | 清单编辑用多行 `TextEdit` + `ScrollArea` |
| 无障碍 | AccessKit 支持中，实验性 | 内部工具可接受 |

### 2.4 egui 的一个意外加分项

0.35 起新增 **egui_mcp / Inspection Protocol**：让 AI agent 能"看见并操作"运行中的 egui 应用
（`EGUI_INSPECTION=1` 监听 5719 端口）。对本项目（AI 门禁管理台）意味着：
**未来可让 AI 自助打开 GUI 查看门禁状态、复现拦截**，与"AI 协作"主题天然契合。

---

## 3. 方案对比

| 方案 | 说明 | 优点 | 缺点 | 适用度 |
| --- | --- | --- | --- | --- |
| **S0 不做 GUI** | 保持 CLI，仅补跨平台细节 | 零成本、零风险、保住零依赖 | 审核人员必须敲命令 | ★★★☆☆ |
| **S1 单 crate + feature** | 同 crate 加 `gui` feature | 改动小 | **破坏零依赖**（即使不启用 feature，Cargo.lock 也会膨胀） | ★☆☆☆☆ |
| **S2 workspace 拆分（推荐）** | `core`(lib,零依赖) + `cli`(bin) + `gui`(bin, eframe) | CLI 仍开箱即 build；GUI 可选装 | 需重构：逻辑从 bin 迁到 lib | **★★★★★** |
| **S3 egui + WASM** | 网页版配置器 | 免安装、天然跨平台 | ❌ 浏览器沙箱**不能执行 git/写文件**，只能导出配置/脚本 | ★★☆☆☆ |
| **S4 Tauri** | Web 前端 + Rust 后端 | UI 能力强、生态成熟 | 依赖系统 WebView（WebView2/webkit2gtk）+ Node 构建链 | ★★★☆☆ |
| **S5 Ratatui (TUI)** | 终端图形界面 | 依赖远小于 egui（crossterm/ratatui）、SSH/无头可用 | 交互与观感弱于 GUI；清单编辑体验一般 | ★★★☆☆ |

### 决策矩阵

| 诉求 | S0 CLI | S2 egui | S3 WASM | S4 Tauri | S5 TUI |
| --- | --- | --- | --- | --- | --- |
| 审核人员零命令行 | ❌ | ✅ | ✅ | ✅ | ⚠️ |
| 保住核心零依赖 | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| 本地执行 git/门禁 | ✅ | ✅ | ❌ | ✅ | ✅ |
| 构建/分发成本 | 极低 | 中 | 极低（网页） | 高 | 低 |
| 你的既有经验 | — | **有（diff-guard/sql-guard）** | 无 | 无 | 无 |
| 跨平台难度 | 低 | **中高（Linux/macOS）** | 低 | 中 | 低 |

---

## 4. 推荐架构：S2 workspace 三 crate

```
req-guard/
├── Cargo.toml            # workspace，resolver = "2"
├── core/                 # ★ 零依赖 lib：全部业务逻辑
│   ├── src/lib.rs        # 暴露 project / templates / generator / git / gates / requirement / gate
│   └── Cargo.toml        # 无 [dependencies]
├── cli/                  # 现有 req-guard bin，仅做参数解析与输出
│   └── Cargo.toml        # req-guard-core
└── gui/                  # req-guard-gui bin（eframe 0.36.1）
    ├── Cargo.toml        # eframe + rfd + req-guard-core
    └── assets/NotoSansSC-Regular.otf   # 内嵌中文字体
```

**收益**：
- `cargo build -p req-guard` 仍**零依赖、秒级**——CI/脚本场景不受影响；
- `cargo build -p req-guard-gui` 才拉入 eframe 依赖树；
- GUI 与 CLI 复用同一套逻辑，**不会出现两边行为不一致**（门禁尤其重要）。

**重构代价**：现有 `src/*.rs` 从 bin 模块迁到 `core/src/lib.rs`，`main.rs` 瘦身。约 1 次中等改动。

---

## 5. 跨平台构建矩阵（若选 S2）

| 平台 | 目标三元组 | CLI（零依赖） | GUI（eframe） | 说明 |
| --- | --- | --- | --- | --- |
| **Windows x64** | `x86_64-pc-windows-msvc` | zigbuild ✅ | ✅ | **你的环境已齐备**（MSVC 2022 14.44 + Win Kits 22621） |
| Windows x64 | `x86_64-pc-windows-gnu` | zigbuild ✅ | ✅ | 你的既有 zigbuild 工作流 |
| **Linux x64** | `x86_64-unknown-linux-gnu` | zigbuild ✅ | ⚠️ 需 X11/Wayland 头文件 | 建议 **目标机或 CI runner 原生构建** |
| **Linux ARM64** | `aarch64-unknown-linux-gnu` | zigbuild ✅ | ⚠️ 同上 | 与你在 diff-guard 的做法一致（glibc，非 musl） |
| Linux musl | `*-unknown-linux-musl` | ✅ | ❌ **不建议** | 与你的既有结论一致：musl 静态承载不了 GUI |
| **macOS x64 / ARM64** | `x86_64/aarch64-apple-darwin` | ✅ | ⚠️ 需 Xcode CLI + 代码签名 | 需 macOS 机或 CI（macOS runner） |
| Web | `wasm32-unknown-unknown` | N/A | ✅（功能受限） | 只能做配置器，见 S3 |

### 5.1 交叉编译的现实选择

`cargo-zigbuild` **不能解决** Linux GUI 的 X11 头文件缺失（zig 只提供 C 链接器与 libc，不提供 libxcb 头文件）。可行路径：

1. **CI 多平台原生构建（推荐）**：GitHub Actions / CNB 起 `ubuntu-latest`、`macos-latest`、`windows-latest`
   runner 各自原生 `cargo build --release`，一次产出三平台产物。这是最省心的方案，也契合你现有的
   CNB/GitHub Actions 经验。
2. **cross + Docker**：本地交叉 Linux，需维护 Docker 镜像。
3. **目标机构建**：最原始但最可靠。

### 5.2 构建命令示例

```bash
# CLI：保持零依赖，zigbuild 出多平台
cargo zigbuild --target x86_64-pc-windows-gnu        -p req-guard --release
cargo zigbuild --target aarch64-unknown-linux-gnu    -p req-guard --release

# GUI：建议原生构建（Windows 本机即可）
cargo build -p req-guard-gui --release                       # 宿主平台
# Linux/macOS GUI 交给 CI runner 原生构建
```

### 5.3 中文字体（必做）

egui 默认字体**无 CJK 字形**，中文会全变成豆腐块。做法（以 0.36 API 为准，落地时验证）：

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

- 完整 Noto Sans SC OTF ≈ 10MB；**子集化（常用 3500 字）可降到 2–3MB**；
- 许可：SIL OFL（可商用、可内嵌）。

---

## 6. GUI 该长什么样（若选 S2）

门禁管理台，三个面板：

| 面板 | 内容 |
| --- | --- |
| **需求列表** | `.gates/requirements/`（req-guard）全部需求，状态色标（未解锁=红 / 已解锁=绿） |
| **三段清单** | 选中需求的分解/方案/计划正文（多行编辑）+ 每段状态与审核人 |
| **操作区** | `创建需求` / `批准该段`（弹窗填审核人+原因）/ `打回` / `查看审计日志` / `应急绕过` |

> 原则：GUI **只做管理台**，不重复实现拦截判定——所有状态变更仍调用 `core` 的
> `requirement::review()` / `gate::bypass()`，保证与 CLI 行为完全一致。

---

## 7. 落地清单（选 S2 后的执行顺序）

| # | 步骤 | 产出 |
| --- | --- | --- |
| 1 | workspace 化：`core` / `cli` / `gui` 三 crate，`resolver = "2"` | 目录与 Cargo.toml |
| 2 | 现有 `src/*.rs` 迁入 `core/src/lib.rs`，`main.rs` 瘦身 | CLI 行为不变（回归验证） |
| 3 | `gui` crate 骨架：eframe 0.36.1 + 内嵌 Noto Sans SC | 空窗口 + 中文正常 |
| 4 | 三面板 UI（列表 / 清单 / 操作） | 可创建与批准需求 |
| 5 | `rfd` 接入：选择项目根目录 | 可选任意项目 |
| 6 | 跨平台验证：Windows 本机 + CI 三平台原生构建 | 三平台产物 |
| 7 | 文档更新：技术方案增"GUI 与跨平台"章节 | 文档 |

---

## 8. 需要你拍板的决策点

1. **是否真的需要 GUI**？若只是"门禁跨平台生效"，现状已满足，不必引入 egui。
2. **走 S2（egui）还是 S5（TUI）**？S5 依赖小得多、跨平台容易，但观感弱。
3. **GUI 定位**：纯管理台（推荐）还是同时承担"生成新项目"向导？
4. **构建策略**：Windows 本机构建 + CI 出 Linux/macOS，还是全部走 CI？
5. **版本**：eframe 用最新 **0.36.1** 还是与你现有项目对齐 **0.33**？

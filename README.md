# req-guard —— AI 需求门禁

> **AI 在编写代码前，必须先走完「需求分解 → 技术方案 → 测试计划」三段审核，否则物理上无法写入文件。**
> gates-toolkit 家族成员（流程门禁：管"该不该写"；sql-guard/java-guard 管"写得对不对"）。

## 特性

- **三段清单审核**：强制顺序 `需求分解 → 技术方案 → 测试计划`，逐段批准
- **硬拦截**：AI 工具 `PreToolUse`（写不了文件）+ git `pre-commit`（fail-closed）+ CI `check`
- **审核评论**：审核人只评论不改正文；步骤级 + 行号锚定；`--blocking` 未 resolve 即拦截
- **证据不可篡改**：评论独立文件，AI 禁止直接写、禁止 resolve（只能 reply）
- **审计留痕**：拦截/放行/绕过/评论全部入 `audit/gate-audit.log`
- **应急绕过**：有时效、必填原因；**不覆盖评论证据保护**
- **审批锁**：`approve/reject/resolve/bypass` 检测 AI 会话标记（`REQ_GUARD_AI_CTX`）即拒，堵 AI 自批
- 零外部依赖（纯 Rust 标准库），开箱即 build

## 快速上手

```bash
# 0. 初始化（写入 .gates/ + 注入 AI 工具 hook + 追加 pre-commit）
req-guard init

# 1. 创建需求清单
req-guard create -t "用户登录改造"          # → REQ-001

# 2. AI 填写三段正文（编辑 .gates/requirements/REQ-001-*.md）

# 3. 审核人逐段批准（此时 AI 仍被拦截）
req-guard approve REQ-001 --step decomposition --reviewer 寇工
req-guard approve REQ-001 --step solution      --reviewer 寇工
req-guard approve REQ-001 --step testplan      --reviewer 寇工

# 4. 查看状态 / 评论
req-guard status REQ-001
req-guard comments REQ-001

# 5. 解锁后 AI 方可编写代码；提交时 pre-commit 二次校验
```

## 审核评论（审核人只评论、不修改）

```bash
# 纯评论（不改状态）
req-guard comment REQ-001 --step solution --author 寇工 --text "回滚方案需补充 DB 迁移回退"

# 锚定到原文 + 标记阻塞（未 resolve 时拦截编码）
req-guard comment REQ-001 --step solution --author 寇工 --quote "回滚方案" --blocking --text "..."

# AI 回复（只能 reply，不能 resolve）
# 不带 --reply 会被拒绝：AI 不得新开评论，只能回复审核人
req-guard comment REQ-001 --author ai --reply C001 --text "已补充迁移回退步骤"

# 审核人关闭
req-guard resolve REQ-001 C001 --author 寇工

# 正文修改后重算行号锚点
req-guard comments REQ-001 --refresh-anchors
```

## 应急绕过

```bash
req-guard bypass --reason "线上热修，事后补审" --ttl 60   # 有痕、有时效
req-guard check                                            # 手动判定（CI 用）
```

## 合规部署（三层 + 锁）

```bash
req-guard install --verify   # 校验资产 / 各 AI 工具 hook / pre-commit 就位（CI 步骤，缺口退出码 1）
req-guard audit-digest       # 本机审计日志 SHA-256 摘要 → 入库 .gates/audit/DIGEST
```

- **L2 fail-closed**：`.gates/hooks/req-guard-check.sh` 缺失时 `git commit` 被阻止（非静默放行）
- **L3 默认开启**：`enforce.ci: true`——流水线调 `req-guard check` 并设为必需状态检查 + 分支保护
- **工具原生 schema**：为 claude / codebuddy / codex / cursor 各自注入原生 hook 配置；
  Codex/Cursor 走 deny 包装（exit 2）适配其拦截语义，CodeBuddy 已修正为 `.codebuddy/settings.json`
- **审批锁（方案 A）**：Claude Code / CodeBuddy 会话注入 `REQ_GUARD_AI_CTX=1`（配置 `env` 段），
  `approve/reject/resolve/bypass` 检测到即拒；审核人在自己的终端审批
- **审批令牌（方案 B）**：`req-guard token issue` 签发短期令牌（原文仅打印一次，人类带外持有），
  启用后审批必须携带有效令牌，AI 拿不到 → 无法自批；`token status/revoke` 查询与撤销
- **带外审批（方案 C）**：审批显式声明带外渠道 `--oob` 或 `req-guard oob <命令>`；
  设 `REQ_GUARD_OOB_ONLY=1` 可强制"仅接受带外审批"；台账按 `channel=oob|interactive` 留痕
- **payload 真解析**：拦截脚本第 0 段优先调 `req-guard hook-check`，由 Rust 解析 AI 工具的
  PreToolUse JSON——路径可写成 `.gates\u002f…comments.md` 这类 Unicode 转义，与明文**完全等价**，
  脚本用正则抠字段会漏判并静默放过；`req-guard` 不在 PATH 时退回正则粗判（保底，不完备）
- **清单正文可写、状态行不可改**：AI 可用 Write 整篇填写 `.gates/requirements/*.md`
  （不等三段批准——否则"AI 填三段正文"这步会被门禁自己拦死）；但 GATE 状态行必须与磁盘
  **逐行一致**，改动即拦截（防 AI 自批）；片段编辑（Edit）无法与基线比对，一律禁止
- **审计入库**：approve / reject / resolve / bypass / 阻塞评论写入 `.gates/audit/ledger.md`（PR 可复核）

详见 [`docs/规范/AI工具合规保证规范.md`](docs/规范/AI工具合规保证规范.md)。

## 命令一览

`init` `create` `approve` `reject` `comment` `resolve` `status` `list` `comments` `check` `install` `bypass` `audit-digest` `token` `oob` `ui`
（内部命令 `hook-check`：由拦截脚本调用，读 stdin 做证据保护判定，通常无需手工执行）
（`req-guard -h` 查看完整参数；`-p` 指定项目根；身份回退环境变量 `REQ_GUARD_REVIEWER`；审批令牌 `--token`/`REQ_GUARD_TOKEN`；带外审批 `--oob`/`REQ_GUARD_OOB`）

## 门禁管理台（TUI / GUI）

```bash
cargo build -p req-guard --features tui   # CLI + 终端界面（依赖小）
cargo build -p req-guard --features gui   # CLI + 桌面界面（依赖大，含内嵌中文字体）
cargo build -p req-guard --features full  # 三合一，单二进制自动探测

req-guard ui            # 自动探测：Windows/macOS → GUI，SSH 会话 / 无图形环境 → TUI
req-guard ui --tui      # 强制终端界面
req-guard ui --gui      # 强制桌面界面
```

TUI 键位：`↑↓` 选择需求 · `←→/Tab` 切段 · `a` 批准 · `r` 打回 · `n` 新建 · `g` 门禁检查 ·
`b` 应急绕过 · `L` 审计日志 · `R` 刷新 · `?` 帮助 · `q` 退出。

GUI 为三面板：左需求列表（红=被卡 / 绿=已解锁）、右三段折叠清单（状态 + 审核人 + 只读正文 + 批准/打回）、
底部操作区（创建/刷新/检查/绕过/审计），另可切换项目根目录。

> 界面只做**管理台**：状态读写全部走 core，与 CLI 行为完全等价（不会出现"界面放行、CLI 拦截"）。
> 清单正文在界面中**只读**——正文由 AI/编辑器维护，界面只负责审核决策。
> GUI 启动失败（无图形栈 / wgpu 初始化失败）时会**自动回退到 TUI**（`full` 特性下）。

## 与 gates-toolkit 的关系

- 由 `setup-gates` 装配（片段 `030-reqguard`，`hookctl register ... --priority 30 --mode block`）
- dev-scaffold `new` 默认启用，`--no-req-guard` 关闭
- 拦截脚本随仓库提交（`.gates/hooks/`）——保证 clone 即生效；脚本缺失时 **fail-closed**

## 目录

```
.gates/
├── req-guard.yaml               # 门禁声明
├── requirements/                # REQ-00N-*.md 清单 + *.comments.md 评论
├── hooks/req-guard-check.{sh,ps1}   # 拦截脚本（Claude/CodeBuddy 直连）
├── hooks/req-guard-deny.{sh,ps1}    # deny 包装：Codex/Cursor 拦截编码转 exit 2
├── ci/req-guard-ci.yml          # L3 接入样例（GitHub Actions），复制进 .github/workflows/
└── audit/gate-audit.log         # 审计（不入库）
```

## 构建与验证

工程为 cargo workspace：`core`（零依赖 lib）+ `cli`（唯一 bin）+ `tui` / `gui`（界面 lib）。

```bash
cargo build --release                  # 默认只构建 core + cli：零 UI 依赖、秒级
cargo build -p req-guard --features tui --release   # 含 TUI
cargo build --release --target x86_64-pc-windows-gnu # 交叉编译（需 mingw-w64 工具链，见「多平台 Release」）
bash scripts/build-release.sh          # 一键多平台 Release 构建（5 目标 × 2 变体）

cargo fmt --all                        # 格式
cargo clippy --workspace --all-targets -- -D warnings   # 静态检查（零警告为门槛）
cargo test --workspace                 # 单元测试（全 workspace；用例数随迭代增长，以实跑输出为准）
python scripts/verify_gate.py          # 拦截脚本真机场景（13 固定场景 + 1 个需二进制在 PATH 的条件场景）
```

> **Windows + Git Bash 注意**：`/usr/bin/link`（GNU coreutils）会遮蔽 MSVC 的 `link.exe`，
> 直接 `cargo build` 会失败。需把 MSVC `bin/Hostx64/x64` 前置到 `PATH`，并设置 `LIB`
> 指向 MSVC `lib/x64` 与 Windows Kits 的 `um/x64`、`ucrt/x64`。

## 多平台 Release 构建与发布

流程与 sql-guard 的 CNB 流水线保持一致：**推 tag → 创建 Release → 交叉编译 → 上传附件**。

```bash
# 1) 把 Cargo.toml 的 [workspace.package] version 改到目标版本（如 0.2.0）
# 2) 提交后打 tag（tag 名必须是 v + 该版本号，两者会被流水线交叉校验）
git tag v0.2.0 && git push cnb main --tags
```

CNB 上 `v*` 触发 `req-guard release` 流水线（`.cnb.yml`），三个阶段：

| 阶段 | 动作 |
|---|---|
| 创建 Release | 内置任务 `git:release`，tag/标题 = 触发 tag，`overlying: true`（同 tag 重跑不报错） |
| 构建多平台二进制 | `bash scripts/build-release.sh`，在 Linux x86_64 执行机上交叉编译到 `dist/` |
| 上传 Release 附件 | `cnbcool/attachments` 上传 `./dist/*`（`git:release` 本身不支持附件） |

**产物矩阵**（每目标 2 个变体，命名规则 `req-guard-<target>[.exe]`）：

| target | 平台 | 默认变体（零依赖 CLI） | TUI 变体（CLI + 终端界面） |
|---|---|---|---|
| `x86_64-unknown-linux-musl` | Linux amd64（静态，无 glibc 依赖） | `req-guard-x86_64-unknown-linux-musl` | `req-guard-ui-x86_64-unknown-linux-musl` |
| `aarch64-unknown-linux-musl` | Linux arm64（静态） | `req-guard-aarch64-unknown-linux-musl` | `req-guard-ui-aarch64-unknown-linux-musl` |
| `x86_64-pc-windows-gnu` | Windows amd64 | `…-windows-gnu.exe` | `…-windows-gnu.exe` |
| `x86_64-apple-darwin` | macOS Intel | `…-apple-darwin` | `…-apple-darwin` |
| `aarch64-apple-darwin` | macOS Apple Silicon | `…-apple-darwin` | `…-apple-darwin` |

另附 `dist/SHA256SUMS` 供下载后校验。

- **版本号规则**：Release tag = `v` + `Cargo.toml` 的 `[workspace.package] version`。
  脚本会做一致性检查（不一致只告警不阻断，但产物以 `Cargo.toml` 为准）。
  `req-guard -V` 输出的就是编译期版本，可用于核对下载到的二进制。
- **失败策略**：Linux/Windows 目标是**必需**的（失败即整体失败，避免发布缺件版本）；
  两个 macOS 目标是 **best-effort**（拿不到 Zig/cargo-zigbuild 时跳过，不阻断）。
  TUI 变体默认必需，可设 `REQGUARD_TUI_BEST_EFFORT=1` 降级为跳过。
- **GUI 不参与交叉编译**：`gui`（eframe/wgpu/rfd）在 Linux 侧依赖 X11/Wayland/GTK
  系统库，无法在 Linux 执行机上交叉编译，需由各平台原生执行机构建
  （见 `.github/workflows/ci.yml`，GitHub Actions 已在 Windows/macOS 上构建 gui）。

**本地复现**（任意 Linux x86_64，或 Windows 上单独验证某个 target）：

```bash
# 完整多平台构建（需联网 apt/下载 Zig），产物在 dist/
bash scripts/build-release.sh

# 只验证单个目标的链接配置（不改动 .cnb.yml 也能提前发现问题）
cargo build --release -p req-guard --target x86_64-unknown-linux-musl        # 默认变体
cargo build --release -p req-guard --target aarch64-unknown-linux-musl --features tui

# 校验产物：ELF magic + 架构（3e00=x86-64, b700=aarch64）
od -An -tx1 -N4 target/x86_64-unknown-linux-musl/release/req-guard
```

**如何验证发布结果**

| 检查项 | 方法 | 期望 |
|---|---|---|
| 附件齐全 | Release 页面 / `cnbcool/attachments` 的 `FILES` 输出 | 10 个二进制 + `SHA256SUMS` |
| 完整性 | `sha256sum -c SHA256SUMS` | 全部 OK |
| 版本号 | `./req-guard-<target> -V` | `req-guard <tag 版本号>` |
| 可用性 | Linux：`./req-guard-x86_64-unknown-linux-musl check`；Windows：`req-guard-…-windows-gnu.exe check` | 正常输出门禁判定（退出码 0/1） |
| 静态性 | `ldd req-guard-aarch64-unknown-linux-musl` | `not a dynamic executable` |


## 文档

设计与规范文档统一放在 [`docs/`](docs/README.md)：

| 目录 | 内容 |
| --- | --- |
| [`docs/需求/`](docs/需求) | 需求分析报告（功能范围 / 命令规格 / 验收口径） |
| [`docs/设计/`](docs/设计) | 技术方案、UI 架构细化方案、GUI 跨平台方案选型 |
| [`docs/规范/`](docs/规范) | AI 工具合规保证规范（三层 + 审批锁 / 部署验收清单） |
| [`docs/提案/`](docs/提案) | AI 协同审核 GUI 自动弹出（未实现，含 P0–P5 落地路径） |

完整索引与文档清理记录见 [`docs/README.md`](docs/README.md)。

## 源码结构

```
core/  req-guard-core   零依赖 lib：error / requirement / comment / gate / status / ui_mode
cli/   req-guard        唯一 bin：main + cli（参数解析）+ render（文本渲染）
tui/   req-guard-tui    终端界面 lib：app（状态机）+ ui（渲染）
gui/   req-guard-gui    桌面界面 lib：app + fonts（内嵌 Noto Sans SC 子集）
```

判定逻辑只有一份，在 `core`；三个前端只负责渲染——这是"GUI 与 CLI 不会行为漂移"的根本保证。

中文字体：`gui/assets/NotoSansSC-Regular.otf`（Noto Sans SC 子集，SIL OFL 许可，见
`gui/assets/OFL-NOTO.txt`）；用 `scripts/make_font_subset.py` 可重新生成。

详细设计见 [`docs/设计/技术方案.md`](docs/设计/技术方案.md)、[`docs/需求/需求分析报告.md`](docs/需求/需求分析报告.md)；
架构决策见 dev-scaffold `架构决策记录.md` ADR-001。

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

## 命令一览

`init` `create` `approve` `reject` `comment` `resolve` `status` `list` `comments` `check` `install` `bypass` `ui`
（`req-guard -h` 查看完整参数；`-p` 指定项目根；身份回退环境变量 `REQ_GUARD_REVIEWER`）

## 门禁管理台（TUI）

```bash
cargo build -p req-guard --features tui   # 带 TUI 的构建（默认构建不含界面依赖）
req-guard ui            # 自动探测：Windows/macOS → GUI（规划中），SSH 会话 → TUI
req-guard ui --tui      # 强制终端界面
```

键位：`↑↓` 选择需求 · `←→/Tab` 切段 · `a` 批准 · `r` 打回 · `n` 新建 · `g` 门禁检查 ·
`b` 应急绕过 · `L` 审计日志 · `R` 刷新 · `?` 帮助 · `q` 退出。

> 界面只做**管理台**：状态读写全部走 core，与 CLI 行为完全等价（不会出现"界面放行、CLI 拦截"）。
> 清单正文在 TUI 中**只读**——正文由 AI/编辑器维护，界面只负责审核决策。

## 与 gates-toolkit 的关系

- 由 `setup-gates` 装配（片段 `030-reqguard`，`hookctl register ... --priority 30 --mode block`）
- dev-scaffold `new` 默认启用，`--no-req-guard` 关闭
- 拦截脚本随仓库提交（`.gates/hooks/`）——保证 clone 即生效；脚本缺失时 **fail-closed**

## 目录

```
.gates/
├── req-guard.yaml               # 门禁声明
├── requirements/                # REQ-00N-*.md 清单 + *.comments.md 评论
├── hooks/req-guard-check.{sh,ps1}
└── audit/gate-audit.log         # 审计（不入库）
```

## 构建与验证

工程为 cargo workspace：`core`（零依赖 lib）+ `cli`（唯一 bin）+ `tui`（界面 lib）。

```bash
cargo build --release                  # 默认只构建 core + cli：零 UI 依赖、秒级
cargo build -p req-guard --features tui --release   # 含 TUI
cargo zigbuild --target x86_64-pc-windows-gnu   # 交叉编译（本机既有工作流）

cargo fmt --all                        # 格式
cargo clippy --workspace --all-targets -- -D warnings   # 静态检查（零警告为门槛）
cargo test --workspace                 # 单元测试（44 用例）
python scripts/verify_gate.py          # 拦截脚本真机场景（10 场景）
```

> **Windows + Git Bash 注意**：`/usr/bin/link`（GNU coreutils）会遮蔽 MSVC 的 `link.exe`，
> 直接 `cargo build` 会失败。需把 MSVC `bin/Hostx64/x64` 前置到 `PATH`，并设置 `LIB`
> 指向 MSVC `lib/x64` 与 Windows Kits 的 `um/x64`、`ucrt/x64`。

## 源码结构

```
core/  req-guard-core   零依赖 lib：error / requirement / comment / gate / status / ui_mode
cli/   req-guard        唯一 bin：main + cli（参数解析）+ render（文本渲染）
tui/   req-guard-tui    终端界面 lib：app（状态机）+ ui（渲染）
```

判定逻辑只有一份，在 `core`；三个前端只负责渲染——这是"GUI 与 CLI 不会行为漂移"的根本保证。

详细设计见《技术方案.md》《需求分析报告.md》；架构决策见 dev-scaffold `架构决策记录.md` ADR-001。

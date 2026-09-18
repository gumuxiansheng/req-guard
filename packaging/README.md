# req-guard v{{VERSION}} —— 发布包

> **AI 在写代码前必须先走完「需求分解 → 技术方案 → 测试计划」三段审核，否则写不了文件。**
> req-guard 是 gates-toolkit 家族的**流程门禁**（管"该不该写"），与 sql-guard / java-guard（管"写得对不对"）互补。

本目录是一个**可直接分发**的发布包：内置各平台预编译二进制、安装脚本、用户手册与 CI 接入模板。
下载后无需 Rust 工具链、无需联网安装依赖，按《安装指南》执行两条命令即可运行。

---

## 1. 版本信息

| 项 | 值 |
| --- | --- |
| 版本 | `{{VERSION}}` |
| 构建日期 | {{BUILD_DATE}} |
| 源码提交 | `{{GIT_COMMIT}}` |
| 构建工具链 | {{RUSTC_VERSION}} |
| 许可 | 源码许可见仓库 `LICENSE`（当前仓库未附带）；第三方声明见 `THIRD-PARTY-NOTICES.md` |

版本自证（任一平台通用）：

```bash
req-guard -V     # → req-guard {{VERSION}}
```

二进制中的版本号是**编译期写入**的，与 `Cargo.toml` 的 `[workspace.package] version` 严格一致；
若你手中的二进制 `-V` 与上面表格不符，说明拿到的不是本包产物，请重新下载。

---

## 2. 目录结构

```
req-guard-v{{VERSION}}/
├── README.md                    ← 本文件（总览 + 快速开始）
├── VERSION                      ← 机器可读的版本与构建信息
├── CHANGELOG.md                 ← 版本变更记录
├── SHA256SUMS.txt               ← 全部文件的 SHA-256 校验清单
├── THIRD-PARTY-NOTICES.md       ← 第三方组件许可声明（含内嵌字体 OFL）
│
├── bin/                         ← 预编译二进制（按平台分目录，互不混淆）
│   ├── windows-x86_64/
│   │   ├── req-guard.exe            CLI（零 UI 依赖，最小）
│   │   ├── req-guard-ui.exe         CLI + 终端界面（TUI）
│   │   └── req-guard-gui.exe        CLI + 桌面界面（GUI，含内嵌中文字体）
│   ├── linux-x86_64/
│   │   ├── req-guard                CLI（musl 静态链接）
│   │   └── req-guard-ui             CLI + TUI（musl 静态链接）
│   └── linux-aarch64/
│       ├── req-guard                CLI（musl 静态链接）
│       └── req-guard-ui             CLI + TUI（musl 静态链接）
│
├── docs/                        ← 文档
│   ├── 安装指南.md                 逐步安装、升级、卸载、CI 部署
│   ├── 用户手册.md                 全部命令、工作流、门禁三层模型、界面操作
│   ├── 校验说明.md                 校验和怎么验、版本怎么对、源码怎么复现
│   └── 常见问题.md                 故障排查速查表
│
├── scripts/                     ← 安装与运维脚本
│   ├── install.sh / install.ps1        一键安装（选平台、选变体、写入 PATH）
│   ├── uninstall.sh / uninstall.ps1    卸载
│   ├── verify-checksums.sh / .ps1      校验包内文件完整性
│   └── selfcheck.sh / .ps1             安装后冒烟自检（放行 + 拦截两条路径）
│
└── templates/                   ← 接入模板
    ├── ci/req-guard-ci.yml            L3 流水线门禁样例（GitHub Actions）
    └── ci/req-guard-ci.yml.gitlab     同上，GitLab CI 版
```

> **二进制命名原则**：目录名用「平台-架构」这种人类可读形式（`windows-x86_64`），
> 目录内文件名统一为 `req-guard[.exe]` 或 `req-guard-ui[.exe]`，避免把 target 三元组
> （如 `x86_64-unknown-linux-musl`）暴露给用户——那是构建概念，不是使用概念。
> 每个平台的真实 target 记录在 `VERSION` 文件的「产物清单」中。

---

## 3. 三分钟上手

### Windows（PowerShell）

```powershell
# 在本目录（发布包根目录）执行
powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -AddPath
# 重开一个终端，或刷新 PATH 后：
req-guard -V
```

### Linux / macOS（bash）

```bash
bash scripts/install.sh            # 默认装到 ~/.local/bin（无 root 权限也可）
export PATH="$HOME/.local/bin:$PATH"
req-guard -V
```

### 接入你的项目

```bash
cd your-project
req-guard init                                  # 生成 .gates/ + AI 工具 hook + pre-commit
req-guard create -t "用户登录改造"               # → REQ-001
# AI 填写 .gates/requirements/REQ-001-*.md 三段正文
req-guard approve REQ-001 --step decomposition --reviewer 寇工
req-guard approve REQ-001 --step solution      --reviewer 寇工
req-guard approve REQ-001 --step testplan      --reviewer 寇工
req-guard check                                 # 退出码 0 = 解锁，AI 方可写代码
```

可选的自检（验证"放行"与"拦截"两条路径都通）：

```bash
bash scripts/selfcheck.sh        # Windows 用 scripts\selfcheck.ps1
```

---

## 4. 下载后请先校验

```bash
# Linux / macOS
sha256sum -c SHA256SUMS.txt

# Windows PowerShell
Get-FileHash -Algorithm SHA256 bin\windows-x86_64\req-guard.exe
# 与 SHA256SUMS.txt 中对应行比对
```

一键校验：

```bash
bash scripts/verify-checksums.sh     # 或 scripts\verify-checksums.ps1
```

详见 [`docs/校验说明.md`](docs/校验说明.md)。

---

## 5. 该读哪份文档

| 你是谁 | 读什么 |
| --- | --- |
| 第一次安装 | `docs/安装指南.md` |
| 日常使用 / 想知道全部命令 | `docs/用户手册.md` |
| 要在 CI 里强制门禁 | `docs/安装指南.md` §6 + `templates/ci/` |
| 装完想确认没装错 | `scripts/selfcheck.sh` / `selfcheck.ps1` |
| 出问题了 | `docs/常见问题.md` |
| 关心这包有没有被篡改 | `docs/校验说明.md` |

---

## 6. 平台支持矩阵

| 平台 | 架构 | CLI | TUI | GUI | 运行时依赖 |
| --- | --- | --- | --- | --- | --- |
| Windows 10/11 | x86_64 | ✅ | ✅ | ✅ | 无（静态链接 CRT，无需 VC++ 运行库） |
| Linux（任意发行版） | x86_64 | ✅ | ✅ | ❌ | 无（musl 静态链接，无 glibc 版本要求） |
| Linux（任意发行版） | aarch64 | ✅ | ✅ | ❌ | 无（musl 静态链接） |
| macOS | x86_64 / aarch64 | ⚠️ | ⚠️ | ❌ | 需从源码构建（`cargo build --release`），见安装指南 §7 |

- **GUI 只提供 Windows 版**：eframe/wgpu 依赖各平台图形栈，无法交叉编译，
  macOS/Linux 的 GUI 需在本机 `cargo build -p req-guard --features gui --release` 自行构建。
- **macOS 未提供预编译包**：缺少 Apple 平台交叉构建链路（需 macOS 执行机或 Zig + SDK）。
  命令行版本纯 Rust、无系统依赖，本机一条 `cargo build --release` 即可获得。

---

## 7. 许可与第三方声明

- req-guard 源码许可：见源码仓库的 `LICENSE`（本包未重复附带，以仓库为准）。
- 内嵌中文字体 Noto Sans SC（仅 GUI 变体）遵循 SIL Open Font License 1.1，
  全文见 `THIRD-PARTY-NOTICES.md`。
- 其他 Rust 依赖的许可清单同样见 `THIRD-PARTY-NOTICES.md`。

---

## 8. 获取源码 / 反馈

- 源码仓库：`git clone <你的仓库地址>`（含 `docs/需求`、`docs/设计`、`docs/规范` 全套设计文档）
- 从源码构建：`cargo build --release`（默认零 UI 依赖，秒级）；
  界面变体：`cargo build -p req-guard --features tui|gui|full --release`
- 问题反馈：请在仓库提交 Issue，并附上 `req-guard -V` 与 `.gates/audit/gate-audit.log` 末尾 20 行。

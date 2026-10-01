# req-guard 项目长期备忘

## 项目定位
gates-toolkit 家族的**流程门禁**（管"AI 该不该写"），与 sql-guard/java-guard（管"写得对不对"）互补。
核心：需求分解 → 技术方案 → 测试计划，三段逐段批准 + 阻塞评论未 resolve 即硬拦截。

## 约定
- **零外部依赖**是立身之本，core 永远不加 `[dependencies]`；UI 走 feature 门控的 workspace（core/cli/tui/gui）。
- Rust stable（rust-toolchain.toml 固定）。
- 中文注释与提示，结论先行。
- 目录：`.gates/`（入库：yaml 声明 + 清单 + 评论 + 拦截脚本）/`gates-tools/`（不入库：二进制与渲染产物）。
- 拦截判定**唯一真相**在 `.gates/hooks/req-guard-check.{sh,ps1}` 脚本，`req-guard check` 只透传退出码。
- req-guard 是安全机制 → **fail-closed**（区别于 sql-guard/java-guard 的 fail-open）。
- **文档统一放 `docs/`**（2026-09-18 归拢）：`需求/` `设计/` `规范/` `提案/` + `docs/README.md` 索引；
  根目录只留 `README.md`（项目入口）；`.workbuddy/memory/` 是 AI 工作记忆，不并入 docs。
  新文档只进对应分类；**引用代码位置写"文件名 + 符号名"，不写行号**（行号必然腐坏）；
  验证数字（用例/场景数）只记快照日期，以实跑为准。
- **版本号唯一真相 = 根 `Cargo.toml` 的 `[workspace.package] version`**，但 4 个成员 crate
  （core/cli/tui/gui）的 `version` 是**硬编码**而非 `version.workspace = true` → bump 必须 5 处一起改；
  再加 `Cargo.lock`（构建自刷）+ 两份 CI 样例（`templates/ci/req-guard-ci.yml` 的 `VER`、
  `packaging/templates/ci/*.gitlab` 的 `REQ_GUARD_VERSION`）+ `packaging/CHANGELOG.md`。
  发布包文档一律 `{{VERSION}}` 占位符，由 `scripts/make-release-package.sh` 从 Cargo.toml 解析注入，
  **不要手改**。（2026-09-21 统一到 v0.1.4）

## 已完成（截至 2026-09-12）
- **workspace 已拆分**：`core`（零依赖 lib）/ `cli`（唯一 bin）/ `tui`（lib）/ `gui`（lib）；
  `default-members = ["core","cli"]` → 根目录 `cargo build` 仍零依赖秒级。
- CLI 核心（13 命令，含 `ui`）+ **TUI 与 GUI 门禁管理台均可用**；**44 单测**（core 39 / tui 5）+ 10 脚本场景全绿；
  `clippy --workspace -D warnings` 与 `fmt --check` 零告警；GUI 真机截图验证通过（中文无豆腐块）。
- core 结构化 API：`status::{ReqStatus,StepStatus,ReqState,req_get,req_list}`、`gate::{GateVerdict,gate_check,audit_tail}`、
  `ui_mode::{UiMode,EnvFacts,detect}`；渲染分别在 `cli::render` / `tui::ui` / `gui::app`。
- `ui` 子命令自动探测 + `--gui/--tui` 强制 + **GUI 启动失败回退 TUI**（P4 已落地）。
- GUI 中文字体：Noto Sans SC 子集 1.51MB 内嵌（`gui/assets/`，SIL OFL），
  生成脚本 `scripts/make_font_subset.py`。
- 命名统一 `req-guard-*`（常量 `gate::HOOK_SH_REL`/`HOOK_PS1_REL` 为唯一真相）；ps1 强制 UTF-8 BOM；
  `strict_order` 真实生效；install 幂等追加 `.gitignore`；评论/绕过事件入审计；AI 只能 reply 不得 resolve。
- **跨平台原则**：渲染出的 hook 命令可平台相关（`hook_script_rel()` 按 `cfg!(windows)` 选 .ps1/.sh，唯一真相），
  但"配置是否已接入门禁"的 marker 必须**平台中立**（取脚本名词干 `req-guard-check`/`req-guard-deny`，不取后缀），
  否则 `install` 幂等检查与 `install --verify` 在各平台假红；测试断言不得写死 `.sh`/`.ps1`。
- 已 git init + 6 次提交（最新见 git log）+ GitHub Actions 三平台矩阵 + CNB 流水线 + `.gitattributes`(sh=LF)。
- **多平台 Release 流程已落地**（2026-09-12，对齐 sql-guard）：`.cargo/config.toml`（rust-lld + link-self-contained）、
  `scripts/build-release.sh`（5 目标 × 2 变体 + SHA256SUMS）、`.cnb.yml` 的 `"v*": tag_push`
  （`git:release` → 构建 → `cnbcool/attachments` 传 `./dist/*`）、`req-guard -V` 版本自证。
  产物命名 `req-guard[-ui]-<target>[.exe]`；tag 规则 `v<Cargo.toml version>`；镜像 `rust:1.88` + `RUSTUP_TOOLCHAIN=1.88.0`。
- **可分发发布包已落地**（2026-09-19，提交 `265fce3`/`3e4c79c`）：`packaging/` 存**包内容模板**
  （README/CHANGELOG/THIRD-PARTY-NOTICES + `docs/` 四份 + `scripts/` 四个脚本的 .sh/.ps1 双份 + CI 模板），
  `scripts/build-release-local.sh`（Windows 宿主 7 产物）+ `scripts/make-release-package.sh`/`make_release_package.py`
  组装 `dist/req-guard-v<ver>/` 与 4 个分平台归档。**模板入库、产物不入库**；模板里占位符 `{{VERSION}}` 等由构建脚本注入。
  对外目录名用人类可读的平台名（`windows-x86_64` / `linux-aarch64`），target 三元组只写进 `VERSION` 的产物清单。
  包内脚本职责：`install`（选平台变体 + 版本自证 + PATH + 可选 init）/ `uninstall` / `verify-checksums` /
  `selfcheck`（实跑"未过审必拦截、过审必放行"两条路径，无 git 时用最小 `.git` 桩降级并标注）。
- **CLI 约定：`-p/--path` 等通用选项必须写在子命令之后**（`req-guard status -p <dir>` ✅；
  `req-guard -p <dir> status` ❌ → "未知命令: -p" + 退出码 2）。写脚本文档前先用真二进制试参数顺序。
- **需求编号防冲突已落地**（2026-09-19）：规范 `docs/规范/需求编号防冲突命名规范.md`（推荐
  `REQ-<owner>-<YYYYMMDD>-<rrrr>`，owner 花名册唯一、随机段 Crockford32、第二段必须字母开头防
  next_id 污染）；工具侧 `core/src/idcheck.rs`（`ids --check` 三类检测，硬伤退出码 1 + `create --id`
  lint 不阻断）已实现并实测，存量 REQ-NNN 不迁移。

## 关键坑（复用）
- **测试里比较"路径后缀"必须先归一分隔符**：`Path::to_string_lossy().ends_with("a/b/c")`
  在 Linux 绿、**Windows 必红**（`join` 产出 `\`）。断言前 `.replace('\\', "/")`
  （生产代码里 `gate::is_requirement_doc` 已是这个约定）。2026-09-21 修过一次此类假红，
  原因是 Windows 本地很少跑全量 `cargo test --workspace`，CI 在 Linux 上永远看不见。
- **egui CollapsingHeader 传 `.open(Some(..))` 后点击被完全忽略**（源码 `if let Some(open) = open {...} else if clicked {toggle}`），
  要"选中段展开"必须自己接管 `header_response.clicked()`；批准/打回后要前进光标到下一个未通过段；已通过段不显示审核按钮。
- **GUI 真机点击验证（Windows）**：PowerShell Add-Type 被安全策略拦 → Python venv + ctypes；
  pyautogui.click 静默无效，必须 SendInput；GetWindowRect 含 DWM 不可见边框，egui 逻辑坐标 =
  (物理−客户区原点)/缩放，小按钮会脱靶（用探针打印 response.rect + pointer.latest_pos() 反推校准）；
  taskkill 输出 GBK 需 encoding='gbk'；GUI 常驻必须 run_in_background。
- `cargo test` 在 workspace 根**只跑 default-members**，TUI 测试要用 `cargo test --workspace`。
- TUI 测试摊平 TestBackend 缓冲必须按 `unicode-width` 跳格，否则 CJK 占位空格导致断言误判。
- **egui 0.36 API 大改**（详见 2026-09-11.md）：`App::update` → `logic`+`ui`；
  `TopBottomPanel/SidePanel` → `egui::Panel::{top,bottom,left}`（show 接收 &mut Ui）；
  `CollapsingHeader::open(Option<bool>)` 按值。核对 API 直接读 registry 源码最快。
- 字体源：noto-cjk 仓库结构已变，正确路径 `Sans/SubsetOTF/SC/`（jsdelivr 分发）。
- 本机构建见 2026-09-11.md：Git Bash 下需前置 MSVC bin 到 PATH 并设 LIB，否则 GNU `link` 抢先。
- **工具链下限 1.88.0**（ratatui 0.30.2 的 `rust-version`，edition 2024）——任何构建含 TUI 的环境（CI 镜像、
  本地 toolchain）都不得低于此版本；sql-guard 的 `rust:1.80` 镜像不可照抄。
- CLI+TUI 依赖链**零 C 代码**（无 cc/psm/stacker）→ 交叉编译无需 Zig 当 C 编译器；但
  windows-gnu **必须**装 mingw（换 `rust-lld` 会 `unable to find library -lkernel32`）。
- **GUI 无法从 Linux 交叉编译**（需 X11/Wayland/GTK），只能原生执行机构建，勿加入交叉矩阵。
- **GitHub windows runner 的 Python 步骤默认 cp1252**：中文 print / `subprocess(text=True)` 解码
  必崩（`UnicodeEncodeError` / `UnicodeDecodeError`）。脚本须自切 UTF-8
  （`reconfigure(encoding="utf-8", errors="replace")` + `subprocess(encoding="utf-8")`），CI 再兜 `PYTHONIOENCODING: utf-8`。
- **hook 配置的"是否已接入"判定必须按词干**：`marker_of` 取 `req-guard-check` / `req-guard-deny`
  （不带 .sh/.ps1），否则 Windows↔Linux 互装互验必假红；渲染出的命令才可平台相关。
- **`install --verify` 的 L3 体检是 fail-closed**：`init` 只把 CI 样例生成到 `.gates/ci/`，
  **有意不替用户写 CI 编排**；`enforce.ci` 默认 true → 样例没复制进 `.github/workflows/` 等目录时
  **必红（退出码 1，报"未检测到任何 CI 编排文件"）**。任何自举脚本/文档都不得断言"刚 init 完就全绿"，
  须**双向断言**（未接 CI 必红 / 复制样例后必绿）。2026-09-25 修 CI `gate-selfcheck` 时踩到（提交 0c0a173）。
- **`gui` crate 在 Linux 不参与 lint / test**：其 eframe 关了 `default-features`，连带
  `x11`/`wayland`/`winit/default` 后端未启用 → ubuntu 上编 winit 直接
  `compile_error!("The platform you're compiling for is not supported by winit")`
  （是 **feature 缺失**，不是缺系统库）。CI 用 matrix 维度 `gui_excl` 给 Linux 加
  `--exclude req-guard-gui`，与 `runner.os != 'Linux'` 的 GUI 构建门控对齐；本地 Linux 开发同理。
- **GitHub Actions 的 `runner` 上下文在 `jobs.<job_id>.env` 中不可用**（该处只允许
  `github`/`needs`/`strategy`/`matrix`/`vars`/`secrets`/`inputs`）→ 在 job 级 env 写
  `${{ runner.os == 'Linux' && ... || '' }}` 会直接判 workflow 无效
  （`Unrecognized named-value: 'runner'`）；只有 step 级 `env`/`if`/`run` 才有 `runner`。
  平台差异请用 matrix include 维度（渲染期即字面量）。2026-09-25（提交 171e57c）。
- **任何"由工具自动执行"的落盘脚本都必须补执行位**（`ensure_executable`，0o755）：
  git 在 Unix 上**静默跳过**不可执行钩子，`fs::write` 默认 0644 → L2 完全失效且 `--verify`
  只查内容会假绿。同理：`install` 的"已接入"提前返回分支也要补 chmod，否则旧仓库无法自愈；
  `verify_install` 必须单独查执行位（内容对 ≠ 生效）。Windows 侧 `is_executable` 恒 true 防假红。
- **脚本→Rust 的状态/字段传递一律用机器可读标记或真解析**，不要匹配人类可读文案、也不要用
  `sed` 抠 JSON 字段：JSON 允许 Unicode 转义（`.gates\u002f…comments.md` 与明文等价），
  正则漏判的方向恰好是"看着在拦、其实没拦"。解析下沉到 `core::json`（零依赖递归下降解析器）；
  脚本侧保留正则**兜底**（二进制不在 PATH 时），但兜底不完备属已知降级路径。
- `cargo clippy -D warnings` 不被识别，正确写法是 `cargo clippy --workspace --all-targets -- -D warnings`。
- **发布包四条硬约束**（详见 2026-09-19.md）：① windows-msvc 必须 `-C target-feature=+crt-static`
  （否则依赖 `VCRUNTIME140.dll`，"下载即运行"是假的）；② 执行位必须写进 **tar/zip 归档条目**
  （Windows 上 `chmod`/`os.chmod` 是空操作，只在本地设权限不会进归档；目录保持 0755，否则解压后进不去）；
  ③ Python 落盘一律 `newline="\n"`（默认 CRLF 会让 `.sh` 在 Linux 崩、让校验脚本读到 `路径\r` 而全部报 MISSING）；
  ④ 组装流程**全程不删除文件**（旧快照 `mv` 进 `dist/.trash/`、归档先写 `.tmp` 再 `os.replace`）——
  环境的批量删除保护会拦 `rm -rf` 甚至 Python 的 `rmtree/unlink`，只有不删才能无人值守跑完。
- **Windows 宿主跑批处理脚本的通用坑**：Git Bash 的 `/c/...` 传 Windows Python 前要 `cygpath -w`；
  PowerShell 的 `$psi.ArgumentList` 是 `Collection[string]`（无 `AddRange`，逐个 `Add`）且必须读走 stderr；
  `& native 2>$null | Out-Null` 会报"无法在管道中间运行文档"；`$env:PROCESSOR_ARCHITECTURE` 可能为空需三级兜底；
  判换行/编码用 Python 数字节，别用 `grep -c $'\r'`（会误报）。

- **审批锁是"白名单注入"，不是"默认拒绝"**（2026-09-29 Firedit 事故）：`gate.rs` 的 TOOLS 只登记
  claude / codebuddy / codex / cursor，`REQ_GUARD_AI_CTX` 只注入前两者的 `settings.json`。
  **WorkBuddy 不在列表**（它不读 `.codebuddy/`，`~/.workbuddy/settings.json` 也无 env/hooks 段）
  → AI 会话里无任何 `REQ_GUARD_*` 变量 → `auth::ensure_human` 判定"非 AI 上下文"→ approve 直接放行。
  → 已通过**方案 D（2026-09-29 落地）**修复：AI 通道实测 stdin/stdout/stderr **全非 TTY**
  → 严格模式（`auth.strict: true` 或 `REQ_GUARD_STRICT_AUTH=1`）下审批须出示凭据
  （令牌 或 终端一次性挑战码），**无凭据即拒**；`init` 模板已默认开启方案 D。**新工具不登记也不会静默失效**。

- **`req-guard token issue/revoke/status` 未走 `ensure_human`**（2026-09-29 发现 → 当晚已修）：
  AI 可自签令牌再自批，方案 B 一招破解。现 `token issue/revoke` 走 `auth::ensure_token_admin`
  ——已有有效令牌时重签/撤销**必须出示当前令牌**，严格模式下首次签发要求 TTY 在场。
- 同用户隔离下**没有本地不可伪造的审批凭据**：guard.cfg 与 AI 同用户可读写，AI 可写自己的哈希。
  方案 B 的强度只来自"令牌原文只有人知道"，最终兜底仍是**事后审计 + 服务端复核**（审批须与人类在
  PR/CI 侧的批准对应），不要把本地状态当自证。

- **Windows 上令牌熵曾退化（2026-09-30 已修）**：`token::random_hex` 旧实现把 `now`/`pid`
  在循环外取一次 → 32 字节全由单一 128 位状态派生，实际熵仅约 20~30 bit；而 `guard.cfg` 的
  哈希与 AI 同用户可读 → AI 可**离线暴力猜原文**再自批。现已改为逐字节采样时间抖动 + 混入
  ASLR 栈地址 + 滚动状态；弱熵平台由 CLI 提示缩短 TTL。**任何"AI 可离线验证猜测"的凭据，
  熵不够就等于没有**。
- **GUI/TUI 在严格模式下审批会被拒**（非 TTY）：唯一干净解法是从已 `export REQ_GUARD_TOKEN`
  的终端启动；⚠️ 令牌**不可**写进系统级环境变量（AI 同用户会继承 → 令牌失去意义）。

- **本机 `sed` 是 toybox 0.8.13，不是 GNU sed**（2026-10-01）：BRE 里 `[[:space:]]` 与 `v\?`
  组合会**静默解析失败返回空串**（`sed -n 's/^req-guard[[:space:]]*v\?//p'` 抽不到版本号，
  而去掉 `v\?` 就正常）。写**本机可跑**的解析脚本一律用 `awk`，不要用 `sed -n 's/.../\1/p'`
  （`build-release.sh` 里那套只在 Debian CI 的 GNU sed 下成立，本机跑会假红）。

## 构建脚本分工（2026-10-01 定型）
- `scripts/build-release.sh` = **CI 专用**（Debian 容器）：`apt-get` 装 mingw/binutils + 下载
  Zig/cargo-zigbuild，交叉 5 目标 × 2 变体；**在 macOS 上会直接卡在 apt-get**。
- `scripts/build-release-native.sh` = **本机专用**（新增）：只编 `rustc -vV` 自报的 host 三元组，
  不装依赖不联网，产出 `dist/req-guard-<host>` / `dist/req-guard-ui-<host>` + SHA256SUMS，
  命名与 CI 矩阵一致所以可同放 `dist/`；带产物 `--version` 自证。GUI 变体本机可用
  `cargo build --release -p req-guard --features full`（GUI 无法交叉编译），但脚本故意不做，
  以免 `dist/` 混入 CI 不发布的件。

## 已知未完成
- **P5 剩余**：GUI 产物的原生产物矩阵（GitHub Actions windows/macos 原生构建并发布）尚未做；
  CNB 流水线尚未在真实 tag 上端到端跑过一次（需先 push `v0.1.0`）。
- CNB 远端 `https://cnb.cool/mikezhu/req-guard` 已配置（remote 名 `cnb`），但尚未 push。

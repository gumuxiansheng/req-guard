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

## 关键坑（复用）
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
- **任何"由工具自动执行"的落盘脚本都必须补执行位**（`ensure_executable`，0o755）：
  git 在 Unix 上**静默跳过**不可执行钩子，`fs::write` 默认 0644 → L2 完全失效且 `--verify`
  只查内容会假绿。同理：`install` 的"已接入"提前返回分支也要补 chmod，否则旧仓库无法自愈；
  `verify_install` 必须单独查执行位（内容对 ≠ 生效）。Windows 侧 `is_executable` 恒 true 防假红。
- **脚本→Rust 的状态/字段传递一律用机器可读标记或真解析**，不要匹配人类可读文案、也不要用
  `sed` 抠 JSON 字段：JSON 允许 Unicode 转义（`.gates\u002f…comments.md` 与明文等价），
  正则漏判的方向恰好是"看着在拦、其实没拦"。解析下沉到 `core::json`（零依赖递归下降解析器）；
  脚本侧保留正则**兜底**（二进制不在 PATH 时），但兜底不完备属已知降级路径。
- `cargo clippy -D warnings` 不被识别，正确写法是 `cargo clippy --workspace --all-targets -- -D warnings`。

## 已知未完成
- **P5 剩余**：GUI 产物的原生产物矩阵（GitHub Actions windows/macos 原生构建并发布）尚未做；
  CNB 流水线尚未在真实 tag 上端到端跑过一次（需先 push `v0.1.0`）。
- CNB 远端 `https://cnb.cool/mikezhu/req-guard` 已配置（remote 名 `cnb`），但尚未 push。

# req-guard 项目长期备忘

（2026-10-04 归拢：从 16KB 压到规则本身，历史细节看 `.workbuddy/memory/YYYY-MM-DD.md`）

## 定位与约定
- gates-toolkit 家族的**流程门禁**（管"AI 该不该写"）：需求分解 → 技术方案 → 测试计划，逐段批准 +
  阻塞评论未 resolve 即硬拦截。**fail-closed**（区别于 sql-guard/java-guard 的 fail-open）。
- **core 零依赖**是立身之本（永不加 `[dependencies]`）；workspace = core(lib)/cli(唯一 bin)/tui/gui(lib)，
  `default-members = ["core","cli"]` → 根目录 `cargo build` 零依赖秒级。stable 固定（工具链**下限 1.88.0**）。
- 目录：`.gates/`（入库）/`gates-tools/`（不入库产物）；文档统一 `docs/{需求,设计,规范,提案}` + 索引；
  **引用位置写"文件名 + 符号名"，不写行号**；验证数字只记快照日期，以实跑为准。
- 拦截判定**唯一真相** = `.gates/hooks/req-guard-check.{sh,ps1}`，`req-guard check` 只透传退出码。
- 版本号唯一真相 = 根 `Cargo.toml` 的 `[workspace.package] version`；4 个成员 crate 是硬编码 → bump 共 5 处，
  另加 `Cargo.lock`、`templates/ci/*.yml` 的 `VER`、`packaging/templates/ci/*.gitlab` 的 `REQ_GUARD_VERSION`、
  `packaging/CHANGELOG.md`。发布包文档一律 `{{VERSION}}` 占位符由脚本注入，勿手改。
- CLI 通用选项**必须写在子命令之后**（`status -p <dir>` ✅ / `-p <dir> status` ❌ 退出码 2）。
- **SDD（spec-kit 类）接入**：唯一真相仍是 `.gates/requirements/REQ-*.md`；规则见
  `docs/规范/SDD产物接入规范.md`（R1 一份需求一份清单 / R2 零复制只引用 / R3 `specs/` 只当草稿源，
  apply 后删除或 gitignore）。**门禁对 `specs/**` 是"看不见"的**：`gate::pretool` 只对
  `.gates/requirements/*.md` 判 AllowDoc、对 `*.comments.md` 硬拦，其余路径 `Continue`，而 `resolve`
  G4 约定「未声明路径不额外拦」→ 让 SDD 产物进视野的唯一办法 = 第二段 `GATE:TOUCH` 声明 `specs/**`；
  细粒度规格（design/research/data-model/contracts）**必须落 `docs/设计/`**，否则方案段的
  `touch::cross_refs` 判 `target_missing` 直接拒批；`tasks.md ≠ 第 3 段测试计划`（`ac::lint` 硬拦
  Given/When/Then）。README 里的「SDD 契约」= codebound frontmatter 约定，**不是** SDD 工具。

## 跨平台（踩过就忘不掉）
- hook"是否已接入"的 marker **按词干**判定（`req-guard-check`/`req-guard-deny`，不带后缀）；只有渲染出的
  命令才可平台相关（`hook_script_rel()` 按 `cfg!(windows)`）。测试断言不得写死 `.sh`/`.ps1`。
- 比较路径后缀先归一分隔符（`.replace('\\', "/")`），否则 Windows 必红。
- "由工具自动执行"的落盘脚本必须补执行位（0o755）；`verify_install` 要单独查执行位（内容对 ≠ 生效）。
- `install --verify` 的 CI 体检 fail-closed：`init` 只生成 `.gates/ci/` 样例、**不替用户写 CI 编排** →
  刚 init 完必红。自举脚本/文档一律**双向断言**（未接 CI 必红 / 复制样例后必绿）。
- **GUI 不能从 Linux 交叉编译**（需 X11/Wayland/GTK）；`gui` crate 在 Linux 因 eframe 关 `default-features`
  致 winit `compile_error!` → CI 用 matrix 维度 `gui_excl` 排除。CLI+TUI 零 C 代码，但 windows-gnu 仍需 mingw。
- GitHub Actions：`runner` 上下文在 `jobs.<job_id>.env` **不可用**（改用 matrix include 维度）；
  windows runner 的 Python 默认 cp1252 → 脚本自切 UTF-8 + CI 设 `PYTHONIOENCODING: utf-8`。
- 发布包四条硬约束：① msvc 加 `+crt-static`；② 执行位写进 tar/zip **归档条目**；③ Python 落盘
  `newline="\n"`；④ 组装全程不删除文件（先写 `.tmp` 再 `os.replace`，旧件 `mv` 进 `.trash/`）。

## 安全（审批锁）
- 审批锁是**白名单注入**（只登记 claude/codebuddy/codex/cursor），新工具不登记即静默失效
  → 已用**方案 D** 兜底：严格模式下审批须出示凭据（令牌 / 终端挑战码），无凭据即拒；`init` 模板默认开启。
- `token issue/revoke` 必须走 `ensure_token_admin`（否则 AI 能自签再自批）；已有令牌时重签要出示当前令牌。
- 同用户隔离下**没有本地不可伪造的凭据** → 兜底是事后审计 + 服务端复核，别把本地状态当自证；
  凭据熵不足 = 没有（Windows 上曾退化到 ~20bit，可离线爆破）。
- GUI/TUI 严格模式下非 TTY 会被拒：从已 `export REQ_GUARD_TOKEN` 的终端启动；⚠️ 令牌不可写进系统环境变量。

## GUI（egui 0.36）
- 0.36 API 大改：`App::update` → `logic`+`ui`；`egui::Panel::{top,bottom,left}`；
  `CollapsingHeader::open(Option<bool>)` 按值传；`Visuals` **没有** `dark` 字段（用 `ctx.theme()`）。
- `CollapsingHeader` 传 `.open(Some(..))` 会忽略点击 → "选中段展开"要自己接管 `header_response.clicked()`。
- **只读但可选中 / 可复制**：`TextEdit::multiline(&mut text.as_str())`（egui 给 `&str` 的 TextBuffer
  `is_mutable()==false`、写入全空实现）；`interactive(false)` 会把 sense 降成 hover → 选不中、收不到 `Event::Copy`。
- 提示色一律走 `gui::palette::Tone`（浅/深两套，满足 WCAG AA，附图标；红=错/绿=成/琥珀=警/灰=中）；
  **不要再用 `Color32::RED/GREEN/YELLOW`**（黄 1.01、绿 1.29 对比度，浅底上等于看不见）。
- 离屏测试限制：galley 几何退化（量到 `rect` 是 0×0）、`cursor_from_pos` 恒返回第 0 字符、`clamp_cursor`
  会夹短选区 → 鼠标框选只能验"能进拖拽态"，选区内容改在状态层面验（可编辑的 TextEdit 同样如此）。
- 真机验证：Windows 用 Python ctypes + SendInput（pyautogui 静默无效）；macOS 用 `screencapture` + 临时
  `.with_always_on_top()`（否则窗口会被前台应用盖住），`sips --cropOffset` 裁剪。

## 工具链与命令
- `cargo test` 在 workspace 根**只跑 default-members** → 全量要用 `cargo test --workspace`。
- clippy 正确写法 `cargo clippy --workspace --all-targets -- -D warnings`（`-D warnings` 不能省）。
- 测试函数名含大写 ASCII 会被 clippy 判 `non_snake_case`（本机全中文名最省事）。
- 本机 `sed`/`grep` 是 **toybox**：BRE 的 `\|`、`v\?`、`[[:space:]]` 组合会静默失效 → 改用 `grep -E` 与 `awk`。
- 字体源 `noto-cjk` 的 `Sans/SubsetOTF/SC/`；子集脚本 `scripts/make_font_subset.py`（符号区 0x25A0–0x27BF）。

## 构建脚本分工
- `scripts/build-release.sh` = **CI 专用**（Debian 容器，交叉 5 目标 × 2 变体；macOS 上会卡在 apt-get）。
- `scripts/build-release-native.sh` = **本机专用**（只编 host 三元组、不联网；GUI 变体走 `--features full` 原生构建）。

## 未完成
- P5：GUI 产物的原生矩阵（GH Actions windows/macos）未做；CNB 流水线未在真实 tag 上端到端跑过
  （remote `cnb` = `https://cnb.cool/mikezhu/req-guard` 已配置、未 push）。

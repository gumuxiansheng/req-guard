//! AI 需求门禁：硬拦截的安装与执行。
//!
//! ## 拦截分层（越靠前越硬）
//! | 层 | 拦截点 | 效果 |
//! | --- | --- | --- |
//! | **L1 AI 工具 Hook** | `PreToolUse`（Write/Edit 等） | AI **根本无法写文件**——真正意义的"不准写代码" |
//! | L2 git pre-commit | 提交时 | 兜底：未过审不得入库 |
//! | L3 CI | `req-guard check` | 兜底：未过审不得合并 |
//!
//! L1 借鉴 teamai-cli 的"声明式 hooks.yaml → 注入各 AI 工具原生配置"范式：
//! 本模块把 [`HOOK_SH`] / [`HOOK_PS1`] 注入 `.claude/settings.json`、`.codebuddy/hooks.json` 等，
//! AI 工具在每次写文件前调用脚本，脚本返回非零即阻断。
//!
//! ## 逃逸阀
//! 完全堵死不现实（也危险），故提供**有时效、有痕**的应急绕过：`req-guard bypass`，
//! 生成 `.gates/.bypass`（含原因/操作人/过期时间），拦截脚本命中窗口时放行但**必须写审计日志**。

use crate::error::{GateError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 拦截脚本相对路径（**唯一真相**）：gates-toolkit 片段、pre-commit 块、
/// `req-guard check` 与 `req-guard install` 必须引用同一个名字。
pub const HOOK_SH_REL: &str = ".gates/hooks/req-guard-check.sh";
/// Windows 等价脚本相对路径。
pub const HOOK_PS1_REL: &str = ".gates/hooks/req-guard-check.ps1";

/// deny 包装脚本（POSIX）相对路径：供 Codex/Cursor 等把 `exit 1` 转成其能识别的
/// 拒绝（`exit 2`）——这些工具把非 `exit 0` 一律视为 fail-open（继续执行），
/// 直接复用 `check.sh` 会在拦截时静默放行。
pub const DENY_SH_REL: &str = ".gates/hooks/req-guard-deny.sh";
/// deny 包装脚本（Windows）相对路径。
pub const DENY_PS1_REL: &str = ".gates/hooks/req-guard-deny.ps1";

/// 拦截脚本命中应急绕过窗口时输出的**机器可读标记**（sh / ps1 两端都必须输出）。
///
/// 判定"本次放行是不是靠绕过"不能去匹配人类可读文案——文案一改判定就失效，
/// 而失效方向恰好是最坏的：绕过被伪装成"三段已批准"的正常放行。
/// 脚本额外输出本行，`gate_check` 据此置 [`GateVerdict::Pass::bypassed`]。
///
/// [`HOOK_SH`] / [`HOOK_PS1`] 内该字符串是**字面量**（脚本是 raw string，无法插值），
/// 由单测 `hook_脚本输出绕过标记` 锁定两端与本常量一致。
pub const BYPASS_MARKER: &str = "REQ_GUARD_BYPASS=1";

/// 当前平台注入工具配置时引用的拦截脚本**相对路径**（Windows→`.ps1`，其余→`.sh`）。
///
/// 与 [`run_hook`] 同规则：按**编译目标平台**选，不按"哪个文件存在"（install 在任意平台
/// 都会同时落盘 .sh/.ps1 作为跨平台资产）。`hook_json` 与测试断言共用此函数，
/// 避免平台差异在两个地方各写一遍而漂移。
fn hook_script_rel(use_deny: bool) -> &'static str {
    match (cfg!(windows), use_deny) {
        (true, true) => DENY_PS1_REL,
        (true, false) => HOOK_PS1_REL,
        (false, true) => DENY_SH_REL,
        (false, false) => HOOK_SH_REL,
    }
}

/// 某工具注入时，判断"配置已含门禁"的 marker 子串：deny 型工具注入的 command
/// 指向 deny 包装（不含 `req-guard-check` 字样），须按 derive 判定。
///
/// 取脚本名**词干**（去掉 `.sh` / `.ps1` 后缀）而非完整文件名：工具配置通常随仓库入库，
/// 同一仓库可能被 Windows（配置里落 `.ps1`）与 Linux/macOS（落 `.sh`）的开发者先后
/// install。若 marker 只认 `.sh`，Windows 侧 `install` 幂等检查会凭空冒出"请手工合并"
/// 提示、`install --verify` 会报不存在的 L1 缺口（跨平台假红）。
fn marker_of(prof: &ToolProfile) -> &'static str {
    let rel = if prof.use_deny {
        DENY_SH_REL
    } else {
        HOOK_SH_REL
    };
    rel.rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .unwrap_or("req-guard-check")
}

/// 创建父目录并写入文件，返回完整路径。
pub fn write_file(root: &Path, rel: &str, content: &str) -> Result<PathBuf> {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|e| GateError::Io {
            path: Some(parent.to_path_buf()),
            source: e,
        })?;
    }
    fs::write(&full, content).map_err(|e| GateError::Io {
        path: Some(full.clone()),
        source: e,
    })?;
    Ok(full)
}

/// 类 Unix：给文件补执行位（0755）。
///
/// **为什么必须**：git 在 Unix 上会**静默跳过**没有执行位的钩子。`install` 若只用
/// `fs::write` 落盘（默认 0644），pre-commit 就形同虚设——提交照常成功、没有任何提示，
/// 而 `install --verify` 只看内容不看权限，会给出"全部就位"的假绿。这正是本项目
/// 最危险的失效模式（"看起来在拦，实际没拦"）。
///
/// AI 工具配置里脚本以 `sh <script>` 调用，本不依赖执行位；一并设置是为了让人在
/// 终端直接 `./.gates/hooks/req-guard-check.sh` 也能跑（排查门禁时很常用）。
#[cfg(unix)]
fn ensure_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

/// 非 Unix（Windows）：无 POSIX 执行位模型，跳过。
#[cfg(not(unix))]
fn ensure_executable(_path: &Path) {}

/// 类 Unix：文件是否带任一执行位。
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// 非 Unix：Windows 由扩展名与策略决定是否可执行，此处无法判定。
///
/// 恒返回 true 而不是 false：Windows 侧本来就没有这个失效模式，
/// 返回 false 会在 CI 上制造无法修复的假红。
#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// 写入门禁**声明类**文件（`.gates/req-guard.yaml` / `.gates/README.md`）：
/// 已存在则**原样保留**——用户可能已按本项目情况调整过（例如关掉 strict_order），
/// 重复 install 不应把人的决策冲掉。
fn write_decl(root: &Path, rel: &str, content: &str, notes: &mut Vec<String>) -> Result<PathBuf> {
    let full = root.join(rel);
    if full.exists() {
        notes.push(format!(
            "{} 已存在，保留现有内容（如需重置请先删除该文件）",
            rel
        ));
        return Ok(full);
    }
    write_file(root, rel, content)
}

/// 去掉路径里的 `.`（保留语义、纯装饰）与 `..`（真的上一层级）。
///
/// 零依赖实现：`components()` 过滤后再重建。
/// 前缀（Windows 盘符 `C:`、UNC）不属于普通成分，必须原样推进。
fn normalize_path(p: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// 从 `start` 向上查找**项目根**：第一个含 `.gates/` 子目录的祖先（含自身）。
///
/// 存在理由：真实调用场景里 CWD 往往不是项目根——
///   - 双击 `req-guard-ui.exe`：CWD 是 exe 所在目录（如 `gates-tools/req-guard-ui/bin`）；
///   - 在 `src/`、`web/` 等子目录里执行 `req-guard status`。
///
/// 参数解析里 root 默认 `"."`，这些场景会全部落到错误目录，
/// 表现为「找不到需求 / 门禁看起来没生效」。
///
/// 上限 `max_up` 层，找不到返回 `None`（调用方应保持原 CWD，不能猜：
/// `init` 必须在"当前目录"建门禁，凭空跳到某个祖先目录是危险的）。
pub fn find_project_root(start: &Path, max_up: usize) -> Option<PathBuf> {
    // ★ 必须先转成绝对路径：相对路径**无法向上遍历**。
    //   `Path::new(".").parent()` 是 `Some("")`（空路径），再取其 parent 直接是 None，
    //   于是"从 CWD 向上找"会退化成"只看 CWD 自己"——双击 exe（CWD 是 exe 目录）
    //   时正是这个路径，表现为界面能起来但需求列表是空的。
    let start_abs = if start.is_absolute() {
        start.to_path_buf()
    } else {
        match std::env::current_dir() {
            // ★ normalize：`cwd.join(".")` 会留下尾随 `.`，原样返回会让
            //   后续 `root.join(".gates/…")` 显示成 `C:\proj\.\.gates\…`。
            Ok(cwd) => normalize_path(cwd.join(start)),
            Err(_) => start.to_path_buf(),
        }
    };
    let mut cur = Some(start_abs.as_path());
    for n in 0..=max_up {
        let p = cur?;
        if p.join(".gates").is_dir() {
            return Some(p.to_path_buf());
        }
        if n == max_up {
            break;
        }
        cur = p.parent();
    }
    None
}

/// 读取 `.gates/req-guard.yaml` 的 `strict_order` 开关。
///
/// 零依赖实现：逐行匹配 `strict_order:`（跳过注释行）。
/// 文件缺失或未声明时返回 **true**——门禁是安全机制，宁严不宽（fail-closed）。
pub fn strict_order(root: &Path) -> bool {
    let path = root.join(".gates/req-guard.yaml");
    let Ok(content) = fs::read_to_string(&path) else {
        return true;
    };
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with('#') {
            continue;
        }
        if let Some(v) = l.strip_prefix("strict_order:") {
            return !v.trim().eq_ignore_ascii_case("false");
        }
    }
    true
}

/// 读取 `.gates/req-guard.yaml` 的 `archive.after_days`（缺省 **30**）。
///
/// 与 [`strict_order`] 同为零依赖逐行解析：只在 `archive:` 区块内匹配 `after_days:`。
/// 语义：done 满 N 天后，`req-guard done` 成功时自动物理归档；`0` = done 即归档。
pub fn archive_after_days(root: &Path) -> u32 {
    let path = root.join(".gates/req-guard.yaml");
    let Ok(content) = fs::read_to_string(&path) else {
        return 30;
    };
    let mut in_archive = false;
    let mut found: Option<u32> = None;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if t == "archive:" {
            in_archive = true;
            continue;
        }
        if in_archive {
            if line.starts_with(' ') {
                if let Some(v) = t.strip_prefix("after_days:") {
                    let v = v.split('#').next().unwrap_or("").trim();
                    if let Ok(n) = v.parse::<u32>() {
                        found = Some(n); // 重复键：后者覆盖前者（贴合常见 YAML 语义）
                    }
                }
            } else if !t.is_empty() {
                in_archive = false; // 缩进结束，离开 archive 区块
            }
        }
    }
    found.unwrap_or(30)
}

/// 内置 AI 工具配置映射（借鉴 teamai：声明式注入各工具**原生**配置）。
///
/// 各工具 hook schema 存在差异（尤其事件名、matcher、拦截语义）——必须注入
/// 工具原生格式，否则工具不识别、hook 静默不加载（见《AI工具合规保证规范.md》§4.1）。
struct ToolProfile {
    name: &'static str,
    /// 该工具的原生 hook 配置文件（相对项目根）。
    config: &'static str,
    /// hook 事件名：Cursor 用小驼峰 `preToolUse`；其余（Claude/CodeBuddy/Codex）用 `PreToolUse`。
    event: &'static str,
    /// PreToolUse matcher（匹配**工具名**）：Claude/CodeBuddy/Cursor 是文件写工具
    /// `Write|Edit|MultiEdit|NotebookEdit`；Codex 的文件写入走 `apply_patch`。
    matcher: &'static str,
    /// 拦截编码是否需走 deny 包装：Claude/CodeBuddy 原生以 `exit 0/1` 拦截，
    /// 而 Codex/Cursor 把非 `exit 0` 视为 fail-open（继续执行），须把 `exit 1`
    /// 转成它们能识别的拒绝（`exit 2`）——见 [`DENY_SH_REL`]。
    use_deny: bool,
    /// 该工具原生配置支持**会话级环境变量注入**（`settings.json` 的顶层 `env` 段）
    /// ——注入 [`crate::auth::AI_CTX_ENV`] 后审批锁才能覆盖其会话内 Shell 路径。
    env_capable: bool,
}

const TOOL_PROFILES: [ToolProfile; 4] = [
    // Claude Code：settings.json，事件大驼峰，文件写工具名，原生 exit 0/1，支持 env 注入
    ToolProfile {
        name: "claude",
        config: ".claude/settings.json",
        event: "PreToolUse",
        matcher: "Write|Edit|MultiEdit|NotebookEdit",
        use_deny: false,
        env_capable: true,
    },
    // CodeBuddy：settings.json（非 hooks.json！与 Claude Code 同构 + 顶层 env 段）
    ToolProfile {
        name: "codebuddy",
        config: ".codebuddy/settings.json",
        event: "PreToolUse",
        matcher: "Write|Edit|MultiEdit|NotebookEdit",
        use_deny: false,
        env_capable: true,
    },
    // Codex：hooks.json，事件大驼峰，文件写入走 apply_patch，非 machine exit 需 deny 包装
    ToolProfile {
        name: "codex",
        config: ".codex/hooks.json",
        event: "PreToolUse",
        matcher: "apply_patch",
        use_deny: true,
        env_capable: false,
    },
    // Cursor：hooks.json，事件小驼峰 preToolUse，文件写工具名，需 deny 包装
    ToolProfile {
        name: "cursor",
        config: ".cursor/hooks.json",
        event: "preToolUse",
        matcher: "Write|Edit|MultiEdit|NotebookEdit",
        use_deny: true,
        env_capable: false,
    },
];

/// 支持的 AI 工具名。
pub fn known_tools() -> Vec<&'static str> {
    TOOL_PROFILES.iter().map(|t| t.name).collect()
}

/// 默认注入的工具：**全部内置 profile**（claude / codebuddy / codex / cursor）。
///
/// L1 覆盖的边界要诚实：只有能注册 PreToolUse 式 hook 的工具才能被 L1 拦到；
/// 无 hook 机制的 agent（Copilot/Trae/Gemini 等）不在 profile 内，只能靠 L2/L3 兜底。
pub fn default_tools() -> Vec<String> {
    vec![
        "claude".to_string(),
        "codebuddy".to_string(),
        "codex".to_string(),
        "cursor".to_string(),
    ]
}

/// 校验工具名合法。
pub fn validate_tools(tools: &[String]) -> Result<()> {
    for t in tools {
        if t == "none" {
            continue;
        }
        if !TOOL_PROFILES.iter().any(|p| p.name == t.as_str()) {
            return Err(GateError::Validation(format!(
                "未知 AI 工具: {}（可用 {} 或 none）",
                t,
                known_tools().join(", ")
            )));
        }
    }
    Ok(())
}

/// 安装 AI 需求门禁：生成脚本与声明文件 → 注入 AI 工具 hook → 追加 git pre-commit。
///
/// 幂等：已含 `req-guard-check` 的配置/钩子不会重复写入；
/// 已存在但不含门禁配置的 AI 工具配置文件**不会被覆盖**（避免破坏用户既有配置），
/// 改为在 `notes` 中提示手工合并。
pub fn install(root: &Path, tools: &[String], verbose: bool) -> Result<Vec<PathBuf>> {
    validate_tools(tools)?;
    let mut created = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    created.push(write_decl(
        root,
        ".gates/req-guard.yaml",
        REQ_GUARD_YAML,
        &mut notes,
    )?);
    created.push(write_decl(
        root,
        ".gates/README.md",
        REQ_GUARD_README,
        &mut notes,
    )?);
    created.push(write_file(root, HOOK_SH_REL, HOOK_SH)?);
    created.push(write_file(root, HOOK_PS1_REL, &ps1_with_bom())?);
    // deny 包装（Codex/Cursor 专用）随安装一并落盘
    created.push(write_file(root, DENY_SH_REL, DENY_SH)?);
    created.push(write_file(root, DENY_PS1_REL, &deny_ps1_with_bom())?);
    // 落盘后补执行位：git 会静默跳过不可执行的钩子（详见 ensure_executable）。
    for rel in [HOOK_SH_REL, HOOK_PS1_REL, DENY_SH_REL, DENY_PS1_REL] {
        ensure_executable(&root.join(rel));
    }
    created.push(write_file(root, ".gates/requirements/.gitkeep", "")?);
    created.push(write_file(root, ".gates/audit/.gitkeep", "")?);
    // L3 接入样例（§4.3）：随安装生成到 .gates/ci/，使用方复制进 .github/workflows/
    created.push(write_file(
        root,
        ".gates/ci/req-guard-ci.yml",
        include_str!("../../templates/ci/req-guard-ci.yml"),
    )?);

    for t in tools {
        if t == "none" {
            continue;
        }
        inject_tool(root, t, &mut created, &mut notes)?;
    }

    append_pre_commit(root, &mut created, &mut notes)?;
    append_gitignore(root, &mut created, &mut notes)?;

    // L3 强提示：enforce.ci 默认 true，但 install 不会替用户写 CI 编排（那是仓库配置），
    // 未接入时提示人工接入——避免"装完只有两层"的默认现状被误当作三层已齐。
    if !verify_ci(root).is_empty() {
        notes.push(
            "L3 提示：未检测到 CI 编排调用 req-guard——请把 .gates/ci/req-guard-ci.yml \
             复制进 CI（如 .github/workflows/）并设为必需状态检查，或在 req-guard.yaml \
             显式设 enforce.ci: false 声明放弃"
                .into(),
        );
    }

    if verbose {
        for n in &notes {
            println!("   提示  : {}", n);
        }
    }
    Ok(created)
}

/// PowerShell 脚本落盘内容：**必须带 UTF-8 BOM**。
///
/// Windows PowerShell 5.1 读取无 BOM 的文件时按当前 ANSI 代码页（中文 Windows 为 GBK）解码，
/// 脚本里的中文提示会变成乱码并破坏字符串引号 → `ParserError`，导致脚本**一行都不执行**，
/// 进程退出码恒为 1。对门禁而言这是最危险的失效：看起来"拦住了"，实际是脚本崩了，
/// 即使三段已全部批准也一律拦截（假拦截）。BOM 明确宣告编码后即恢复正常。
pub fn ps1_with_bom() -> String {
    format!("\u{feff}{}", HOOK_PS1)
}

/// deny 包装（PowerShell）落盘内容：同样必须带 UTF-8 BOM（见 [`ps1_with_bom`]）。
pub fn deny_ps1_with_bom() -> String {
    format!("\u{feff}{}", DENY_PS1)
}

/// 文件是否以 UTF-8 BOM 开头。
pub fn has_utf8_bom(path: &Path) -> bool {
    match fs::read(path) {
        Ok(b) => b.starts_with(&[0xEF, 0xBB, 0xBF]),
        Err(_) => false,
    }
}

/// 追加一行门禁审计日志到 `.gates/audit/gate-audit.log`。
///
/// 需求口径是"拦截/放行/绕过/**评论**事件全部入审计"——拦截与绕过由脚本与 [`bypass`] 写入，
/// 评论类事件（新增/关闭）由 [`crate::comment`] 调用本函数补齐，避免只留在清单内部的
/// `GATE:AUDIT` 区块里、无法集中审计。
pub fn audit(root: &Path, entry: &str) {
    let dir = root.join(".gates/audit");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let log = dir.join("gate-audit.log");
    let line = format!("{} {}\n", now_str(), one_line(entry));
    let mut content = fs::read_to_string(&log).unwrap_or_default();
    content.push_str(&line);
    // 审计失败不阻断主流程（门禁是安全机制，但审计是旁路证据）。
    let _ = fs::write(&log, content);
}

/// 入库审计台账（§4.6 审计可信化）：关键审批事件的**版本化**记录。
///
/// 与本机 `gate-audit.log`（`.gitignore` 忽略、clone 不可见）不同，
/// 本文件随仓库提交——"谁在何时批了什么 / 是否绕过"在 PR diff 中可直接复核。
/// 写入方：approve/reject（[`crate::requirement::review`]）、resolve /
/// 阻塞性评论新增（[`crate::comment`]）、bypass（[`bypass`]）。
pub const LEDGER_REL: &str = ".gates/audit/ledger.md";

/// 审计摘要文件（[`audit_digest`] 追加写入），随仓库提交。
pub const DIGEST_REL: &str = ".gates/audit/DIGEST";

/// 关键事件追加到入库台账：Markdown 表格，一行一事件。
pub fn audit_ledger(root: &Path, entry: &str) {
    let path = root.join(LEDGER_REL);
    if let Some(dir) = path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let mut content = fs::read_to_string(&path).unwrap_or_default();
    if content.is_empty() {
        content.push_str("# 审计台账（关键审批事件，随仓库提交）\n\n");
        content.push_str("| 时间 | 事件 |\n| --- | --- |\n");
    }
    // 表格行：压单行 + 转义竖线（reason 等自由文本可能含 |）
    let safe = one_line(entry).replace('|', "\\|");
    content.push_str(&format!("| {} | {} |\n", now_str(), safe));
    let _ = fs::write(&path, content);
}

/// 生成审计摘要：对本机 `gate-audit.log` 计算 SHA-256，追加到入库的
/// `.gates/audit/DIGEST`（时间 / 行数 / 摘要）。
///
/// 台账让人在 PR 里看清轨迹，摘要让**本机日志可被比对**——任何事后篡改
/// （删改拦截/放行记录）都会使哈希对不上。返回 `(DIGEST 路径, 摘要, 日志行数)`。
pub fn audit_digest(root: &Path) -> Result<(PathBuf, String, usize)> {
    let log = root.join(".gates/audit/gate-audit.log");
    if !log.exists() {
        return Err(GateError::Validation(format!(
            "尚无审计日志（{}）；产生拦截/审批事件后再执行 audit-digest",
            log.display()
        )));
    }
    let bytes = fs::read(&log).map_err(|e| GateError::Io {
        path: Some(log.clone()),
        source: e,
    })?;
    let lines = String::from_utf8_lossy(&bytes).lines().count();
    let hex = crate::digest::sha256_hex(&bytes);

    let path = root.join(DIGEST_REL);
    let mut content = fs::read_to_string(&path).unwrap_or_default();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("{} lines={} sha256={}\n", now_str(), lines, hex));
    fs::write(&path, content).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    Ok((path, hex, lines))
}

/// 门禁裁决结果（结构化）。
///
/// 裁决**只来自拦截脚本的退出码**（唯一判定逻辑），这里只是把脚本输出整理成
/// 前端可直接消费的形态——因此不会出现"CLI 放行、GUI 拦截"这类漂移。
#[derive(Debug, Clone)]
pub enum GateVerdict {
    /// 放行：三段已批准且无未解决的阻塞性评论（或命中应急绕过窗口）。
    Pass {
        summary: String,
        /// 脚本给出的放行说明（如"命中应急绕过窗口"），放行时也**必须**可达。
        ///
        /// 历史上 Pass 不带明细，导致绕过窗口内 CLI 只打印"三段已批准"——
        /// 而当时三段其实一段都没批。审核人与 CI 日志都被这句"事实性错误"误导。
        detail: Vec<String>,
        /// 本次放行是否**命中应急绕过窗口**。为 true 时 summary 必须显式写明，
        /// 不得伪装成正常放行。
        bypassed: bool,
    },
    /// 拦截：`detail` 为脚本给出的逐行原因（原样透传，便于 AI/人排查）。
    Block {
        summary: String,
        detail: Vec<String>,
    },
}

impl GateVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, GateVerdict::Pass { .. })
    }

    pub fn summary(&self) -> &str {
        match self {
            GateVerdict::Pass { summary, .. } | GateVerdict::Block { summary, .. } => summary,
        }
    }

    /// 脚本给出的明细（放行时为放行说明，可能为空；拦截时为逐行原因）。
    pub fn detail(&self) -> &[String] {
        match self {
            GateVerdict::Pass { detail, .. } | GateVerdict::Block { detail, .. } => detail,
        }
    }

    /// 是否因命中**应急绕过窗口**而放行。
    ///
    /// 调用方（CLI / TUI / GUI / CI）必须在放行提示里显式体现这一点：
    /// 放行原因不同，事后审计与责任归属完全不同。
    pub fn bypassed(&self) -> bool {
        matches!(self, GateVerdict::Pass { bypassed: true, .. })
    }
}

/// 执行拦截脚本，返回 `(是否放行, stdout, stderr)`。
///
/// 脚本是唯一判定逻辑（见《技术方案.md》§1.2），本函数只负责调用与收集输出。
fn run_hook(root: &Path) -> Result<(bool, String, String)> {
    let sh = root.join(HOOK_SH_REL);
    let ps1 = root.join(HOOK_PS1_REL);

    // 解释器按**编译目标平台**决定，而非按"哪个脚本文件存在"：`install` 在任意平台都会
    // 同时落盘 .sh/.ps1（跨平台资产）。若按"ps1 存在即调 powershell"，Linux/macOS 上
    // `req-guard check` 会误调不存在的 powershell 而恒败——这是"脚本崩了 / 真在拦"
    // 之外的第三种危险失效（命令错配）。故本机一律只执行本平台解释器对应的脚本。
    let is_windows = cfg!(windows);
    let (prog, script): (&str, std::path::PathBuf) = if is_windows {
        ("powershell", ps1.clone())
    } else {
        ("sh", sh.clone())
    };

    // 仅 Windows 关注 ps1 的 UTF-8 BOM（缺失会让 PowerShell 解析失败 → 恒拦截）；
    // 类 Unix 平台上该文件仅为跨平台占位，不参与本机执行。
    if is_windows && ps1.exists() && !has_utf8_bom(&ps1) {
        eprintln!(
            "⚠️ 拦截脚本 {} 缺少 UTF-8 BOM，Windows PowerShell 会解析失败（表现为恒拦截）。\
             请重新执行 req-guard install 修复。",
            ps1.display()
        );
    }

    if !script.exists() {
        return Err(GateError::Validation(format!(
            "未安装 AI 需求门禁（缺少 {}），请先执行 req-guard install",
            if is_windows {
                HOOK_PS1_REL
            } else {
                HOOK_SH_REL
            }
        )));
    }

    let args: Vec<String> = if is_windows {
        vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            script.to_string_lossy().to_string(),
        ]
    } else {
        vec![script.to_string_lossy().to_string()]
    };

    let out = Command::new(prog)
        .args(&args)
        .current_dir(root)
        .output()
        .map_err(|e| GateError::External {
            command: format!("{} {}", prog, args.join(" ")),
            status: None,
            stderr: e.to_string(),
        })?;

    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

/// 执行门禁检查，返回**结构化裁决**（CLI / TUI / GUI / CI 共用）。
///
/// 放行时同样保留脚本明细：绕过窗口内脚本会输出 [`BYPASS_MARKER`]，据此置
/// [`GateVerdict::bypassed`] 并把 summary 改成显式警告——绝不能让"靠绕过放行"
/// 显示成"三段已批准"。
pub fn gate_check(root: &Path) -> Result<GateVerdict> {
    let (ok, stdout, stderr) = run_hook(root)?;
    let raw = format!("{}\n{}", stdout, stderr);
    let bypassed = ok && raw.contains(BYPASS_MARKER);
    let detail: Vec<String> = raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        // 标记行是给程序看的，混进界面明细只是噪音。
        .filter(|l| !l.contains(BYPASS_MARKER))
        .collect();

    if ok {
        let summary = if bypassed {
            "⚠️ 门禁放行：命中应急绕过窗口（三段并非全部批准，已记审计日志）".to_string()
        } else {
            "✅ 门禁放行：三段已批准且无未解决的阻塞性评论".to_string()
        };
        Ok(GateVerdict::Pass {
            summary,
            detail,
            bypassed,
        })
    } else {
        Ok(GateVerdict::Block {
            summary: detail
                .first()
                .cloned()
                .unwrap_or_else(|| "⛔ 门禁拦截".to_string()),
            detail,
        })
    }
}

/// `req-guard hook-check` 判定"AI 正在写清单正文"时输出的标记。
///
/// 与 [`BYPASS_MARKER`] 同构：脚本据此直接放行本次写操作，不再走"三段是否已批准"
/// 的判定——否则 AI 连清单正文都写不了（文档流程第 2 步被门禁自己拦死）。
pub const ALLOW_DOC_MARKER: &str = "REQ_GUARD_ALLOW_DOC=1";

/// PreToolUse 阶段对**单次 AI 写操作**的裁决。
#[derive(Debug)]
pub enum PretoolVerdict {
    /// 本次写与证据/状态无关，交给后续门禁判定（脚本第 1 段起）。
    Continue,
    /// 本次写被禁止（证据保护 / 防自批），携带给人看的原因。
    Block(String),
    /// 放行：AI 正在填写清单正文，不必等三段批准。
    ///
    /// 只针对 `.gates/requirements/*.md`（评论文件除外）——这是文档流程里明确
    /// 要求 AI 完成的一步；源码仍必须过门禁。
    AllowDoc,
}

/// AI 写操作（PreToolUse）的裁决入口，由拦截脚本第 0 段调用。
///
/// payload 用 [`crate::json`] 真解析，而不是脚本里的 `sed` 正则：AI 工具可以对
/// 路径/内容做 Unicode 转义，正则匹配不到就静默放过——失效方向是最坏的
/// "看着在拦、其实没拦"。取不到路径时返回 [`PretoolVerdict::Continue`]：
/// 那不是一次可识别的文件写操作，硬拦只会误伤，真正的门禁判定仍在后面接管。
pub fn pretool_verdict(root: &Path, payload: &str) -> PretoolVerdict {
    let Some(fp) = crate::json::file_path_of(payload) else {
        return PretoolVerdict::Continue;
    };

    // 1) 证据保护：评论文件 AI 一律不得直接改（嫌疑人不得修改证据）
    if fp.ends_with(".comments.md") {
        audit(root, &format!("BLOCK-AI-WRITE-COMMENTS {}", fp));
        return PretoolVerdict::Block(format!(
            "审核评论文件禁止 AI 直接修改（嫌疑人不得修改证据）：{}\n\
             AI 回复请用：req-guard comment <需求ID> --author ai --reply C001 --text \"...\"",
            fp
        ));
    }

    // 1.5) 归档区彻底禁写：已归档是生命周期终点，任何修改都破坏历史证据链
    //（必须在 is_requirement_doc 之前——否则 archive/ 路径会当"清单正文"放行）。
    if fp
        .replace('\\', "/")
        .contains(".gates/requirements/archive/")
    {
        audit(root, &format!("BLOCK-AI-WRITE-ARCHIVE {}", fp));
        return PretoolVerdict::Block(format!(
            "归档需求禁止 AI 修改（历史证据不可变）：{}\n\
             归档区是只读历史，请以新需求清单承接后续改动",
            fp
        ));
    }

    // 2) 清单正文：允许 AI 写，但**状态行一个字都不许变**（防自批）
    if is_requirement_doc(&fp) {
        return match doc_write_guard(root, &fp, payload) {
            Ok(()) => PretoolVerdict::AllowDoc,
            Err(reason) => {
                audit(root, &format!("BLOCK-AI-TAMPER-STATUS {}", fp));
                PretoolVerdict::Block(reason)
            }
        };
    }

    PretoolVerdict::Continue
}

/// 是否为"需求清单正文"文件（`.gates/requirements/*.md`，评论文件已在上一步排除）。
fn is_requirement_doc(fp: &str) -> bool {
    let p = fp.replace('\\', "/");
    p.contains(".gates/requirements/") && p.ends_with(".md")
}

/// 清单正文写入的**防自批**闸门：可写返回 `Ok(())`，篡改返回拦截原因。
///
/// 核心不变式：**磁盘上已有的 GATE 状态行，一个字都不许变**。状态行是 `approve`
/// 命令的职权；AI 一旦能改 `status=approved` 就等于能自批，而审批锁
/// （[`crate::auth::ensure_human`]）只管命令层，管不到文件层写入。
fn doc_write_guard(root: &Path, fp: &str, payload: &str) -> std::result::Result<(), String> {
    // 片段编辑（Edit/MultiEdit）只提交 old/new 片段，无法与磁盘基线逐行比对：
    // 把 old_string 写成 "status=pending" 就能骗过"内容里有没有 GATE 标记"的粗判。
    // 故一律要求 Write 整篇覆盖。
    if crate::json::old_string_of(payload).is_some() {
        return Err(format!(
            "清单文件禁止片段编辑（无法校验状态行是否被改动）：{}\n\
             请改用 Write 整篇覆盖，且保持 GATE:HEAD / GATE:STEP 行原样不变",
            fp
        ));
    }

    let new_text = crate::json::content_of(payload)
        .or_else(|| crate::json::new_string_of(payload))
        .ok_or_else(|| {
            format!(
                "无法取得待写入内容，出于安全不予放行：{}\n请改用 Write 整篇覆盖",
                fp
            )
        })?;

    let disk = fs::read_to_string(root.join(fp)).unwrap_or_default();
    if disk.trim().is_empty() {
        // 新建文件：不得自带审批状态行——那是 approve 的职权
        if !gate_lines(&new_text).is_empty() {
            return Err(format!(
                "新建清单文件时不得自带 GATE:HEAD / GATE:STEP 状态行\
                 （审批状态由 approve 命令写入）：{}",
                fp
            ));
        }
        return Ok(());
    }

    let before = gate_lines(&disk);
    let after = gate_lines(&new_text);
    if before != after {
        return Err(format!(
            "清单正文可以写，但审批状态行不得改动（AI 自批等同于绕过审核）：{}\n\
             磁盘状态行：{:?}\n本次写入：{:?}",
            fp, before, after
        ));
    }
    Ok(())
}

/// 取出文本里的 GATE 状态行（已 trim），用于比对是否被篡改。
fn gate_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.contains("GATE:HEAD") || l.contains("GATE:STEP"))
        .map(|l| l.trim().to_string())
        .collect()
}

/// 读取审计日志尾部 `n` 行（供 UI 展示）。
///
/// 日志可能由 PowerShell 首次写入而带 UTF-8 BOM，这里统一清洗掉。
pub fn audit_tail(root: &Path, n: usize) -> Result<Vec<String>> {
    let log = root.join(".gates/audit/gate-audit.log");
    if !log.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&log).map_err(|e| GateError::Io {
        path: Some(log.clone()),
        source: e,
    })?;
    let content = content.trim_start_matches('\u{feff}');
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let start = lines.len().saturating_sub(n);
    Ok(lines[start..].to_vec())
}

/// 生成有时效的应急绕过令牌（写入 `.gates/.bypass`，并记审计）。
pub fn bypass(root: &Path, reason: &str, actor: &str, ttl_minutes: u64) -> Result<PathBuf> {
    // 审批锁（§4.4 方案 A）：绕过同样是审批类动作，AI 会话内禁止自助开启。
    crate::auth::ensure_human("bypass")?;
    if reason.trim().is_empty() {
        return Err(GateError::Validation(
            "应急绕过必须填写原因（--reason），否则无法追溯".into(),
        ));
    }
    let now = now_epoch();
    let expires = now + ttl_minutes.saturating_mul(60);
    let content = format!(
        "reason={}\nactor={}\ncreated_epoch={}\nexpires_epoch={}\nttl_minutes={}\n",
        one_line(reason),
        one_line(actor),
        now,
        expires,
        ttl_minutes
    );
    let audit_dir = root.join(".gates/audit");
    fs::create_dir_all(&audit_dir).map_err(|e| GateError::Io {
        path: Some(audit_dir.clone()),
        source: e,
    })?;
    let bypass_event = format!(
        "BYPASS-OPEN actor={} ttl={}min reason={} channel={}",
        one_line(actor),
        ttl_minutes,
        one_line(reason),
        crate::auth::declared_channel()
    );
    audit(root, &bypass_event);
    // 关键事件入**入库台账**：绕过必须 PR 可见（§4.6）
    audit_ledger(root, &bypass_event);
    write_file(root, ".gates/.bypass", &content)
}

// ===================== 注入实现 =====================

fn inject_tool(
    root: &Path,
    tool: &str,
    created: &mut Vec<PathBuf>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let prof = TOOL_PROFILES
        .iter()
        .find(|p| p.name == tool)
        .ok_or_else(|| GateError::Validation(format!("未知 AI 工具: {}", tool)))?;
    let path = root.join(prof.config);
    let marker = marker_of(prof);

    if path.exists() {
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.contains(marker) {
            // 幂等：hook 已在。但 env 注入型工具若缺审批锁标记，仍需提示补齐
            // （旧版 install 写入的配置没有 env 段）。
            if prof.env_capable && !existing.contains(crate::auth::AI_CTX_ENV) {
                notes.push(format!(
                    "{} 已含门禁 hook，但缺审批锁 env 段（\"{}\": \"1\"），\
                     建议手工合并以防 AI 自批",
                    prof.config,
                    crate::auth::AI_CTX_ENV
                ));
            }
            return Ok(());
        }
        notes.push(format!(
            "{} 已存在且不含 req-guard 门禁配置，为避免破坏既有配置未覆盖，请手工合并以下片段：\n{}",
            prof.config,
            hook_json(prof)
        ));
        return Ok(());
    }
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| GateError::Io {
            path: Some(p.to_path_buf()),
            source: e,
        })?;
    }
    fs::write(&path, hook_json(prof)).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    created.push(path);
    Ok(())
}

/// 生成 AI 工具 hook 配置：按各工具**原生 schema** 渲染。
///
/// - 事件名 / matcher 取自 [`ToolProfile`]（如 Cursor 用小驼峰 `preToolUse`、matcher 匹配工具名）；
/// - `env_capable=true` 时注入会话级 `env` 段，把 [`crate::auth::AI_CTX_ENV`] 打进 AI 会话，
///   `approve/reject/resolve/bypass` 检测到即自拒（审批锁，方案 A）；
/// - 拦截命令：Claude/CodeBuddy 直连 `req-guard-check`（原生 exit 0/1）；
///   Codex/Cursor 改连 `req-guard-deny` 包装（把 exit 1 转成其能识别的 exit 2）。
fn hook_json(prof: &ToolProfile) -> String {
    let cmd = if cfg!(windows) {
        format!(
            "powershell -NoProfile -ExecutionPolicy Bypass -File {}",
            hook_script_rel(prof.use_deny)
        )
    } else {
        format!("sh {}", hook_script_rel(prof.use_deny))
    };
    let env = if prof.env_capable {
        format!(
            "  \"env\": {{\n    \"{}\": \"1\"\n  }},\n",
            crate::auth::AI_CTX_ENV
        )
    } else {
        String::new()
    };
    format!(
        "{{\n{}  \"hooks\": {{\n    \"{}\": [\n      {{\n        \
         \"matcher\": \"{}\",\n        \
         \"hooks\": [\n          {{\n            \"type\": \"command\",\n            \
         \"command\": \"{}\"\n          }}\n        ]\n      }}\n    ]\n  }}\n}}\n",
        env, prof.event, prof.matcher, cmd
    )
}

/// git pre-commit 追加块：**fail-closed**（与 gates-toolkit 片段 `030-reqguard` 语义一致）。
///
/// 门禁是安全机制：拦截脚本缺失必须**拦截提交并提示初始化**，不得静默放行——
/// "看起来在拦，实际没拦"是本工具最危险的失败模式（见《AI工具合规保证规范.md》§4.2）。
const PRE_COMMIT_BLOCK: &str = r#"
# ===== req-guard（AI 需求门禁；追加在 gates-toolkit 之后） =====
# ⚠️ fail-closed：脚本缺失即拦截，不得静默放行（与片段 030-reqguard 语义一致）
if [ ! -f .gates/hooks/req-guard-check.sh ]; then
  echo "✗ req-guard 门禁脚本缺失（.gates/hooks/req-guard-check.sh），提交已被阻止。" >&2
  echo "  请先执行 req-guard install 初始化门禁；确需跳过本次：git commit --no-verify" >&2
  exit 1
fi
sh .gates/hooks/req-guard-check.sh || exit 1
"#;

/// `.gitignore` 需要忽略的门禁本机运行态文件。
pub const GITIGNORE_LINES: [&str; 2] = [".gates/.bypass", ".gates/audit/*.log"];

/// 读取 `.gates/req-guard.yaml` 的 `enforce.ci` 开关（缺省 **true**，fail-closed）。
///
/// 与 [`strict_order`] 同为零依赖逐行解析：只在 `enforce:` 区块内匹配 `ci:`，
/// 避免误读文件其他位置的同名键。
fn enforce_ci(root: &Path) -> bool {
    let path = root.join(".gates/req-guard.yaml");
    let Ok(content) = fs::read_to_string(&path) else {
        return true;
    };
    let mut in_enforce = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if t == "enforce:" {
            in_enforce = true;
            continue;
        }
        if in_enforce {
            if line.starts_with(' ') {
                if let Some(v) = t.strip_prefix("ci:") {
                    // 值可能带行内注释（模板即 `ci: true   # ...`），先按 `#` 截断再判定。
                    let v = v.split('#').next().unwrap_or("").trim();
                    return !v.eq_ignore_ascii_case("false");
                }
            } else if !t.is_empty() {
                in_enforce = false; // 缩进结束，离开 enforce 区块
            }
        }
    }
    true
}

/// L3 CI 接入体检：`enforce.ci` 默认 true，仓库就必须有 CI 编排**实际调用** `req-guard`，
/// 否则本机的任何绕过（含 `git commit --no-verify`）在服务端无人抵消，"三层"实际只剩两层。
///
/// 覆盖常见编排位置：GitHub Actions（`.github/workflows/*.yml`）、GitLab（`.gitlab-ci.yml`）、
/// CircleCI（`.circleci/config.yml`）、CNB（`.cnb.yml`）。判定宽松：编排内容含 `req-guard`
/// 即视为已接入（门禁 job 的注释/命令均含该字样，宽松匹配避免把已接入误判为缺口）。
/// 显式 `enforce.ci: false` 时不体检（声明放弃 L3）。
fn verify_ci(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    if !enforce_ci(root) {
        return problems;
    }
    let mut orchs: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = fs::read_dir(root.join(".github/workflows")) {
        for e in rd.flatten() {
            let p = e.path();
            let is_yaml = matches!(p.extension().and_then(|x| x.to_str()), Some("yml" | "yaml"));
            if is_yaml {
                orchs.push(p);
            }
        }
    }
    for rel in [".gitlab-ci.yml", ".circleci/config.yml", ".cnb.yml"] {
        let p = root.join(rel);
        if p.exists() {
            orchs.push(p);
        }
    }
    if orchs.is_empty() {
        problems.push(
            "未检测到任何 CI 编排文件（.github/workflows/、.gitlab-ci.yml、.circleci/、.cnb.yml）\
             ——L3 未落地：enforce.ci 默认 true，AI 在本机 --no-verify 将无人兜底；\
             请把 .gates/ci/req-guard-ci.yml 复制进 CI 目录并设为必需状态检查，\
             或显式设 enforce.ci: false 声明放弃"
                .into(),
        );
        return problems;
    }
    if !orchs.iter().any(|p| {
        fs::read_to_string(p)
            .map(|c| c.contains("req-guard"))
            .unwrap_or(false)
    }) {
        let names: Vec<String> = orchs.iter().map(|p| p.display().to_string()).collect();
        problems.push(format!(
            "存在 CI 编排（{}）但未调用 req-guard——L3 缺口：本机绕过（含 --no-verify）\
             服务端无人抵消；把 .gates/ci/req-guard-ci.yml 复制进编排并设为必需状态检查",
            names.join(", ")
        ));
    }
    problems
}

/// `req-guard install --verify`：校验门禁是否真正就位（§4.5，供 CI 使用）。
///
/// 返回**问题清单**：空 = 全部通过；非空 = 存在"静默缺口"，CI 应据此红。
/// 校验规则（配置文件存在即视为该工具"在用"）：
/// - 核心资产（拦截脚本 sh/ps1、门禁声明）缺失 → 问题；
/// - 在用工具的配置不含 `req-guard-check` → **L1 静默缺口**（只装不生效）；
/// - env 注入型工具（如 claude）缺 [`crate::auth::AI_CTX_ENV`] → 审批锁缺口；
/// - 有 `.git` 但 pre-commit 未含拦截 → **L2 缺口**；
/// - 有 `.git` 且 pre-commit 已接入但**缺执行位** → **L2 静默失效**（类 Unix 上
///   git 会跳过不可执行钩子，内容再对也不会拦）；
/// - `enforce.ci` 默认 true 但无 CI 编排调用 `req-guard` → **L3 缺口**（§4.3）。
///
/// 未安装（配置不存在）的工具不要求——不制造噪音；未知新工具出现时，
/// 由团队把它登记进 `TOOL_PROFILES` 后纳入校验白名单。
pub fn verify_install(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();

    for rel in [
        HOOK_SH_REL,
        HOOK_PS1_REL,
        DENY_SH_REL,
        DENY_PS1_REL,
        ".gates/req-guard.yaml",
    ] {
        if !root.join(rel).exists() {
            problems.push(format!("缺少门禁资产 {}（执行 req-guard install）", rel));
        }
    }

    for prof in TOOL_PROFILES.iter() {
        let p = root.join(prof.config);
        if !p.exists() {
            continue; // 未在用的工具不要求
        }
        let content = fs::read_to_string(&p).unwrap_or_default();
        let marker = marker_of(prof);
        if !content.contains(marker) {
            problems.push(format!(
                "{} 已存在（在用）但未接入 req-guard hook（缺 `{}` 的 .sh / .ps1 任一形式）\
                 ——L1 静默缺口；按 req-guard install 输出的片段手工合并",
                prof.config, marker
            ));
        }
        if prof.env_capable && !content.contains(crate::auth::AI_CTX_ENV) {
            problems.push(format!(
                "{} 缺少 {} 会话标记——审批锁缺口（防 AI 自批）；手工合并 env 段后重验",
                prof.config,
                crate::auth::AI_CTX_ENV
            ));
        }
    }

    if root.join(".git").exists() {
        let hook_path = root.join(".git/hooks/pre-commit");
        match fs::read_to_string(&hook_path) {
            Ok(c) if c.contains("req-guard-check") => {
                // 内容对了不代表生效：Unix 上 git 会静默跳过没有执行位的钩子，
                // 这是"verify 全绿但 L2 完全没拦"的唯一成因，必须单独查。
                if !is_executable(&hook_path) {
                    problems.push(
                        ".git/hooks/pre-commit 已接入但缺少执行位——git 会静默跳过它（L2 实际未生效）；\
                         执行 chmod +x .git/hooks/pre-commit 或重跑 req-guard install"
                            .into(),
                    );
                }
            }
            Ok(_) => problems.push(
                ".git/hooks/pre-commit 未含 req-guard 拦截——L2 缺口；重跑 req-guard install".into(),
            ),
            Err(_) => {
                problems.push("缺少 .git/hooks/pre-commit——L2 缺口；重跑 req-guard install".into())
            }
        }
    }

    // L3 CI 接入体检（§4.3）：enforce.ci 默认 true——必须真有 CI 编排调用 req-guard，
    // 否则样例没复制出去时"默认三层"实际只有两层（本机任何绕过服务端无人抵消）。
    problems.extend(verify_ci(root));

    problems
}

/// 追加 git pre-commit（幂等；不覆盖 gates-toolkit 已写入的内容）。
fn append_pre_commit(
    root: &Path,
    created: &mut Vec<PathBuf>,
    notes: &mut Vec<String>,
) -> Result<()> {
    if !root.join(".git").exists() {
        notes.push(
            "未检测到 .git，已跳过 pre-commit 注入（git init 后可执行 req-guard install）".into(),
        );
        return Ok(());
    }
    let hook = root.join(".git/hooks/pre-commit");
    if hook.exists() {
        let c = fs::read_to_string(&hook).map_err(|e| GateError::Io {
            path: Some(hook.clone()),
            source: e,
        })?;
        if c.contains("req-guard-check") {
            // 已接入：仍要补执行位——旧版 install 落盘时未设置，git 会静默跳过。
            // 少了这一步，老仓库重跑 install 也永远修不好 L2。
            ensure_executable(&hook);
            return Ok(());
        }
        let mut nc = c;
        if !nc.ends_with('\n') {
            nc.push('\n');
        }
        nc.push_str(PRE_COMMIT_BLOCK);
        fs::write(&hook, nc).map_err(|e| GateError::Io {
            path: Some(hook.clone()),
            source: e,
        })?;
    } else {
        let mut nc = String::from("#!/bin/sh\n");
        nc.push_str(PRE_COMMIT_BLOCK);
        fs::write(&hook, nc).map_err(|e| GateError::Io {
            path: Some(hook.clone()),
            source: e,
        })?;
    }
    // git 在 Unix 上只执行带执行位的钩子，缺了就是"静默不拦"。
    ensure_executable(&hook);
    created.push(hook);
    Ok(())
}

/// 追加 `.gitignore` 条目（幂等；只追加，不覆盖用户既有内容）。
///
/// 绕行令牌 `.gates/.bypass` 与审计日志是**本机运行态**，入库会造成误导
/// （别人 clone 后拿到一个已过期的绕行窗口），必须在生成时就把它们忽略掉。
fn append_gitignore(
    root: &Path,
    created: &mut Vec<PathBuf>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let path = root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = GITIGNORE_LINES
        .iter()
        .copied()
        .filter(|l| !existing.lines().any(|e| e.trim() == *l))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut nc = existing;
    if !nc.is_empty() && !nc.ends_with('\n') {
        nc.push('\n');
    }
    if !nc.is_empty() {
        nc.push('\n');
    }
    nc.push_str("# req-guard 门禁本机运行态（不入库）\n");
    for l in &missing {
        nc.push_str(l);
        nc.push('\n');
    }
    fs::write(&path, nc).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    created.push(path.clone());
    notes.push(format!("已追加 .gitignore 忽略项：{}", missing.join("、")));
    Ok(())
}

// ===================== 时间工具 =====================

/// 当前时间字符串（best-effort：`date` → PowerShell → `-`）。
pub fn now_str() -> String {
    if let Ok(o) = Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output() {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Ok(o) = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Date -Format \"yyyy-MM-dd HH:mm:ss\"",
        ])
        .output()
    {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "-".to_string()
}

/// 当前 Unix 秒（用于绕过令牌过期判定）。
pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 压成单行（避免换行破坏 key=value 行解析）。
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ===================== 模板常量 =====================

/// 门禁声明（借鉴 teamai hooks.yaml：声明式、可评审、随仓库版本化）。
pub const REQ_GUARD_YAML: &str = r#"# req-guard — AI 需求门禁声明
# 目标：AI 在编写代码前，必须先完成三段清单并经审核人批准；否则硬拦截。
# 本文件随仓库版本化，改动须走评审（与团队规范同源）。
version: 1
enabled: true

# 三段清单（顺序即默认强制审核顺序）
steps:
  - name: decomposition
    label: 需求分解
  - name: solution
    label: 技术方案
  - name: testplan
    label: 测试计划

# 强制顺序：审核后一步之前，其前置步骤必须已 approved
strict_order: true

# 拦截点
enforce:
  ai_tool_hook: true   # AI 工具 PreToolUse，拦截 Write/Edit —— 最硬的一层
  pre_commit: true     # git pre-commit 兜底
  ci: true             # CI 侧拦截：流水线须调用 req-guard check，并设为必需（required）状态检查

# 被拦截的 AI 写操作（matcher 语法随工具而异）
blocked_tools:
  - Write
  - Edit
  - MultiEdit
  - NotebookEdit

# 应急绕过（有时效、必填原因、强制审计）
bypass:
  enabled: true
  default_ttl_minutes: 60
  require_reason: true

# 到期自动归档：done 满 N 天后，req-guard done 成功时自动把清单
# 物理搬入 .gates/requirements/archive/<创建年份>/（无日期段 → misc/）
# 0 = done 即刻归档；手动补扫：req-guard archive
archive:
  after_days: 30
"#;

/// `.gates/README.md`：门禁使用说明（随项目生成，便于新成员自助）。
pub const REQ_GUARD_README: &str = r#"# AI 需求门禁（req-guard）

> **规则**：AI 在本项目实现新需求前，必须先按步骤完成
> **需求分解 → 技术方案 → 测试计划** 三段清单，且每段由审核人显式批准，
> 否则**硬拦截**：AI 工具无法写入/修改任何文件。

## 工作原理

| 层 | 拦截点 | 说明 |
| --- | --- | --- |
| L1 | AI 工具 `PreToolUse`（Write/Edit） | 最硬：AI 根本写不了文件 |
| L2 | `git pre-commit` | 兜底：未过审不得提交 |
| L3 | CI 调用 `req-guard check` | 兜底：未过审不得合并 |

拦截脚本：`.gates/hooks/req-guard-check.sh`（POSIX）与 `.ps1`（Windows）。
它只解析清单里的 `GATE` 标记行——**正文随便改，GATE 行只能由 `req-guard` 命令改写**。

## 标准流程

```bash
# 1. 创建需求清单（AI 或人执行）
req-guard create -t "用户登录改造"

# 2. AI 填写三段正文（编辑生成的 .gates/requirements/REQ-001-*.md）

# 3. 审核人逐段批准（注意顺序：分解 → 方案 → 测试计划）
req-guard approve REQ-001 --step decomposition --reviewer 张三
req-guard approve REQ-001 --step solution      --reviewer 张三
req-guard approve REQ-001 --step testplan      --reviewer 张三

# 4. 查看状态；三步全 approved 才"已解锁"
req-guard status REQ-001

# 5. 解锁后 AI 才可编写代码

# 6. 需求完成（或中止）后归档——门禁随之跳过该清单
#    done 满 archive.after_days 天（默认 30）后，下次 done 时自动物理归档；
#    手动补扫：req-guard archive --author <姓名>（--dry-run 先预览）
req-guard done REQ-001 --author 张三

# （可选）查看归档历史
req-guard status --archived
```

打回：`req-guard reject REQ-001 --step solution --reviewer 张三 --reason "缺少回滚方案"`

## 到期归档（done 满 N 天后自动搬入 archive/）

需求文档会越积越多，全部平铺在 `.gates/requirements/` 会让 `status`/`list` 越来越长。
`req-guard done` 成功后，系统自动扫描 done 满 `archive.after_days` 天（默认 30，
可在 `.gates/req-guard.yaml` 的 `archive.after_days` 调整）的清单，**成对搬移**
（清单 + 评论）到 `.gates/requirements/archive/<创建年份>/`（无日期段 → `misc/`）：

- 归档区是**只读历史**：拦截脚本、`list`、自动编号都不再看它，但 `status`/`comments`
  仍可按 id 查到（`req-guard status REQ-001`、`req-guard status --archived`）；
- 归档**不复号**：`create` 自动编号会跳过归档区已用过的 `REQ-NNN`；
- AI 不得修改归档区文件（原样保留历史证据）。

## 审批锁：approve / resolve / done / bypass 须人类执行

`--reviewer` / `--author` 只是名字，不构成身份保证。因此 req-guard 给支持会话环境
注入的 AI 工具（如 Claude Code）写入 `"REQ_GUARD_AI_CTX": "1"`，`approve / reject /
resolve / done / bypass` 检测到该标记即**拒绝执行**——AI 经 Shell 自批会被堵在命令层。

- 审核人请在**自己的终端**（AI 会话之外）执行审批命令；
- 人类误中拦截时：在不带该变量的终端重试，或先 `unset REQ_GUARD_AI_CTX`；
- 该标记可被 `env -u` 剥离，属提高门槛而非强保证；生产环境请升级
  reviewer token / 带外审批（见《AI工具合规保证规范.md》§4.4）。

## 应急绕过（有痕、有时效）

```bash
req-guard bypass --reason "线上故障热修，事后补审" --ttl 60
```

绕过窗口内放行，但**每次都写审计日志** `.gates/audit/gate-audit.log`。
`.gates/.bypass` 已被 `.gitignore` 忽略，不会入库。

**人肉开发场景**：typo 修正、文档笔误、线上热修、临时试改等不值得建 REQ 的改动，
可直接 `bypass --reason <原因> --ttl <分钟>`（默认 60 分钟，到期自动失效）。
但 bypass 是**应急阀不是常规通道**——功能与架构改动必须先建 REQ 走三段审核；
事后请补建需求并 `req-guard done <REQ-ID> --author <姓名>` 归档，把账还上。

## L3 CI 强制门禁（部署规范）

`.gates/req-guard.yaml` 默认 `enforce.ci: true`——CI 必须**独立重跑**门禁，
本机任何绕过（含 `git commit --no-verify`）都会在服务端被抵消：

1. 流水线中执行 `req-guard check`（退出码非 0 即失败）；CI 镜像内置 req-guard
   二进制，版本与 Release tag 一致（`req-guard -V` 可核对）；
2. 将该检查设为**必需（required）状态检查**：不通过禁止合并；
3. 分支保护：禁止直推 `main` 等受保护分支；
4. 并行开发团队建议追加一步 `req-guard ids --check`：合并后扫描需求编号冲突，
   同 id 多文件 / 前缀歧义即红（规范详见 `docs/规范/需求编号防冲突命名规范.md`）；
5. 建议追加一步 `req-guard install --verify`：任一在用 AI 工具缺 hook 即红，
   消除 L1 静默缺口。

**开始接入**：`req-guard install` 已在本项目生成可直接部署的样例
`.gates/ci/req-guard-ci.yml`（GitHub Actions）——把它复制到 `.github/workflows/`
并设为必需状态检查即可；该样例同时跑 `req-guard check`、`ids --check`
与 `install --verify`。

## 审计台账与摘要（随仓库提交）

- `.gates/audit/ledger.md`：approve / reject / resolve / bypass / 阻塞性评论等
  关键事件的**入库台账**——PR diff 可直接复核"谁在何时批了什么"；
- `.gates/audit/DIGEST`：`req-guard audit-digest` 生成本机 `gate-audit.log` 的
  SHA-256 摘要；PR 中与本地日志比对即可发现事后篡改；
- `gate-audit.log`（全量流水）与 `.bypass` 是本机运行态，已由 .gitignore 忽略。

## 与 gates-toolkit 的关系

- **gates-toolkit**：代码质量门禁（提交时查 SQL/Java 规范）——管"写得对不对"。
- **AI 需求门禁**：流程门禁（写代码前查审核）——管"该不该写"。
- 两者互补；req-guard 生成时把 AI 门禁**追加**在 gates-toolkit 的 pre-commit 之后，不覆盖。
"#;

/// POSIX 拦截脚本（AI 工具 hook 与 pre-commit 共用，唯一判定逻辑）。
pub const HOOK_SH: &str = r#"#!/usr/bin/env sh
# req-guard — AI 需求门禁硬拦截脚本（POSIX）
#
# 用法：
#   1) AI 工具 PreToolUse hook：拦截 Write/Edit 等写操作（AI 写不了文件）
#   2) git pre-commit：兜底，未过审不得提交
# 退出码：0 放行；1 拦截（stderr 会被 AI 工具回显，从而阻止其继续写代码）
set -u

REQ_DIR=".gates/requirements"
AUDIT_LOG=".gates/audit/gate-audit.log"
BYPASS_FILE=".gates/.bypass"

log() {
  mkdir -p "$(dirname "$AUDIT_LOG")" 2>/dev/null || true
  printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo '-')" "$1" >>"$AUDIT_LOG" 2>/dev/null || true
}

# ---------- 0) AI 禁止直接修改评论文件（★ 必须先于应急绕过：逃逸阀不覆盖证据完整性） ----------
if [ ! -t 0 ]; then
  STDIN_DATA=$(cat 2>/dev/null || true)
  if [ -n "$STDIN_DATA" ]; then
    # 优先交给 req-guard 用 Rust **真解析** JSON：AI 工具 payload 允许 Unicode 转义
    # （".gates\u002f…comments.md" 与 ".gates/…comments.md" 完全等价），正则匹配不到
    # 会静默放过——那正是本工具最坏的失效：看着在拦，其实没拦。
    if command -v req-guard >/dev/null 2>&1; then
      # 拦截时 req-guard 已把原因打到 stderr（AI 看得见），这里只接退出码
      OUT=$(printf '%s' "$STDIN_DATA" | req-guard hook-check) || exit 1
      case "$OUT" in
        # 清单正文（状态行未改动）：放行本次写，不再要求三段已批准
        *REQ_GUARD_ALLOW_DOC=1*) exit 0 ;;
      esac
    else
      # 兜底：二进制不在 PATH（受限环境）时退回正则粗判。
      # 只保留证据保护，**不**放行清单正文（无法校验状态行 → 宁可维持 fail-closed）
      FP=$(printf '%s' "$STDIN_DATA" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
      case "$FP" in
        *.comments.md)
          log "BLOCK-AI-WRITE-COMMENTS $FP"
          echo "[req-guard] ⛔ 拦截：审核评论文件禁止 AI 直接修改（嫌疑人不得修改证据）。" >&2
          echo "            AI 回复请用：req-guard comment <需求ID> --author ai --reply C001 --text \"...\"" >&2
          exit 1
          ;;
      esac
    fi
  fi
fi

# ---------- 1) 应急绕过窗口（有痕、有时效） ----------
if [ -f "$BYPASS_FILE" ]; then
  EXP=$(sed -n 's/.*expires_epoch=\([0-9]*\).*/\1/p' "$BYPASS_FILE" 2>/dev/null | head -1)
  case "$EXP" in ''|*[!0-9]*) EXP="";; esac
  NOW=$(date '+%s' 2>/dev/null || echo 0)
  case "$NOW" in ''|*[!0-9]*) NOW=0;; esac
  if [ -n "$EXP" ] && [ "$NOW" -lt "$EXP" ]; then
    log "BYPASS-HIT expires_epoch=$EXP"
    echo "[req-guard] 警告：命中应急绕过窗口，本次放行（已记审计日志）" >&2
    # 机器可读标记：供 req-guard check 判定"本次放行靠绕过"（勿改，与 BYPASS_MARKER 对应）
    echo "REQ_GUARD_BYPASS=1"
    exit 0
  fi
fi

# ---------- 2) 定位当前活跃需求（跳过已归档 done 的） ----------
ACTIVE=""
if [ -d "$REQ_DIR" ]; then
  for F in $(ls "$REQ_DIR" 2>/dev/null | grep '\.md$' | grep -v '\.comments\.md$' | sort -r); do
    ST=$(sed -n 's/.*GATE:HEAD .*status=\([a-z_]*\).*/\1/p' "$REQ_DIR/$F" 2>/dev/null | head -1)
    if [ "$ST" != "done" ]; then
      ACTIVE="$F"
      break
    fi
  done
fi

if [ -z "$ACTIVE" ]; then
  log "BLOCK no-requirement"
  echo "[req-guard] ⛔ 拦截：未找到待开发的需求清单。" >&2
  echo "          AI 在编写代码前，必须先创建并走完三段清单审核：" >&2
  echo "            req-guard create -t \"<需求标题>\"" >&2
  exit 1
fi

# ---------- 3) 三段步骤必须全部 approved ----------
FAILED=""
for STEP in decomposition solution testplan; do
  LINE=$(grep 'GATE:STEP' "$REQ_DIR/$ACTIVE" 2>/dev/null | grep "name=$STEP " | head -1)
  ST=$(printf '%s' "$LINE" | sed -n 's/.*status=\([a-z_]*\).*/\1/p')
  if [ "$ST" != "approved" ]; then
    FAILED="$FAILED\n    - $STEP 未通过审核（当前: ${ST:-pending}）"
  fi
done

if [ -n "$FAILED" ]; then
  log "BLOCK $ACTIVE"
  printf "[req-guard] ⛔ 拦截：需求 %s 尚未通过审核，AI 不得编写/修改源码。\n" "$ACTIVE" >&2
  printf "          未完成步骤：%b\n" "$FAILED" >&2
  echo "          请补齐清单后由审核人执行：" >&2
  echo "            req-guard approve <需求ID> --step <步骤> --reviewer <姓名>" >&2
  echo "          步骤顺序：decomposition(需求分解) → solution(技术方案) → testplan(测试计划)" >&2
  exit 1
fi

# ---------- 4) 阻塞性评论必须全部 resolved ----------
COMMENTS="$REQ_DIR/${ACTIVE%.md}.comments.md"
if [ -f "$COMMENTS" ] && grep -q 'blocking=true' "$COMMENTS" 2>/dev/null; then
  OPEN=$(grep 'GATE:COMMENT' "$COMMENTS" 2>/dev/null | grep 'blocking=true' | grep 'state=open')
  if [ -n "$OPEN" ]; then
    log "BLOCK-COMMENT $ACTIVE"
    echo "[req-guard] ⛔ 拦截：存在未解决的阻塞性评论，需审核人 resolve 后才可编码" >&2
    exit 1
  fi
fi

# ---------- 5) 评论摘要（每次都提示，保证 AI 必然看到） ----------
if [ -f "$COMMENTS" ]; then
  N=$(grep 'GATE:COMMENT' "$COMMENTS" 2>/dev/null | grep -c 'state=open')
  case "$N" in ''|*[!0-9]*) N=0;; esac
  if [ "$N" -gt 0 ]; then
    echo "[req-guard] 提示：有 ${N} 条 open 评论，执行 req-guard comments ${ACTIVE} 查看" >&2
  fi
fi

log "PASS $ACTIVE"
exit 0
"#;

/// Windows 拦截脚本（PowerShell，逻辑与 [`HOOK_SH`] 等价）。
pub const HOOK_PS1: &str = r#"# req-guard — AI 需求门禁硬拦截脚本（Windows PowerShell）
# 退出码：0 放行；1 拦截
$ErrorActionPreference = 'Continue'

$REQ_DIR = ".gates/requirements"
$AUDIT_LOG = ".gates/audit/gate-audit.log"
$BYPASS_FILE = ".gates/.bypass"

function Write-GateAudit([string]$msg) {
  $dir = Split-Path -Parent $AUDIT_LOG
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
  $line = "{0} {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $msg
  Add-Content -Path $AUDIT_LOG -Value $line -Encoding UTF8
}

# ---------- 0) AI 禁止直接修改评论文件（★ 必须先于应急绕过） ----------
if (-not [Console]::IsInputRedirected) {
  $stdinData = ''
} else {
  $stdinData = [Console]::In.ReadToEnd()
}
if ($stdinData.Trim()) {
  # 优先交给 req-guard 用 Rust **真解析**（正则会被 Unicode 转义绕过，详见 HOOK_SH 第 0 段）
  if (Get-Command req-guard -ErrorAction SilentlyContinue) {
    $out = $stdinData | req-guard hook-check
    if ($LASTEXITCODE -ne 0) { exit 1 }
    # 清单正文（状态行未改动）：放行本次写，不再要求三段已批准
    if ($out -match 'REQ_GUARD_ALLOW_DOC=1') { exit 0 }
  } else {
    # 兜底：只保留证据保护，**不**放行清单正文（无法校验状态行 → fail-closed）
    $m0 = [regex]::Match($stdinData, '"file_path"\s*:\s*"([^"]+)"')
    if ($m0.Success -and $m0.Groups[1].Value -like '*.comments.md') {
      Write-GateAudit "BLOCK-AI-WRITE-COMMENTS $($m0.Groups[1].Value)"
      Write-Error "[req-guard] 拦截：审核评论文件禁止 AI 直接修改（嫌疑人不得修改证据）。AI 回复请用：req-guard comment <需求ID> --author ai --reply C001 --text ""..."""
      exit 1
    }
  }
}

# ---------- 1) 应急绕过窗口 ----------
if (Test-Path $BYPASS_FILE) {
  $txt = [string](Get-Content $BYPASS_FILE -Raw -ErrorAction SilentlyContinue)
  $m = [regex]::Match($txt, 'expires_epoch=(\d+)')
  if ($m.Success) {
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    if ($now -lt [int64]$m.Groups[1].Value) {
      Write-GateAudit "BYPASS-HIT expires_epoch=$($m.Groups[1].Value)"
      Write-Output "[req-guard] 警告：命中应急绕过窗口，本次放行（已记审计日志）"
      # 机器可读标记：供 req-guard check 判定"本次放行靠绕过"（勿改，与 BYPASS_MARKER 对应）
      Write-Output "REQ_GUARD_BYPASS=1"
      exit 0
    }
  }
}

# ---------- 2) 定位当前活跃需求 ----------
$active = $null
if (Test-Path $REQ_DIR) {
  $files = Get-ChildItem -Path $REQ_DIR -Filter *.md -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notlike '*.comments.md' } |
    Sort-Object Name -Descending
  foreach ($f in $files) {
    $head = Get-Content $f.FullName -TotalCount 20 -ErrorAction SilentlyContinue | Where-Object { $_ -match 'GATE:HEAD' } | Select-Object -First 1
    $st = ''
    if ($head -match 'status=([a-z_]+)') { $st = $Matches[1] }
    if ($st -ne 'done') { $active = $f; break }
  }
}

if ($null -eq $active) {
  Write-GateAudit "BLOCK no-requirement"
  Write-Error "[req-guard] 拦截：未找到待开发的需求清单。请先执行 req-guard create -t ""<需求标题>"""
  exit 1
}

# ---------- 3) 三段步骤必须全部 approved ----------
$failed = @()
foreach ($step in @('decomposition', 'solution', 'testplan')) {
  $l = Get-Content $active.FullName -ErrorAction SilentlyContinue | Where-Object { $_ -match 'GATE:STEP' -and $_ -match "name=$step " } | Select-Object -First 1
  $st = ''
  if ($l -match 'status=([a-z_]+)') { $st = $Matches[1] }
  if ($st -ne 'approved') {
    if (-not $st) { $st = 'pending' }
    $failed += "$step 未通过审核（当前: $st）"
  }
}

if ($failed.Count -gt 0) {
  Write-GateAudit "BLOCK $($active.Name)"
  Write-Error "[req-guard] 拦截：需求 $($active.Name) 尚未通过审核，AI 不得编写/修改源码。未完成步骤： $($failed -join '; ')"
  exit 1
}

# ---------- 4) 阻塞性评论必须全部 resolved ----------
$commentsFile = Join-Path $REQ_DIR ($active.BaseName + ".comments.md")
if (Test-Path $commentsFile) {
  $blockingOpen = Get-Content $commentsFile -ErrorAction SilentlyContinue |
    Where-Object { $_ -match 'GATE:COMMENT' -and $_ -match 'blocking=true' -and $_ -match 'state=open' }
  if ($blockingOpen) {
    Write-GateAudit "BLOCK-COMMENT $($active.Name)"
    Write-Error "[req-guard] 拦截：存在未解决的阻塞性评论，需审核人 resolve 后才可编码"
    exit 1
  }
  $openN = @(Get-Content $commentsFile -ErrorAction SilentlyContinue |
    Where-Object { $_ -match 'GATE:COMMENT' -and $_ -match 'state=open' }).Count
  if ($openN -gt 0) {
    Write-Output "[req-guard] 提示：有 ${openN} 条 open 评论，执行 req-guard comments $($active.Name) 查看"
  }
}

Write-GateAudit "PASS $($active.Name)"
exit 0
"#;

/// deny 包装（POSIX）：把 [`HOOK_SH`] 的 `exit 1`（拦截）转成 Codex/Cursor 能识别的
/// `exit 2`。Claude/CodeBuddy 把非 `exit 0` 一律拦截；而 Codex/Cursor 则把非
/// `exit 0` 视为 fail-open（继续执行）、只认 `exit 2` 为拒绝——不包装会静默放行。
pub const DENY_SH: &str = r#"#!/usr/bin/env sh
# req-guard deny 包装：把 check.sh 的 exit 1（拦截）转成工具能识别的拒绝（exit 2）。
# 仅 Codex/Cursor 需要：它们把非 exit 0 视为 fail-open（继续执行）。
# 用法（作为 AI 工具 PreToolUse hook 的 command）：sh .gates/hooks/req-guard-deny.sh
set -u

sh .gates/hooks/req-guard-check.sh
RC=$?
if [ "$RC" -ne 0 ]; then
  echo "[req-guard] ⛔ 门禁拦截（原因见上方；以 exit 2 交付，编码为工具可识别的拒绝）" >&2
  exit 2
fi
exit 0
"#;

/// deny 包装（PowerShell，逻辑与 [`DENY_SH`] 等价，带 BOM 落盘）。
pub const DENY_PS1: &str = r#"# req-guard deny 包装（Windows PowerShell）
# 把 check.ps1 的 exit 1（拦截）转成 Codex/Cursor 能识别的拒绝（exit 2）
& (Join-Path $PSScriptRoot 'req-guard-check.ps1')
if ($LASTEXITCODE -ne 0) {
  Write-Error "[req-guard] 门禁拦截（原因见上方；以 exit 2 交付，编码为工具可识别的拒绝）"
  exit 2
}
exit 0
"#;

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cleanup, temp_dir};

    #[test]
    fn find_project_root_从深层子目录向上定位() {
        let root = temp_dir("find-root");
        fs::create_dir_all(root.join(".gates")).unwrap();
        // 模拟真实布局：项目根/gates-tools/req-guard-ui/bin
        let deep = root.join("gates-tools").join("req-guard-ui").join("bin");
        fs::create_dir_all(&deep).unwrap();

        // 绝对起点
        assert_eq!(
            find_project_root(&deep, 6).unwrap(),
            root,
            "从深层子目录应能向上找到项目根"
        );

        // 相对起点（"." —— 双击 exe 时 CWD 就是 exe 所在目录，传进来的正是 "."）
        // 这里必须真正切 CWD：相对路径的 parent() 是空路径，无法向上遍历。
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&deep).unwrap();
        let got = find_project_root(Path::new("."), 6);
        std::env::set_current_dir(&cwd).unwrap();
        let got = got.unwrap();
        // macOS：getcwd 返回解析 symlink 的物理路径（/private/var/…），
        // 而 env::temp_dir 给逻辑路径（/var/…）——两侧都 canonicalize 再比，
        // Windows 上则同为 \\?\ 前缀形式，均一致。
        assert_eq!(
            fs::canonicalize(&got).unwrap(),
            fs::canonicalize(&root).unwrap(),
            "相对起点 \".\" 应先解析为 CWD 再向上找（got={:?} root={:?}）",
            got,
            root
        );
        // ★ 展示给用户的路径不能带尾随 `.`：否则会出现 `C:\proj\.\.gates\…` 这种串味路径
        let shown = got.display().to_string();
        assert!(
            !shown.ends_with('.') && !shown.contains("\\.\\") && !shown.contains("/./"),
            "返回路径应已规范化，实际: {}",
            shown
        );

        // 上限内找不到 → None（调用方须保持 CWD，不能猜）
        let outside = temp_dir("find-root-outside");
        fs::create_dir_all(outside.join("a/b/c/d/e/f/g/h")).unwrap();
        assert!(find_project_root(&outside.join("a/b/c/d/e/f/g/h"), 2).is_none());

        cleanup(&root);
        cleanup(&outside);
    }

    #[test]
    fn ps1_落盘必须带_bom() {
        assert!(
            ps1_with_bom().starts_with('\u{feff}'),
            "PowerShell 5.1 需要 BOM，否则中文乱码导致脚本解析失败"
        );
    }

    #[test]
    fn has_utf8_bom_识别文件头() {
        let d = temp_dir("bom");
        let f = d.join("x.ps1");
        fs::write(&f, "x").unwrap();
        assert!(!has_utf8_bom(&f));
        fs::write(&f, "\u{feff}x").unwrap();
        assert!(has_utf8_bom(&f));
        assert!(!has_utf8_bom(&d.join("不存在.ps1")));
        cleanup(&d);
    }

    #[test]
    fn strict_order_读yaml且缺省fail_closed() {
        let root = temp_dir("strict");
        assert!(strict_order(&root), "缺文件时应 fail-closed = true");
        fs::create_dir_all(root.join(".gates")).unwrap();
        let y = root.join(".gates/req-guard.yaml");
        fs::write(&y, "version: 1\nstrict_order: false\n").unwrap();
        assert!(!strict_order(&root));
        fs::write(&y, "# strict_order: false\nstrict_order: true\n").unwrap();
        assert!(strict_order(&root), "注释行不得参与判定");
        fs::write(&y, "version: 1\n").unwrap();
        assert!(strict_order(&root), "未声明时按 true");
        cleanup(&root);
    }

    #[test]
    fn archive_after_days_读yaml且缺省30() {
        let root = temp_dir("arch-days");
        assert_eq!(archive_after_days(&root), 30, "缺文件时默认 30 天");
        fs::create_dir_all(root.join(".gates")).unwrap();
        let y = root.join(".gates/req-guard.yaml");
        fs::write(&y, "archive:\n  after_days: 7\n").unwrap();
        assert_eq!(archive_after_days(&root), 7);
        fs::write(&y, "archive:\n  after_days: 0  # done 即归档\n").unwrap();
        assert_eq!(archive_after_days(&root), 0, "行内注释应先截断再解析");
        fs::write(&y, "archive:\n  after_days: 0 # 注释\n  after_days: 9\n").unwrap();
        assert_eq!(archive_after_days(&root), 9, "后者覆盖前者");
        fs::write(&y, "archive: false\n").unwrap();
        assert_eq!(archive_after_days(&root), 30, "非键值不得命中");
        cleanup(&root);
    }

    #[test]
    fn 拦截脚本排除评论文件() {
        assert!(
            HOOK_SH.contains(r"grep -v '\.comments\.md$'"),
            "sh 脚本遍历需求目录时必须排除评论文件"
        );
        assert!(
            HOOK_PS1.contains("*.comments.md"),
            "ps1 脚本遍历需求目录时必须排除评论文件"
        );
    }

    #[test]
    fn 脚本资产命名与片段引用一致() {
        // install 生成路径 / pre-commit 块 / gates-toolkit 片段三者必须同名，
        // 否则片段会因 fail-closed 把正常提交全部拦下（历史 P0 缺陷）。
        assert!(
            PRE_COMMIT_BLOCK.contains(HOOK_SH_REL),
            "pre-commit 块必须引用 {}",
            HOOK_SH_REL
        );
        let frag = include_str!("../../templates/hooks/fragments/reqguard-check.sh");
        assert!(
            frag.contains(HOOK_SH_REL),
            "片段调用的脚本名必须与 req-guard install 生成的一致（{}）",
            HOOK_SH_REL
        );
        assert!(HOOK_PS1_REL.ends_with(".ps1"));
        for text in [HOOK_SH, HOOK_PS1, PRE_COMMIT_BLOCK, frag] {
            assert!(!text.contains("ai-gate"), "不得残留旧命名 ai-gate-*");
        }
    }

    #[test]
    fn pre_commit块缺失脚本时必须拦截() {
        // fail-closed（§4.2）：脚本缺失不得静默放行，必须 exit 1 并提示初始化。
        // 真机行为由 scripts/verify_gate.py 场景 11 实跑校验。
        assert!(
            PRE_COMMIT_BLOCK.contains("if [ ! -f .gates/hooks/req-guard-check.sh ]"),
            "必须先判缺失即拦截：\n{}",
            PRE_COMMIT_BLOCK
        );
        assert!(PRE_COMMIT_BLOCK.contains("exit 1"), "缺失分支必须 exit 1");
        // 历史缺陷回归：不得再回到"存在才执行"的静默跳过形态（fail-open）
        assert!(
            !PRE_COMMIT_BLOCK.contains("if [ -f .gates/hooks/req-guard-check.sh ]; then"),
            "不得用 if [ -f ] 包裹实现静默跳过"
        );
    }

    #[test]
    fn install_生成资产且幂等() {
        let root = temp_dir("install");
        install(&root, &["none".to_string()], false).unwrap();

        assert!(root.join(".gates/hooks/req-guard-check.sh").exists());
        assert!(root.join(".gates/req-guard.yaml").exists());
        assert!(root.join(".gates/README.md").exists());
        assert!(has_utf8_bom(&root.join(".gates/hooks/req-guard-check.ps1")));
        // deny 包装随安装落盘（Codex/Cursor 用）
        assert!(root.join(DENY_SH_REL).exists());
        assert!(has_utf8_bom(&root.join(DENY_PS1_REL)));

        // L3 默认开启（§4.3）：ci 必须 true，避免"只装不接"
        let y = fs::read_to_string(root.join(".gates/req-guard.yaml")).unwrap();
        assert!(
            y.contains("ci: true"),
            "REQ_GUARD_YAML 模板 enforce.ci 必须默认 true：\n{}",
            y
        );

        let gi = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains(".gates/.bypass"), "绕过令牌必须被忽略：{}", gi);
        assert!(
            gi.contains(".gates/audit/*.log"),
            "审计日志必须被忽略：{}",
            gi
        );

        // 幂等：声明文件不被覆盖、gitignore 不重复追加
        fs::write(root.join(".gates/req-guard.yaml"), "strict_order: false\n").unwrap();
        install(&root, &["none".to_string()], false).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(".gates/req-guard.yaml")).unwrap(),
            "strict_order: false\n",
            "已存在的声明文件不应被 install 覆盖"
        );
        assert_eq!(
            fs::read_to_string(root.join(".gitignore"))
                .unwrap()
                .matches(".gates/.bypass")
                .count(),
            1,
            "gitignore 条目不应重复追加"
        );
        cleanup(&root);
    }

    /// 按名取工具配置（测试辅助）。
    fn tp(name: &str) -> &'static ToolProfile {
        TOOL_PROFILES.iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn hook_json按工具渲染原生schema() {
        // 注入的命令按**平台**选解释器与脚本（Windows: powershell + .ps1；其余: sh + .sh），
        // 与 run_hook 取用规则一致。断言一律走 hook_script_rel，避免把平台差异写死。
        let check = hook_script_rel(false);
        let deny = hook_script_rel(true);
        let prefix = if cfg!(windows) {
            "powershell -NoProfile -ExecutionPolicy Bypass -File "
        } else {
            "sh "
        };
        let cmd = |rel: &str| format!("command\": \"{}{}\"", prefix, rel);

        // Claude：settings.json 事件大驼峰 + Write 系 matcher + 支持 env 注入
        let j = hook_json(tp("claude"));
        assert!(j.contains("\"env\""), "{}", j);
        assert!(j.contains("REQ_GUARD_AI_CTX"), "{}", j);
        assert!(j.contains("\"PreToolUse\""), "{}", j);
        assert!(j.contains("Write|Edit|MultiEdit|NotebookEdit"), "{}", j);
        assert!(j.contains(&cmd(check)), "claude 直连 check 脚本：{}", j);
        assert!(
            !j.contains("req-guard-deny"),
            "claude 不需 deny 包装：{}",
            j
        );

        // CodeBuddy：settings.json（路径已修正）+ env 段 + 直连 check 脚本
        let j = hook_json(tp("codebuddy"));
        assert!(j.contains("REQ_GUARD_AI_CTX"), "codebuddy 支持 env：{}", j);
        assert!(j.contains(&cmd(check)), "codebuddy 直连 check 脚本：{}", j);
        assert_eq!(tp("codebuddy").config, ".codebuddy/settings.json");

        // Codex：无 env、matcher=apply_patch、走 deny 包装
        let j = hook_json(tp("codex"));
        assert!(!j.contains("REQ_GUARD_AI_CTX"), "codex 无 env：{}", j);
        assert!(
            j.contains("\"apply_patch\""),
            "codex matcher 应为 apply_patch：{}",
            j
        );
        assert!(j.contains(&cmd(deny)), "codex 走 deny 包装：{}", j);
        assert!(
            !j.contains("req-guard-check"),
            "codex 不得直连 check 脚本：{}",
            j
        );

        // Cursor：事件小驼峰 preToolUse、走 deny 包装、无 env
        let j = hook_json(tp("cursor"));
        assert!(!j.contains("REQ_GUARD_AI_CTX"), "cursor 无 env：{}", j);
        assert!(
            j.contains("\"preToolUse\""),
            "cursor 用原生小驼峰事件名：{}",
            j
        );
        assert!(j.contains(&cmd(deny)), "cursor 走 deny 包装：{}", j);
    }

    #[test]
    fn install每工具写入各自原生位置() {
        let root = temp_dir("install-tools");
        install(
            &root,
            &[
                "claude".to_string(),
                "codebuddy".to_string(),
                "codex".to_string(),
                "cursor".to_string(),
            ],
            false,
        )
        .unwrap();
        // 各自原生配置文件均生成，且 schema 与路径正确
        assert!(root.join(".claude/settings.json").exists());
        assert!(
            root.join(".codebuddy/settings.json").exists(),
            "codebuddy 配置应为 settings.json"
        );
        assert!(root.join(".codex/hooks.json").exists());
        assert!(root.join(".cursor/hooks.json").exists());
        // deny 包装脚本随安装落盘
        assert!(root.join(DENY_SH_REL).exists());
        assert!(root.join(DENY_PS1_REL).exists());
        assert!(
            has_utf8_bom(&root.join(DENY_PS1_REL)),
            "deny.ps1 必须带 BOM"
        );

        // 各工具注入内容与原生 schema 一致
        let cb = fs::read_to_string(root.join(".codebuddy/settings.json")).unwrap();
        assert!(
            cb.contains("REQ_GUARD_AI_CTX"),
            "codebuddy env 注入：{}",
            cb
        );
        let cx = fs::read_to_string(root.join(".codex/hooks.json")).unwrap();
        assert!(cx.contains("req-guard-deny"), "codex deny：{}", cx);
        assert!(cx.contains("apply_patch"), "codex matcher：{}", cx);
        // 注入的实际脚本须是**本平台**那支（且随 install 落盘）
        assert!(
            cx.contains(hook_script_rel(true)),
            "codex 应引用本平台 deny 脚本 {}：{}",
            hook_script_rel(true),
            cx
        );
        assert!(
            root.join(hook_script_rel(true)).exists(),
            "注入引用的脚本必须已落盘"
        );
        let cur = fs::read_to_string(root.join(".cursor/hooks.json")).unwrap();
        assert!(cur.contains("\"preToolUse\""), "cursor 事件名：{}", cur);
        cleanup(&root);
    }

    #[test]
    fn deny包装脚本语义() {
        // 拒接信号编码：内部调 check.sh，拦截时转发为 exit 2（工具可识别）
        assert!(DENY_SH.contains("req-guard-check.sh"), "{}", DENY_SH);
        assert!(DENY_SH.contains("exit 2"), "必须 exit 2：{}", DENY_SH);
        assert!(DENY_PS1.contains("req-guard-check.ps1"), "{}", DENY_PS1);
        assert!(DENY_PS1.contains("exit 2"), "ps1 同样 exit 2：{}", DENY_PS1);
        // 放行路径不误转：check 为 0 时须原样放行
        assert!(DENY_PS1.contains("exit 0"), "放行须 exit 0：{}", DENY_PS1);
    }

    #[test]
    fn install_claude写入审批锁env段() {
        let root = temp_dir("install-claude");
        install(&root, &["claude".to_string()], false).unwrap();
        let c = fs::read_to_string(root.join(".claude/settings.json")).unwrap();
        assert!(c.contains("req-guard-check"), "{}", c);
        assert!(
            c.contains(crate::auth::AI_CTX_ENV),
            "审批锁标记必须随 hook 注入：{}",
            c
        );

        // 幂等重装：已含 hook + env → 跳过且无提示
        let mut created = Vec::new();
        let mut notes = Vec::new();
        inject_tool(&root, "claude", &mut created, &mut notes).unwrap();
        assert!(notes.is_empty(), "完整配置重装不应有提示：{:?}", notes);

        // 旧版配置（有 hook 无 env）→ 仍要提示补齐
        fs::write(
            root.join(".claude/settings.json"),
            format!(
                "{{\"hooks\":{{\"x\":[{{\"command\":\"{}\"}}]}}}}",
                hook_script_rel(false)
            ),
        )
        .unwrap();
        let mut notes = Vec::new();
        inject_tool(&root, "claude", &mut created, &mut notes).unwrap();
        assert!(
            notes.iter().any(|n| n.contains(crate::auth::AI_CTX_ENV)),
            "缺 env 段应提示手工合并：{:?}",
            notes
        );

        cleanup(&root);
    }

    #[test]
    fn 门禁识别跨平台_已装配置不因平台后缀误判() {
        // 回归（Windows CI 假红）：工具配置随仓库入库，常被不同平台的开发者先后 install
        // ——Windows 落 `.ps1`、Linux/macOS 落 `.sh`。识别 marker 若只认某一种后缀，
        // 就会在另一平台上虚报 L1 缺口 / 凭空提示"请手工合并"。
        let root = temp_dir("cross-platform");
        install(&root, &["claude".to_string()], false).unwrap();
        let wf = root.join(".github/workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("req-guard-ci.yml"),
            "name: gate\nrun: req-guard check\n",
        )
        .unwrap();

        let cfg = root.join(".claude/settings.json");
        for rel in [HOOK_SH_REL, HOOK_PS1_REL] {
            fs::write(
                &cfg,
                format!(
                    "{{\"hooks\":{{\"x\":[{{\"command\":\"{}\"}}]}},\"env\":{{\"{}\":\"1\"}}}}",
                    rel,
                    crate::auth::AI_CTX_ENV
                ),
            )
            .unwrap();
            assert!(
                verify_install(&root).is_empty(),
                "{} 形态的既有配置应被识别为已接入：{:?}",
                rel,
                verify_install(&root)
            );

            let mut created = Vec::new();
            let mut notes = Vec::new();
            inject_tool(&root, "claude", &mut created, &mut notes).unwrap();
            assert!(
                notes.is_empty(),
                "{} 形态不应触发合并提示：{:?}",
                rel,
                notes
            );
        }

        cleanup(&root);
    }

    #[test]
    fn verify_install_检出各层缺口() {
        let root = temp_dir("verify");
        // 1) 未安装：核心资产缺失即报
        assert!(!verify_install(&root).is_empty());

        // 2) 正常安装（claude：hook + env 齐备）+ 接入 CI 编排 → 全绿
        install(&root, &["claude".to_string()], false).unwrap();
        let wf = root.join(".github/workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("req-guard-ci.yml"),
            "name: gate\nrun: req-guard check\n",
        )
        .unwrap();
        assert!(
            verify_install(&root).is_empty(),
            "刚装完且已接 CI 应通过：{:?}",
            verify_install(&root)
        );

        // 3) 在用工具配置存在但不含 hook → L1 静默缺口
        fs::write(root.join(".claude/settings.json"), "{\"other\":true}").unwrap();
        let p = verify_install(&root);
        assert!(
            p.iter()
                .any(|x| x.contains(".claude/settings.json") && x.contains("L1")),
            "{:?}",
            p
        );

        // 4) 含 hook 但缺审批锁 env → 审批锁缺口
        //    这里刻意用 POSIX 形态（`sh ...check.sh`）：即便在 Windows 上跑，
        //    "已接入 hook"也必须被识别（跨平台配置互认），只报 env 缺口。
        fs::write(
            root.join(".claude/settings.json"),
            "{\"hooks\":{\"x\":[{\"command\":\"sh .gates/hooks/req-guard-check.sh\"}]}}",
        )
        .unwrap();
        let p = verify_install(&root);
        assert!(
            p.iter().any(|x| x.contains(crate::auth::AI_CTX_ENV)),
            "{:?}",
            p
        );

        // 5) .git 存在但 pre-commit 缺拦截 → L2 缺口
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\necho lint\n").unwrap();
        let p = verify_install(&root);
        assert!(p.iter().any(|x| x.contains("L2")), "{:?}", p);

        // 6) 修复 pre-commit（模拟重跑 install 的追加结果）后只剩第 4 项
        let mut pc = "#!/bin/sh\n".to_string();
        pc.push_str(PRE_COMMIT_BLOCK);
        fs::write(root.join(".git/hooks/pre-commit"), pc).unwrap();
        // 真实 install 落盘后会补执行位（否则 Unix 上 git 会静默跳过它），
        // 这里同步模拟，才能只留下"审批锁缺口"这一项。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.join(".git/hooks/pre-commit"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let p = verify_install(&root);
        assert_eq!(p.len(), 1, "仅剩审批锁缺口：{:?}", p);

        cleanup(&root);
    }

    #[test]
    fn 审计台账_关键事件入库且转义竖线() {
        let root = temp_dir("ledger");
        audit_ledger(&root, "APPROVE REQ-001 step=solution reviewer=寇工");
        audit_ledger(&root, "BYPASS-OPEN actor=kou | ttl=60min | reason=hot fix");
        let c = fs::read_to_string(root.join(LEDGER_REL)).unwrap();
        assert!(c.contains("| 时间 | 事件 |"), "首写应带表头：{}", c);
        assert!(c.contains("APPROVE REQ-001 step=solution reviewer=寇工"));
        assert!(
            c.contains("BYPASS-OPEN actor=kou \\| ttl=60min \\| reason=hot fix"),
            "自由文本中的竖线须转义：{}",
            c
        );
        // 台账/摘要不在 gitignore 忽略范围（入库是 PR 可复核的前提）
        assert!(
            GITIGNORE_LINES
                .iter()
                .all(|g| !g.contains("ledger") && !g.contains("DIGEST")),
            "{:?}",
            GITIGNORE_LINES
        );
        cleanup(&root);
    }

    #[test]
    fn 审计摘要_写入入库digest并可复算() {
        let root = temp_dir("audit-digest");
        // 无日志 → 报错提示
        assert!(audit_digest(&root).is_err());

        // 有日志 → 摘要可与本地内容独立复算一致
        audit(&root, "PASS REQ-001.md");
        let (p, hex, lines) = audit_digest(&root).unwrap();
        assert!(p.ends_with("DIGEST"));
        assert_eq!(lines, 1);
        let bytes = fs::read(root.join(".gates/audit/gate-audit.log")).unwrap();
        assert_eq!(hex, crate::digest::sha256_hex(&bytes), "摘要必须可独立复算");

        let d = fs::read_to_string(&p).unwrap();
        assert!(d.contains(&format!("lines=1 sha256={}", hex)), "{}", d);

        // 幂等追加：不覆盖历史摘要；日志变化后摘要必须变化
        audit(&root, "BLOCK REQ-001.md");
        let (_, hex2, lines2) = audit_digest(&root).unwrap();
        assert_eq!(lines2, 2);
        assert_ne!(hex, hex2, "日志变化后摘要必须变化");
        let d = fs::read_to_string(&p).unwrap();
        assert_eq!(
            d.lines().filter(|l| l.contains("sha256=")).count(),
            2,
            "历史摘要不得被覆盖：{}",
            d
        );
        cleanup(&root);
    }

    #[test]
    fn review与bypass事件进入台账() {
        let root = temp_dir("ledger-events");
        bypass(&root, "线上热修", "寇工", 60).unwrap();
        let c = fs::read_to_string(root.join(LEDGER_REL)).unwrap();
        assert!(c.contains("BYPASS-OPEN actor=寇工"), "{}", c);

        let req = crate::requirement::create(&root, None, "台账测试").unwrap();
        crate::requirement::review(&root, &req.id, "decomposition", "寇工", true, "", true)
            .unwrap();
        let c = fs::read_to_string(root.join(LEDGER_REL)).unwrap();
        assert!(
            c.contains(&format!(
                "APPROVE {} step=decomposition reviewer=寇工",
                req.id
            )),
            "{}",
            c
        );
        cleanup(&root);
    }

    #[test]
    fn bypass_写令牌并审计() {
        let root = temp_dir("bypass");
        assert!(bypass(&root, "  ", "寇工", 60).is_err(), "缺原因应被拒");
        let p = bypass(&root, "线上热修", "寇工", 60).unwrap();
        let content = fs::read_to_string(&p).unwrap();
        assert!(content.contains("reason=线上热修"));
        assert!(content.contains("expires_epoch="));
        let audit = fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap();
        assert!(audit.contains("BYPASS-OPEN"), "绕过必须留痕：{}", audit);
        cleanup(&root);
    }

    #[test]
    fn gate_check_未安装脚本时报错() {
        let root = temp_dir("gate-uninstalled");
        let e = gate_check(&root).unwrap_err();
        assert!(e.to_string().contains("未安装"), "应提示先 install：{}", e);
        cleanup(&root);
    }

    #[test]
    fn gate_check_ps1占位存在时unix仍走sh() {
        // 回归（run_hook 史缺陷）：曾按"ps1 文件存在即调 powershell"，而 install 在任意
        // 平台都会同时落盘 .sh/.ps1（跨平台资产），导致 Linux/macOS 上 `req-guard check`
        // 误调不存在的 powershell 而恒败。类 Unix 平台必须始终走 sh、忽略 ps1 占位。
        let root = temp_dir("runhook-plat");
        install(&root, &["none".to_string()], false).unwrap();
        assert!(
            root.join(HOOK_PS1_REL).exists(),
            "install 总会生成 ps1 占位（跨平台资产）"
        );
        // 无需求 → 经 run_hook 用 sh 正常执行并返回 Block（而非 powershell 报错）
        let v = gate_check(&root).unwrap();
        assert!(!v.is_pass(), "缺需求应走 sh 拦截：{}", v.summary());
        cleanup(&root);
    }

    #[test]
    fn audit_tail_取尾部并清洗bom() {
        let root = temp_dir("audit-tail");
        assert!(audit_tail(&root, 10).unwrap().is_empty(), "无日志时返回空");

        let log = root.join(".gates/audit/gate-audit.log");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(
            &log,
            "\u{feff}2026-09-11 10:00:00 PASS a\n\
             2026-09-11 10:00:01 BLOCK b\n\
             2026-09-11 10:00:02 PASS c\n",
        )
        .unwrap();

        let tail = audit_tail(&root, 2).unwrap();
        assert_eq!(tail.len(), 2);
        assert!(tail[1].contains("PASS c"));

        let all = audit_tail(&root, 99).unwrap();
        assert_eq!(all.len(), 3, "n 超过总行数时返回全部");
        assert!(
            all[0].starts_with("2026-09-11"),
            "首行 BOM 应被清洗：{:?}",
            all[0]
        );
        cleanup(&root);
    }

    #[test]
    fn hook_脚本输出绕过标记() {
        // 脚本是 raw string，无法插值 BYPASS_MARKER，只能靠本用例锁定两端一致：
        // 一旦有人改了文案却忘了脚本，绕过放行就会被谎报成"三段已批准"。
        assert!(
            HOOK_SH.contains(BYPASS_MARKER),
            "sh 脚本必须输出绕过标记 {}",
            BYPASS_MARKER
        );
        assert!(
            HOOK_PS1.contains(BYPASS_MARKER),
            "ps1 脚本必须输出绕过标记 {}",
            BYPASS_MARKER
        );
    }

    #[test]
    fn pretool_证据保护_拦截改写评论文件() {
        let root = temp_dir("pretool");
        // 关键回归：路径含 Unicode 转义（\u002f == '/'）。脚本用 sed 抠字段时匹配不到，
        // 会静默放过——Rust 侧真解析必须拦下，这是本次改造要解决的核心漏洞。
        let escaped = r#"{"tool_name":"Write","tool_input":{"file_path":".gates\u002frequirements\u002fREQ-001.comments.md"}}"#;
        match pretool_verdict(&root, escaped) {
            PretoolVerdict::Block(reason) => {
                assert!(reason.contains("REQ-001.comments.md"), "{}", reason)
            }
            other => panic!("转义过的评论文件路径必须被拦截，实际：{:?}", other),
        }

        // 明文路径同样拦截
        let plain = r#"{"tool_input":{"file_path":"a/b.comments.md"}}"#;
        assert!(matches!(
            pretool_verdict(&root, plain),
            PretoolVerdict::Block(_)
        ));
        // 普通源码文件 → 交给后续门禁（本层不拦）
        assert!(matches!(
            pretool_verdict(
                &root,
                r#"{"tool_input":{"file_path":"src/main.rs","content":"x"}}"#
            ),
            PretoolVerdict::Continue
        ));
        // 取不到路径信息：不硬拦（会误伤），交给门禁后续接管
        assert!(matches!(
            pretool_verdict(&root, "not json"),
            PretoolVerdict::Continue
        ));

        // 拦截必须留痕
        let log = fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap();
        assert!(
            log.contains("BLOCK-AI-WRITE-COMMENTS"),
            "证据保护事件须入审计：{}",
            log
        );
        cleanup(&root);
    }

    #[test]
    fn pretool_归档区彻底禁写() {
        let root = temp_dir("pretool-archive");
        // 归档区路径即使只是写正文（状态行未变）也必须拦——读档案是历史，写不行
        let payload = r#"{"tool_input":{"file_path":".gates/requirements/archive/2026/REQ-001.md","content":"新正文"}}"#;
        match pretool_verdict(&root, payload) {
            PretoolVerdict::Block(reason) => {
                assert!(reason.contains("归档"), "原因应点明归档区：{}", reason)
            }
            other => panic!("归档区写入必须拦截，实际：{:?}", other),
        }
        // Windows 反斜杠路径同样拦截
        let win = r#"{"tool_input":{"file_path":".gates\\requirements\\archive\\misc\\REQ-001.md","content":"x"}}"#;
        assert!(matches!(
            pretool_verdict(&root, win),
            PretoolVerdict::Block(_)
        ));
        // 拦截留痕
        let log = fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap();
        assert!(log.contains("BLOCK-AI-WRITE-ARCHIVE"), "{}", log);
        cleanup(&root);
    }

    #[test]
    fn pretool_清单正文可写但状态行不可改() {
        let root = temp_dir("pretool-doc");
        let rel = ".gates/requirements/REQ-001.md";
        let g1 = "<!-- GATE:HEAD id=REQ-001 status=draft -->";
        let g2 = "<!-- GATE:STEP name=decomposition status=pending -->";
        fs::create_dir_all(root.join(".gates/requirements")).unwrap();
        fs::write(root.join(rel), format!("{}\n{}\n正文", g1, g2)).unwrap();

        // 1) 整篇覆盖且状态行原样 → 放行（AI 才写得成三段正文）
        let keep = format!(
            r#"{{"tool_input":{{"file_path":"{}","content":"{}\n{}\n正文"}}}}"#,
            rel, g1, g2
        );
        assert!(
            matches!(pretool_verdict(&root, &keep), PretoolVerdict::AllowDoc),
            "状态行未变时应放行正文：{}",
            keep
        );

        // 2) 把 status=pending 改成 approved → 拦（这就是自批）
        let tampered = format!(
            r#"{{"tool_input":{{"file_path":"{}","content":"{}\n{}\n正文"}}}}"#,
            rel,
            g1,
            g2.replace("status=pending", "status=approved")
        );
        match pretool_verdict(&root, &tampered) {
            PretoolVerdict::Block(reason) => {
                assert!(reason.contains("自批"), "原因应点明是自批：{}", reason)
            }
            other => panic!("篡改状态行必须被拦截，实际：{:?}", other),
        }

        // 3) 片段编辑（Edit）→ 拦：只提交 old/new 片段，无法与磁盘基线比对
        let edit = format!(
            r#"{{"tool_input":{{"file_path":"{}","old_string":"status=pending","new_string":"status=approved"}}}}"#,
            rel
        );
        assert!(
            matches!(pretool_verdict(&root, &edit), PretoolVerdict::Block(_)),
            "片段编辑无法校验状态行，必须拦"
        );

        // 4) 新建清单却自带状态行 → 拦；纯正文 → 放行
        let new_bad = r#"{"tool_input":{"file_path":".gates/requirements/REQ-002.md","content":"<!-- GATE:STEP name=decomposition status=approved -->"}}"#;
        assert!(matches!(
            pretool_verdict(&root, new_bad),
            PretoolVerdict::Block(_)
        ));
        let new_ok =
            r#"{"tool_input":{"file_path":".gates/requirements/REQ-002.md","content":"需求正文"}}"#;
        assert!(matches!(
            pretool_verdict(&root, new_ok),
            PretoolVerdict::AllowDoc
        ));

        // 篡改尝试必须留痕
        let log = fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap();
        assert!(
            log.contains("BLOCK-AI-TAMPER-STATUS"),
            "自批尝试须入审计：{}",
            log
        );
        cleanup(&root);
    }

    #[test]
    fn hook_脚本优先调用rust解析且保留兜底() {
        assert!(
            HOOK_SH.contains("req-guard hook-check"),
            "sh 脚本应优先走 Rust 真解析"
        );
        assert!(HOOK_PS1.contains("hook-check"), "ps1 脚本同理");
        // 放行清单正文的标记两端都要认，否则 AI 写不了正文（文档流程第 2 步被自己拦死）
        assert!(
            HOOK_SH.contains(ALLOW_DOC_MARKER) && HOOK_PS1.contains(ALLOW_DOC_MARKER),
            "脚本须识别清单正文放行标记 {}",
            ALLOW_DOC_MARKER
        );
        // 二进制不在 PATH（受限环境）时仍需正则兜底，否则保护直接消失
        assert!(HOOK_SH.contains("file_path"), "sh 兜底分支仍需正则粗判");
        assert!(HOOK_PS1.contains("file_path"), "ps1 兜底分支仍需正则粗判");
    }

    #[test]
    fn gate_check_绕过窗口放行时标记bypassed() {
        let root = temp_dir("check-bypass");
        install(&root, &["none".to_string()], false).unwrap();
        assert!(
            !gate_check(&root).unwrap().is_pass(),
            "前置条件：无需求时本应拦截，以确保放行确实由绕过窗口导致"
        );

        bypass(&root, "联调临时放行", "tester", 60).unwrap();
        let v = gate_check(&root).unwrap();
        assert!(v.is_pass(), "绕过窗口内应放行");
        assert!(
            v.bypassed(),
            "靠绕过放行必须被标记，否则界面会谎报三段已批准：{}",
            v.summary()
        );
        assert!(
            v.summary().contains("绕过"),
            "放行 summary 必须点明是绕过：{}",
            v.summary()
        );
        assert!(
            !v.detail().iter().any(|l| l.contains(BYPASS_MARKER)),
            "机器标记不应混进界面明细：{:?}",
            v.detail()
        );
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn install_落盘的钩子带执行位() {
        // 回归（P0）：install 曾只用 fs::write 落盘（0644），而 git 在 Unix 上
        // **静默跳过**不可执行钩子——提交照常成功、无任何提示，--verify 还报全绿。
        let root = temp_dir("hook-mode");
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        install(&root, &["none".to_string()], false).unwrap();
        let wf = root.join(".github/workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("req-guard-ci.yml"),
            "name: gate\nrun: req-guard check\n",
        )
        .unwrap();

        assert!(
            verify_install(&root).is_empty(),
            "刚装完且已接 CI 不应有缺口：{:?}",
            verify_install(&root)
        );
        assert!(
            is_executable(&root.join(".git/hooks/pre-commit")),
            "pre-commit 必须可执行，否则 git 静默跳过、L2 形同虚设"
        );
        assert!(
            is_executable(&root.join(HOOK_SH_REL)),
            "拦截脚本应可在终端直接执行"
        );
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn install_修复旧版遗留的不可执行pre_commit() {
        use std::os::unix::fs::PermissionsExt;
        // 老仓库由旧版 install 装过（无执行位），重跑 install 会命中"已接入"并提前返回。
        // 若不在此补 chmod，这些仓库的 L2 永远修不好——升级也需要自愈。
        let root = temp_dir("hook-mode-fix");
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        let hook = root.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nsh .gates/hooks/req-guard-check.sh\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            !is_executable(&hook),
            "前置条件：模拟旧版遗留的不可执行钩子"
        );

        install(&root, &["none".to_string()], false).unwrap();
        assert!(is_executable(&hook), "重跑 install 必须补上执行位");
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn verify_install_检出pre_commit缺执行位() {
        use std::os::unix::fs::PermissionsExt;
        // 内容对 ≠ 生效：只查内容会让 CI 拿着失效门禁报绿，必须单独查执行位。
        let root = temp_dir("verify-mode");
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        install(&root, &["none".to_string()], false).unwrap();
        let wf = root.join(".github/workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("req-guard-ci.yml"),
            "name: gate\nrun: req-guard check\n",
        )
        .unwrap();
        assert!(verify_install(&root).is_empty(), "刚装完且已接 CI 应全绿");

        let hook = root.join(".git/hooks/pre-commit");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
        let problems = verify_install(&root);
        assert!(
            problems.iter().any(|p| p.contains("执行位")),
            "缺执行位必须被 verify 检出：{:?}",
            problems
        );
        cleanup(&root);
    }

    #[test]
    fn verify_install_检出ci未接入() {
        let root = temp_dir("verify-ci");
        install(&root, &["none".to_string()], false).unwrap();

        // 1) 无任何 CI 编排 → L3 未落地（红）
        let p = verify_install(&root);
        assert!(
            p.iter().any(|x| x.contains("L3")),
            "无编排必须检出：{:?}",
            p
        );

        // 2) 有编排但不含 req-guard → 红（L3 缺口：有 CI 却漏接）
        let wf = root.join(".github/workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(wf.join("other.yml"), "name: build\non: [push]\n").unwrap();
        let p = verify_install(&root);
        assert!(
            p.iter()
                .any(|x| x.contains("L3") && x.contains("req-guard")),
            "编排不含 req-guard 必须检出：{:?}",
            p
        );

        // 3) 接入后（编排含 req-guard）→ CI 项通过，无 L3 缺口
        fs::write(
            wf.join("req-guard-ci.yml"),
            "name: gate\non: [push]\nsteps:\n  - run: req-guard check\n",
        )
        .unwrap();
        let p = verify_install(&root);
        assert!(
            p.iter().all(|x| !x.contains("L3")),
            "接入后不应有 L3 缺口：{:?}",
            p
        );

        // 4) 显式 enforce.ci: false → 声明放弃 L3，不体检
        fs::remove_dir_all(&wf).unwrap();
        fs::write(
            root.join(".gates/req-guard.yaml"),
            "version: 1\nenforce:\n  ci: false\n",
        )
        .unwrap();
        let p = verify_install(&root);
        assert!(
            p.iter().all(|x| !x.contains("L3")),
            "显式放弃后不得报 L3：{:?}",
            p
        );
        cleanup(&root);
    }

    #[test]
    fn enforce_ci_读yaml且缺省fail_closed() {
        let root = temp_dir("enforce-ci");
        assert!(enforce_ci(&root), "缺文件时应 fail-closed = true");

        fs::create_dir_all(root.join(".gates")).unwrap();
        fs::write(
            root.join(".gates/req-guard.yaml"),
            "version: 1\nenforce:\n  ai_tool_hook: true\n  ci: false\n",
        )
        .unwrap();
        assert!(!enforce_ci(&root));

        fs::write(
            root.join(".gates/req-guard.yaml"),
            "version: 1\nenforce:\n  ci: true\n",
        )
        .unwrap();
        assert!(enforce_ci(&root));

        // 模板真实形态：值带行内注释（`ci: false   # 说明`）——注释不得参与判定
        fs::write(
            root.join(".gates/req-guard.yaml"),
            "version: 1\nenforce:\n  ai_tool_hook: true   # AI 工具 PreToolUse\n  ci: false             # CI 侧拦截\n",
        )
        .unwrap();
        assert!(!enforce_ci(&root), "行内注释后的 false 必须被识别");

        // enforce 区块外的同名键不参与判定（未声明时 fail-closed = true）
        fs::write(
            root.join(".gates/req-guard.yaml"),
            "version: 1\nci: false\nenforce:\n  pre_commit: true\n",
        )
        .unwrap();
        assert!(enforce_ci(&root), "enforce 外 ci 键不得干扰判定");
        cleanup(&root);
    }

    #[test]
    fn default_tools_覆盖全部内置工具() {
        let def = default_tools();
        for t in known_tools() {
            assert!(
                def.iter().any(|d| d == t),
                "默认注入应覆盖 {}（实际：{:?}）",
                t,
                def
            );
        }
    }
}

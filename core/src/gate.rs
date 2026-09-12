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

/// 内置 AI 工具配置映射（借鉴 teamai：声明式注入各工具原生配置）。
struct ToolProfile {
    name: &'static str,
    config: &'static str,
    /// 该工具配置支持**会话级环境变量注入**（如 Claude Code `settings.json` 的
    /// `env` 段）——注入 [`crate::auth::AI_CTX_ENV`] 后，审批锁（方案 A）才能
    /// 覆盖该工具会话内的 Shell 路径。未确认支持的暂为 false（P2 接原生 schema 时补）。
    env_capable: bool,
}

const TOOL_PROFILES: [ToolProfile; 4] = [
    ToolProfile {
        name: "claude",
        config: ".claude/settings.json",
        env_capable: true,
    },
    ToolProfile {
        name: "codex",
        config: ".codex/hooks.json",
        env_capable: false,
    },
    ToolProfile {
        name: "codebuddy",
        config: ".codebuddy/hooks.json",
        env_capable: false,
    },
    ToolProfile {
        name: "cursor",
        config: ".cursor/hooks.json",
        env_capable: false,
    },
];

/// 支持的 AI 工具名。
pub fn known_tools() -> Vec<&'static str> {
    TOOL_PROFILES.iter().map(|t| t.name).collect()
}

/// 默认注入的工具（本机以 WorkBuddy/CodeBuddy 为主，同时覆盖 Claude Code）。
pub fn default_tools() -> Vec<String> {
    vec!["claude".to_string(), "codebuddy".to_string()]
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
    created.push(write_file(root, ".gates/requirements/.gitkeep", "")?);
    created.push(write_file(root, ".gates/audit/.gitkeep", "")?);

    for t in tools {
        if t == "none" {
            continue;
        }
        inject_tool(root, t, &mut created, &mut notes)?;
    }

    append_pre_commit(root, &mut created, &mut notes)?;
    append_gitignore(root, &mut created, &mut notes)?;

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
    Pass { summary: String },
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
            GateVerdict::Pass { summary } | GateVerdict::Block { summary, .. } => summary,
        }
    }

    /// 拦截原因明细（放行时为空）。
    pub fn detail(&self) -> &[String] {
        match self {
            GateVerdict::Pass { .. } => &[],
            GateVerdict::Block { detail, .. } => detail,
        }
    }
}

/// 执行拦截脚本，返回 `(是否放行, stdout, stderr)`。
///
/// 脚本是唯一判定逻辑（见《技术方案.md》§1.2），本函数只负责调用与收集输出。
fn run_hook(root: &Path) -> Result<(bool, String, String)> {
    let sh = root.join(HOOK_SH_REL);
    let ps1 = root.join(HOOK_PS1_REL);

    // 旧版 install 写的 ps1 没有 BOM，会让 PowerShell 直接解析失败（恒拦截）。
    // 这里给出可操作提示，避免把"脚本崩了"误读成"门禁在正常工作"。
    if ps1.exists() && !has_utf8_bom(&ps1) {
        eprintln!(
            "⚠️ 拦截脚本 {} 缺少 UTF-8 BOM，Windows PowerShell 会解析失败（表现为恒拦截）。\
             请重新执行 req-guard install 修复。",
            ps1.display()
        );
    }

    let (prog, args): (&str, Vec<String>) = if ps1.exists() {
        (
            "powershell",
            vec![
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                ps1.to_string_lossy().to_string(),
            ],
        )
    } else if sh.exists() {
        ("sh", vec![sh.to_string_lossy().to_string()])
    } else {
        return Err(GateError::Validation(
            "未安装 AI 需求门禁（缺少 .gates/hooks/req-guard-check.*），请先执行 req-guard install"
                .into(),
        ));
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
pub fn gate_check(root: &Path) -> Result<GateVerdict> {
    let (ok, stdout, stderr) = run_hook(root)?;
    let detail: Vec<String> = format!("{}\n{}", stdout, stderr)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if ok {
        Ok(GateVerdict::Pass {
            summary: "✅ 门禁放行：三段已批准且无未解决的阻塞性评论".to_string(),
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
        "BYPASS-OPEN actor={} ttl={}min reason={}",
        one_line(actor),
        ttl_minutes,
        one_line(reason)
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

    if path.exists() {
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.contains("req-guard-check") {
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
            hook_json(prof.env_capable)
        ));
        return Ok(());
    }
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| GateError::Io {
            path: Some(p.to_path_buf()),
            source: e,
        })?;
    }
    fs::write(&path, hook_json(prof.env_capable)).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    created.push(path);
    Ok(())
}

/// 生成 AI 工具 hook 配置（Claude Code 风格 schema，其余工具形态相近）。
///
/// `env_capable=true` 时同时注入会话级 `env` 段，把 [`crate::auth::AI_CTX_ENV`]
/// 打进 AI 会话——`approve/reject/resolve/bypass` 检测到即自拒（审批锁，方案 A）。
fn hook_json(env_capable: bool) -> String {
    let cmd = if cfg!(windows) {
        "powershell -NoProfile -ExecutionPolicy Bypass -File .gates/hooks/req-guard-check.ps1"
    } else {
        "sh .gates/hooks/req-guard-check.sh"
    };
    let env = if env_capable {
        format!(
            "  \"env\": {{\n    \"{}\": \"1\"\n  }},\n",
            crate::auth::AI_CTX_ENV
        )
    } else {
        String::new()
    };
    format!(
        "{{\n{}  \"hooks\": {{\n    \"PreToolUse\": [\n      {{\n        \
         \"matcher\": \"Write|Edit|MultiEdit|NotebookEdit\",\n        \
         \"hooks\": [\n          {{\n            \"type\": \"command\",\n            \
         \"command\": \"{}\"\n          }}\n        ]\n      }}\n    ]\n  }}\n}}\n",
        env, cmd
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

/// `req-guard install --verify`：校验门禁是否真正就位（§4.5，供 CI 使用）。
///
/// 返回**问题清单**：空 = 全部通过；非空 = 存在"静默缺口"，CI 应据此红。
/// 校验规则（配置文件存在即视为该工具"在用"）：
/// - 核心资产（拦截脚本 sh/ps1、门禁声明）缺失 → 问题；
/// - 在用工具的配置不含 `req-guard-check` → **L1 静默缺口**（只装不生效）；
/// - env 注入型工具（如 claude）缺 [`crate::auth::AI_CTX_ENV`] → 审批锁缺口；
/// - 有 `.git` 但 pre-commit 未含拦截 → **L2 缺口**。
///
/// 未安装（配置不存在）的工具不要求——不制造噪音；未知新工具出现时，
/// 由团队把它登记进 `TOOL_PROFILES` 后纳入校验白名单。
pub fn verify_install(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();

    for rel in [HOOK_SH_REL, HOOK_PS1_REL, ".gates/req-guard.yaml"] {
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
        if !content.contains("req-guard-check") {
            problems.push(format!(
                "{} 已存在（在用）但未接入 req-guard hook——L1 静默缺口；\
                 按 req-guard install 输出的片段手工合并",
                prof.config
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
        match fs::read_to_string(root.join(".git/hooks/pre-commit")) {
            Ok(c) if c.contains("req-guard-check") => {}
            Ok(_) => problems.push(
                ".git/hooks/pre-commit 未含 req-guard 拦截——L2 缺口；重跑 req-guard install".into(),
            ),
            Err(_) => {
                problems.push("缺少 .git/hooks/pre-commit——L2 缺口；重跑 req-guard install".into())
            }
        }
    }

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
```

打回：`req-guard reject REQ-001 --step solution --reviewer 张三 --reason "缺少回滚方案"`

## 审批锁：approve / resolve / bypass 须人类执行

`--reviewer` / `--author` 只是名字，不构成身份保证。因此 req-guard 给支持会话环境
注入的 AI 工具（如 Claude Code）写入 `"REQ_GUARD_AI_CTX": "1"`，`approve / reject /
resolve / bypass` 检测到该标记即**拒绝执行**——AI 经 Shell 自批会被堵在命令层。

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

## L3 CI 强制门禁（部署规范）

`.gates/req-guard.yaml` 默认 `enforce.ci: true`——CI 必须**独立重跑**门禁，
本机任何绕过（含 `git commit --no-verify`）都会在服务端被抵消：

1. 流水线中执行 `req-guard check`（退出码非 0 即失败）；CI 镜像内置 req-guard
   二进制，版本与 Release tag 一致（`req-guard -V` 可核对）；
2. 将该检查设为**必需（required）状态检查**：不通过禁止合并；
3. 分支保护：禁止直推 `main` 等受保护分支；
4. 建议追加一步 `req-guard install --verify`：任一在用 AI 工具缺 hook 即红，
   消除 L1 静默缺口。

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

# ---------- 1) 应急绕过窗口（有痕、有时效） ----------
if [ -f "$BYPASS_FILE" ]; then
  EXP=$(sed -n 's/.*expires_epoch=\([0-9]*\).*/\1/p' "$BYPASS_FILE" 2>/dev/null | head -1)
  case "$EXP" in ''|*[!0-9]*) EXP="";; esac
  NOW=$(date '+%s' 2>/dev/null || echo 0)
  case "$NOW" in ''|*[!0-9]*) NOW=0;; esac
  if [ -n "$EXP" ] && [ "$NOW" -lt "$EXP" ]; then
    log "BYPASS-HIT expires_epoch=$EXP"
    echo "[req-guard] 警告：命中应急绕过窗口，本次放行（已记审计日志）" >&2
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
$m0 = [regex]::Match($stdinData, '"file_path"\s*:\s*"([^"]+)"')
if ($m0.Success -and $m0.Groups[1].Value -like '*.comments.md') {
  Write-GateAudit "BLOCK-AI-WRITE-COMMENTS $($m0.Groups[1].Value)"
  Write-Error "[req-guard] 拦截：审核评论文件禁止 AI 直接修改（嫌疑人不得修改证据）。AI 回复请用：req-guard comment <需求ID> --author ai --reply C001 --text ""..."""
  exit 1
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

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cleanup, temp_dir};

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

    #[test]
    fn hook注入含审批锁标记() {
        // env 注入型工具（claude）：hook 与 AI_CTX 标记同步写入
        let j = hook_json(true);
        assert!(j.contains("req-guard-check"), "{}", j);
        assert!(
            j.contains(crate::auth::AI_CTX_ENV),
            "审批锁标记必须随 env 段注入：{}",
            j
        );
        assert!(j.contains("PreToolUse"));

        // 非 env 型工具：只写 hook，不猜 env 段（避免破坏未确认的 schema）
        let j = hook_json(false);
        assert!(j.contains("req-guard-check"));
        assert!(!j.contains(crate::auth::AI_CTX_ENV));
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
            "{\"hooks\":{\"x\":[{\"command\":\"sh .gates/hooks/req-guard-check.sh\"}]}}",
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
    fn verify_install_检出各层缺口() {
        let root = temp_dir("verify");
        // 1) 未安装：核心资产缺失即报
        assert!(!verify_install(&root).is_empty());

        // 2) 正常安装（claude：hook + env 齐备）→ 全绿
        install(&root, &["claude".to_string()], false).unwrap();
        assert!(
            verify_install(&root).is_empty(),
            "刚装完应通过：{:?}",
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
}

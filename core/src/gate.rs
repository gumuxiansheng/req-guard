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
}

const TOOL_PROFILES: [ToolProfile; 4] = [
    ToolProfile {
        name: "claude",
        config: ".claude/settings.json",
    },
    ToolProfile {
        name: "codex",
        config: ".codex/hooks.json",
    },
    ToolProfile {
        name: "codebuddy",
        config: ".codebuddy/hooks.json",
    },
    ToolProfile {
        name: "cursor",
        config: ".cursor/hooks.json",
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
    audit(
        root,
        &format!(
            "BYPASS-OPEN actor={} ttl={}min reason={}",
            one_line(actor),
            ttl_minutes,
            one_line(reason)
        ),
    );
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
    let json = hook_json();

    if path.exists() {
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.contains("req-guard-check") {
            return Ok(()); // 幂等
        }
        notes.push(format!(
            "{} 已存在且不含 req-guard 门禁配置，为避免破坏既有配置未覆盖，请手工合并以下片段：\n{}",
            prof.config, json
        ));
        return Ok(());
    }
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| GateError::Io {
            path: Some(p.to_path_buf()),
            source: e,
        })?;
    }
    fs::write(&path, json).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    created.push(path);
    Ok(())
}

/// 生成 AI 工具 hook 配置（Claude Code 风格 schema，其余工具形态相近）。
fn hook_json() -> String {
    let cmd = if cfg!(windows) {
        "powershell -NoProfile -ExecutionPolicy Bypass -File .gates/hooks/req-guard-check.ps1"
    } else {
        "sh .gates/hooks/req-guard-check.sh"
    };
    format!(
        "{{\n  \"hooks\": {{\n    \"PreToolUse\": [\n      {{\n        \
         \"matcher\": \"Write|Edit|MultiEdit|NotebookEdit\",\n        \
         \"hooks\": [\n          {{\n            \"type\": \"command\",\n            \
         \"command\": \"{}\"\n          }}\n        ]\n      }}\n    ]\n  }}\n}}\n",
        cmd
    )
}

const PRE_COMMIT_BLOCK: &str = r#"
# ===== req-guard（AI 需求门禁；追加在 gates-toolkit 之后） =====
if [ -f .gates/hooks/req-guard-check.sh ]; then
  sh .gates/hooks/req-guard-check.sh || exit 1
fi
"#;

/// `.gitignore` 需要忽略的门禁本机运行态文件。
pub const GITIGNORE_LINES: [&str; 2] = [".gates/.bypass", ".gates/audit/*.log"];

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
  ci: false            # CI 侧拦截（需在流水线中调用 req-guard check）

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

## 应急绕过（有痕、有时效）

```bash
req-guard bypass --reason "线上故障热修，事后补审" --ttl 60
```

绕过窗口内放行，但**每次都写审计日志** `.gates/audit/gate-audit.log`。
`.gates/.bypass` 已被 `.gitignore` 忽略，不会入库。

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
    fn install_生成资产且幂等() {
        let root = temp_dir("install");
        install(&root, &["none".to_string()], false).unwrap();

        assert!(root.join(".gates/hooks/req-guard-check.sh").exists());
        assert!(root.join(".gates/req-guard.yaml").exists());
        assert!(root.join(".gates/README.md").exists());
        assert!(has_utf8_bom(&root.join(".gates/hooks/req-guard-check.ps1")));

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

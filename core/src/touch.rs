//! 变更范围契约（`GATE:TOUCH`）：声明要改的 ⊇ 实际改的。
//!
//! 对应《docs/设计/AC与变更范围契约技术方案.md》§3。
//!
//! ## 为什么要它
//!
//! `GATE:TOUCH` 是本清单**唯一**「要改哪些文件」的人工声明源；frontmatter 的
//! `source_refs` 由它单向派生（§3.8），不要求人工维护第二份 —— 两处都写必然漂移，
//! 而漂移的声明等于没有声明。
//!
//! ## 判定方向是反向包含
//!
//! `实际改动 ⊆ 声明`。**多声明无罪、漏声明有罪** —— 方案可以写得比实现细，
//! 不能比实现窄。反向（要求声明恰好等于实际）会逼着人为删声明去过门禁。
//!
//! ## 聚合口径（误报是头号死因）
//!
//! 默认取**全部未 done 清单的声明并集**。按"当前活跃需求"单条比对必然误伤
//! （AI 在 REQ-002 上写代码却撞上 REQ-001 的声明集），而**误报会催生习惯性
//! `--no-verify`** —— 门禁一旦被习惯性绕过就等于不存在。
//! `touch.scope: strict` 可收紧到只比最新活跃需求；`HOOK_REQ` 可指定单份。
//!
//! `done`（已归档）清单的声明一律不参与并集：否则一份几个月前的归档清单会把
//! 声明集撑成"什么都允许"，等于关掉这道墙。

use std::path::Path;
use std::process::Command;

use crate::error::{GateError, Result};
use crate::issue::Severity;
use crate::requirement;

/// 块起始标记（独占一行）。
pub const BEGIN: &str = "<!-- GATE:TOUCH -->";
/// 块结束标记（独占一行）。
pub const END: &str = "<!-- /GATE:TOUCH -->";

/// 环境变量名：注入已暂存文件集（换行分隔）。
///
/// 存在理由不是"少跑一次 git"，而是**可测**：`verify_gate.py` 要在临时目录里
/// 构造"暂存了某个文件"的场景并断言退出码，那不能依赖真实 git 索引。
/// Windows / CI 侧也共用同一通道传参。
pub const STAGED_ENV: &str = "HOOK_STAGED_FILES";

/// 变更范围问题类别。**枚举即契约**（由 `touch_枚举覆盖门槛` 用例强制其被测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TouchIssueKind {
    /// T1 第 2 段无 GATE:TOUCH 标记。
    MissingBlock,
    /// T1 标记未闭合。
    UnclosedBlock,
    /// T1/T2 块存在但无有效条目（"方案没说改哪儿"）。
    EmptyDeclaration,
    /// T3 路径无法归一（`..` 逃出仓库根、绝对路径）。
    BadPath,
    /// T4 实际改动未被任何声明覆盖。
    NotDeclared,
    /// 第 2 段二级标题定位失败。
    SectionNotFound,
    /// 没有任何未 done 的清单 —— 无从比对（fail-closed，如实报而非静默放行）。
    NoRequirement,
}

impl TouchIssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TouchIssueKind::MissingBlock => "MissingBlock",
            TouchIssueKind::UnclosedBlock => "UnclosedBlock",
            TouchIssueKind::EmptyDeclaration => "EmptyDeclaration",
            TouchIssueKind::BadPath => "BadPath",
            TouchIssueKind::NotDeclared => "NotDeclared",
            TouchIssueKind::SectionNotFound => "SectionNotFound",
            TouchIssueKind::NoRequirement => "NoRequirement",
        }
    }
}

/// 一条变更范围问题。`message` 自含上下文（文件 / 声明清单 / 修复指引）。
#[derive(Debug, Clone)]
pub struct TouchIssue {
    pub severity: Severity,
    pub kind: TouchIssueKind,
    pub message: String,
}

impl TouchIssue {
    fn err(kind: TouchIssueKind, message: String) -> TouchIssue {
        TouchIssue {
            severity: Severity::Error,
            kind,
            message,
        }
    }
}

/// 是否存在硬伤。
pub fn has_errors(issues: &[TouchIssue]) -> bool {
    issues.iter().any(|i| i.severity.is_error())
}

/// 聚合口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchScope {
    /// 全部未 done 清单的声明并集（默认）。
    Union,
    /// 只比最新活跃清单（与 `HOOK_SH` 第 2 段同规则），范围更紧、误报更多。
    Strict,
}

/// 判定输入：变更集从哪来。
pub enum Changed {
    /// 已暂存文件（pre-commit 路径）。
    Staged(Vec<String>),
    /// 相对某个 ref 的差异（CI / L3 路径）。
    Range(String),
}

/// 归一一条路径：反斜杠 → `/`、去 `./`、去首尾空白。
///
/// **刻意不去前导 `/`**：绝对路径必须被 [`path_is_sane`] 判为 `BadPath`，
/// 而不是被静默改写成"仓库内相对路径"—— 后者会让 `/etc/passwd` 变成 `etc/passwd`
/// 并悄悄进入比对，声明与实际双双失真。
pub fn normalize_path(raw: &str) -> String {
    let s = raw.trim().replace('\\', "/");
    s.strip_prefix("./").unwrap_or(&s).trim().to_string()
}

/// 路径是否合法（不含 `..` 逃逸、不为绝对路径）。
///
/// `..` 必须拒：否则 `../secrets` 形式的声明会静默匹配到仓库外的路径，
/// 而比对结果只用于"是否声明过"，不会真的去读那个文件 —— 这类条目纯属噪音且误导。
fn path_is_sane(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.split('/').any(|seg| seg == "..")
}

/// 声明块的存在性状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// 找不到 `BEGIN`。
    Missing,
    /// 有 `BEGIN` 无 `END`。
    Unclosed,
    /// 成对存在。
    Found,
}

/// 声明块的存在性（**纯函数**）。
///
/// 单独暴露而不是让 `declared` 顺带返回：缺块与空块是**两件事**——
/// 缺块多半是"人根本不知道要声明"，空块是"知道但没写"。合并成一个
/// `EmptyDeclaration` 就丢掉了这个区分，而 `MissingBlock` / `UnclosedBlock`
/// 两个枚举变体会变成**永不触发的死契约**（枚举覆盖门槛正是为此存在）。
pub fn block_state(section: &str) -> BlockState {
    let lines: Vec<&str> = section.lines().collect();
    let Some(b) = lines.iter().position(|l| l.trim() == BEGIN) else {
        return BlockState::Missing;
    };
    if lines[b + 1..].iter().any(|l| l.trim() == END) {
        BlockState::Found
    } else {
        BlockState::Unclosed
    }
}

/// 抽出一段正文里的声明条目（**纯函数**），返回 `(归一后的路径, 出错的原文, 行号)`。
///
/// `first_line` 是该段首行的 1-based 行号。空行与 `#` 注释被忽略（不是错误）。
pub fn declared(section: &str, first_line: usize) -> (Vec<String>, Vec<(String, usize)>, usize) {
    let lines: Vec<&str> = section.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut bad: Vec<(String, usize)> = Vec::new();
    // 合并**所有**块，而不是只看第一个。
    //
    // 为什么：只取第一个块时，第二个块会被**静默忽略** —— 人明明写了声明，
    // 判定却当它不存在，随后冒出"未声明文件"的拦截，理由还指向错误的块。
    // 这正是本项目最坏的失效模式（看着在拦、其实没拦你真正写的那份）。
    // 合并全部块是"宽容但不静默"：重复写块不会削弱约束，也不会漏看。
    let mut b = 0usize;
    while b < lines.len() {
        let Some(start) = lines[b..].iter().position(|l| l.trim() == BEGIN) else {
            break;
        };
        let start = b + start;
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim() == END)
            .map_or(lines.len(), |i| start + 1 + i);
        collect_block(&lines, first_line, start, end, &mut out, &mut bad);
        b = end.max(start + 1);
    }
    (out, bad, first_line)
}

/// 抽出一个块（`BEGIN` 行号 .. `END` 行号，左闭右开）里的条目。
fn collect_block(
    lines: &[&str],
    first_line: usize,
    start: usize,
    end: usize,
    out: &mut Vec<String>,
    bad: &mut Vec<(String, usize)>,
) {
    for (k, raw) in lines[start + 1..end].iter().enumerate() {
        let ln = first_line + start + 1 + k;
        let text = raw.split('#').next().unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let p = normalize_path(text);
        if !path_is_sane(&p) {
            bad.push((text.to_string(), ln));
            continue;
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
}

/// glob 匹配（**纯函数**，零依赖）。
///
/// 语义（写死在此并由单测表锁定）：
/// - `*` **不跨** `/`；`**` 跨 `/`
/// - `a/**` 命中 `a/b/c.rs`（任意深度）
/// - `**/*.rs` **匹配零段** —— 即也命中根级 `x.rs`（用户一定会这么写）
/// - `?` 匹配单个非 `/` 字符
/// - **不支持** `{a,b}` 花括号：写了不会匹配也不会报错。格式契约里明文声明过，
///   宁可让人知道"不支持"，也不要"写了却静默不匹配"。
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let seg: Vec<&str> = path.split('/').collect();
    seg_match(&pat, &seg)
}

fn seg_match(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        // `**` 匹配零段或多段 —— 零段是 `**/*.rs` 能命中根级文件的关键
        for i in 0..=path.len() {
            if seg_match(&pat[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    if !one_match(&pat[0].chars().collect::<Vec<_>>(), path[0]) {
        return false;
    }
    seg_match(&pat[1..], &path[1..])
}

/// 单段内的 `*` / `?` 匹配（经典回溯）。
fn one_match(pat: &[char], text: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    fn go(pat: &[char], t: &[char]) -> bool {
        if pat.is_empty() {
            return t.is_empty();
        }
        match pat[0] {
            '*' => (0..=t.len()).any(|i| go(&pat[1..], &t[i..])),
            '?' => !t.is_empty() && go(&pat[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && go(&pat[1..], &t[1..]),
        }
    }
    go(pat, &t)
}

/// 取已暂存文件集。
///
/// 优先读 [`STAGED_ENV`]（换行分隔）；未设则回落 `git diff --cached`。
/// 用 `-z`（NUL 分隔）避免带空格/非 ASCII 的路径被 git 加引号后与真实路径对不上。
pub fn staged_files(root: &Path) -> Result<Vec<String>> {
    if let Ok(v) = std::env::var(STAGED_ENV) {
        if !v.trim().is_empty() {
            return Ok(v
                .lines()
                .map(normalize_path)
                .filter(|s| !s.is_empty())
                .collect());
        }
    }
    git_names(
        root,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=ACMRD",
            "-z",
        ],
    )
}

/// 取相对某个 ref 的差异文件集（L3：CI 用它抵消 `--no-verify`）。
pub fn diff_files(root: &Path, base: &str) -> Result<Vec<String>> {
    let range = format!("{base}...HEAD");
    git_names(root, &["diff", "--name-only", &range, "-z"])
}

fn git_names(root: &Path, args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| GateError::External {
            command: format!("git {}", args.join(" ")),
            status: None,
            stderr: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(GateError::External {
            command: format!("git {}", args.join(" ")),
            status: Some(out.status),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.trim().is_empty())
        .map(normalize_path)
        .collect())
}

/// 收集参与比对的声明（按口径）。
fn collect_declarations(
    root: &Path,
    scope: TouchScope,
    only: Option<&str>,
) -> Result<Vec<(String, Vec<String>)>> {
    let mut reqs = requirement::list(root)?;
    // `list()` 只扫顶层，天然不含归档区 —— done 清单仍在顶层，按 head 状态剔除。
    let mut live: Vec<requirement::Requirement> = Vec::new();
    for r in reqs.drain(..) {
        let content = std::fs::read_to_string(&r.path).unwrap_or_default();
        if requirement::head_status(&content) != "done" {
            live.push(r);
        }
    }
    if let Some(id) = only {
        live.retain(|r| r.id == id);
        if live.is_empty() {
            return Err(GateError::Validation(format!(
                "未找到未归档的需求 {id}（--req 指定的比对对象不存在或已归档）"
            )));
        }
    }
    if scope == TouchScope::Strict && only.is_none() && !live.is_empty() {
        // 与 HOOK_SH 第 2 段同规则：文件名倒序第一个非 done
        live.sort_by(|a, b| b.path.file_name().cmp(&a.path.file_name()));
        live.truncate(1);
    }
    let mut out = Vec::new();
    for r in &live {
        let content = std::fs::read_to_string(&r.path).unwrap_or_default();
        let Some((start, end)) = requirement::section_span(&content, 1) else {
            return Ok(vec![(
                r.id.clone(),
                vec![format!(
                    "\u{0}SECTION_NOT_FOUND\u{0}清单 {} 的「## 2. 技术方案」二级标题定位失败",
                    r.id
                )],
            )]);
        };
        let lines: Vec<&str> = content.lines().collect();
        let section = lines[start - 1..end].join("\n");
        match block_state(&section) {
            BlockState::Missing => {
                out.push((
                    r.id.clone(),
                    vec![format!(
                        "\u{0}MISSING_BLOCK\u{0}清单 {} 的「## 2. 技术方案」里没有 {} 标记块",
                        r.id, BEGIN
                    )],
                ));
                continue;
            }
            BlockState::Unclosed => {
                out.push((
                    r.id.clone(),
                    vec![format!(
                        "\u{0}UNCLOSED_BLOCK\u{0}清单 {} 的声明块有 {} 但没有 {}",
                        r.id, BEGIN, END
                    )],
                ));
                continue;
            }
            BlockState::Found => {}
        }
        let (paths, bad, _) = declared(&section, start);
        let mut entries = paths;
        for (raw, ln) in bad {
            entries.push(format!("\u{0}BAD_PATH\u{0}{ln}\u{0}{raw}"));
        }
        entries.insert(0, String::new()); // 占位：保证非空，便于区分"空声明"
        entries.remove(0);
        out.push((r.id.clone(), entries));
    }
    Ok(out)
}

/// 校验一个变更集（**纯逻辑**，不碰 git）：`decls` 是 `(需求 ID, 声明条目)` 列表。
///
/// 拆出这一层是为了让「并集 / strict / exempt / 漏声明」全部可单测，不必构造 git 索引。
pub fn check_changes(
    root: &Path,
    decls: &[(String, Vec<String>)],
    changed: &[String],
) -> Vec<TouchIssue> {
    let mut issues = Vec::new();
    let exempt = crate::gate::touch_exempt_patterns(root);
    let mut flat: Vec<String> = Vec::new();
    let mut any_entry = false;
    for (_id, entries) in decls {
        for e in entries {
            if let Some(rest) = e.strip_prefix('\u{0}') {
                let mut it = rest.split('\u{0}');
                let tag = it.next().unwrap_or("");
                match tag {
                    "SECTION_NOT_FOUND" => issues.push(TouchIssue::err(
                        TouchIssueKind::SectionNotFound,
                        it.collect::<Vec<_>>().join(""),
                    )),
                    "MISSING_BLOCK" | "UNCLOSED_BLOCK" => {
                        let (k, tip) = if tag == "MISSING_BLOCK" {
                            (
                                TouchIssueKind::MissingBlock,
                                format!(
                                    "请在技术方案段加入 {BEGIN} … {END}，并逐行写出路径或 glob。"
                                ),
                            )
                        } else {
                            (
                                TouchIssueKind::UnclosedBlock,
                                format!("请补上收尾标记 {END}。"),
                            )
                        };
                        issues.push(TouchIssue::err(
                            k,
                            format!("{}\n{}", it.collect::<Vec<_>>().join(""), tip),
                        ));
                    }
                    "BAD_PATH" => {
                        let ln = it.next().unwrap_or("");
                        let raw = it.next().unwrap_or("");
                        issues.push(TouchIssue::err(
                            TouchIssueKind::BadPath,
                            format!(
                                "变更范围声明第 {ln} 行（{raw:?}）无法归一：\
                                 不得包含 `..` 或以 `/` 开头。该条目已从比对中剔除。"
                            ),
                        ));
                    }
                    _ => {}
                }
                continue;
            }
            any_entry = true;
            flat.push(e.clone());
        }
    }
    if decls.is_empty() {
        issues.push(TouchIssue::err(
            TouchIssueKind::NoRequirement,
            "没有任何未归档的需求清单，无从比对变更范围。\n\
             请先 req-guard create 并走完三段审核 —— \
             没有声明源的提交一律拦下（fail-closed）。"
                .to_string(),
        ));
        return issues;
    }
    if !any_entry {
        issues.push(TouchIssue::err(
            TouchIssueKind::EmptyDeclaration,
            format!(
                "全部 {} 份清单的 {} 块都没有任何有效声明条目。\n\
                 「方案没说改哪儿」就不能核对改动范围 —— 请在技术方案段逐行写出路径或 glob。",
                decls.len(),
                BEGIN
            ),
        ));
    }
    for f in changed {
        let f = normalize_path(f);
        if f.is_empty() {
            continue;
        }
        if exempt.iter().any(|p| glob_match(p, &f)) {
            continue;
        }
        if flat.iter().any(|d| glob_match(d, &f)) {
            continue;
        }
        issues.push(TouchIssue::err(
            TouchIssueKind::NotDeclared,
            format!(
                "`{f}` 不在任何需求的变更范围声明里。\n\
                 处置三选一：\n\
                 1) 改方案：确认这次改动确实在范围内 → 在技术方案的 {} 块里补上它\
                 （或补一条能覆盖它的 glob）\n\
                 2) 改声明：范围确实要变 → req-guard touch --declare \"<glob>\" --reason \"...\"\
                 （会打回技术方案重新过审）\n\
                 3) 确实越界：git commit --no-verify（L3 的 --base 校验仍会挡住）",
                BEGIN
            ),
        ));
    }
    issues
}

/// 端到端判定：读清单 + 取变更集 + [`check_changes`]。
pub fn check(
    root: &Path,
    scope: TouchScope,
    only: Option<&str>,
    src: &Changed,
) -> Result<Vec<TouchIssue>> {
    let changed = match src {
        Changed::Staged(list) => list.clone(),
        Changed::Range(base) => diff_files(root, base)?,
    };
    let decls = collect_declarations(root, scope, only)?;
    Ok(check_changes(root, &decls, &changed))
}

/// `touch --declare`：向清单的声明块**追加** glob（去重保序），并按配置打回技术方案。
///
/// 走 [`crate::auth::ensure_human`]：AI 能自己扩范围的话，`--declare` 就是绕过本身。
pub fn declare(
    root: &Path,
    id: &str,
    globs: &[String],
    reason: &str,
    actor: &str,
) -> Result<Vec<String>> {
    crate::auth::ensure_human("touch --declare", root, crate::token::ScopeCheck::Any)?;
    if globs.is_empty() {
        return Err(GateError::Validation(
            "declare 需要至少一个路径或 glob".into(),
        ));
    }
    let r = requirement::find(root, id)?;
    let content = std::fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    let (start, end) = requirement::section_span(&content, 1).ok_or_else(|| {
        GateError::Validation(format!(
            "需求 {id} 的「## 2. 技术方案」二级标题定位失败，无法追加变更范围声明。"
        ))
    })?;
    let lines: Vec<&str> = content.lines().collect();
    let section = lines[start - 1..end].join("\n");
    let (mut paths, bad, _) = declared(&section, start);
    if !bad.is_empty() {
        return Err(GateError::Validation(format!(
            "第 2 段已有无法归一的声明条目，请先修掉：{}",
            bad.iter()
                .map(|(r, l)| format!("第 {l} 行 {r:?}"))
                .collect::<Vec<_>>()
                .join("、")
        )));
    }
    if paths.is_empty() {
        return Err(GateError::Validation(format!(
            "需求 {id} 的 {} 块不存在或为空，无法追加。\n\
             请先在技术方案段加入 {BEGIN} … {END}，或直接编辑该块。",
            BEGIN
        )));
    }
    let mut added = Vec::new();
    for g in globs {
        let p = normalize_path(g);
        if !path_is_sane(&p) {
            return Err(GateError::Validation(format!(
                "声明 {g:?} 无法归一：不得包含 `..` 或以 `/` 开头。"
            )));
        }
        if !paths.contains(&p) {
            paths.push(p.clone());
            added.push(p);
        }
    }
    if added.is_empty() {
        return Ok(Vec::new());
    }
    // 整篇重写：把块内行替换为新列表（顺序稳定 → diff 干净）
    let mut out: Vec<String> = Vec::new();
    let b = lines
        .iter()
        .position(|l| l.trim() == BEGIN)
        .ok_or_else(|| GateError::Validation(format!("缺少 {BEGIN}")))?;
    let e = lines
        .iter()
        .skip(b + 1)
        .position(|l| l.trim() == END)
        .map(|i| b + 1 + i)
        .ok_or_else(|| GateError::Validation(format!("缺少 {END}")))?;
    for (i, l) in lines.iter().enumerate() {
        if i == b {
            out.push(BEGIN.to_string());
        } else if i > b && i < e {
            continue; // 旧条目整体丢弃，按新列表重写
        } else {
            out.push(l.to_string());
        }
    }
    // 找到插入点：BEGIN 之后
    let at = out.iter().position(|l| l.trim() == BEGIN).unwrap() + 1;
    for (k, p) in paths.iter().enumerate() {
        out.insert(at + k, p.clone());
    }
    let mut new_content = out.join("\n");
    if !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    std::fs::write(&r.path, &new_content).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;

    // 范围扩张必须重新过审（可关，但默认开）
    let mut reopened = false;
    if crate::gate::touch_reapprove(root) {
        requirement::reopen_step(root, id, "solution")?;
        reopened = true;
    }
    let event = format!(
        "TOUCH.EXTEND {} actor={} added={} reason={} reapprove={}",
        r.id,
        crate::requirement::safe_field_for_audit(actor),
        added.join(","),
        if reason.trim().is_empty() {
            "-".to_string()
        } else {
            crate::requirement::safe_field_for_audit(reason.trim())
        },
        reopened as u8
    );
    crate::gate::audit(root, &event);
    crate::gate::audit_ledger(root, &event);
    Ok(added)
}

/// glob → `source_refs` 的**前缀降级**（§3.8）。
///
/// `source_refs` 是目录前缀 + 向下递归、不支持 glob，故带通配符的条目只能降级成
/// 目录：`core/src/**` → `core/src`。这是**有意偏保守** —— 声明范围变宽，
/// doc-guard 的 FRS004 会多报规格腐化（吵但不出事），而不是漏报
/// （看起来配好了、实际永远不判过期）。
///
/// 无通配符的条目**原样保留**（不降级）：是否真的无损取决于 doc-guard 侧对
/// "恰好存在的文件路径"如何处理，那是跨 crate 的开放问题，确认前按原样保留最直白。
pub fn to_source_refs(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let lowered = if p.contains('*') || p.contains('?') {
            let cut = p.find('*').or_else(|| p.find('?')).unwrap_or(p.len());
            let mut base = &p[..cut];
            while base.ends_with('/') {
                base = &base[..base.len() - 1];
            }
            // `core/*.rs` → cut 落在 `*`，base = "core/" → "core"
            base.to_string()
        } else {
            p.clone()
        };
        let v = if lowered.is_empty() {
            p.clone()
        } else {
            lowered
        };
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
#[allow(non_snake_case)] // 与 gate.rs / idcheck.rs / ac.rs 的中文测试命名一致
mod tests {
    use super::*;

    fn sec(inner: &str) -> String {
        format!("## 2. 技术方案\n\n{BEGIN}\n{inner}\n{END}\n")
    }

    // ---------- glob_match：语义表 ----------

    #[test]
    fn glob_语义表() {
        let cases: &[(&str, &str, bool)] = &[
            ("core/src/**", "core/src/ac.rs", true),
            ("core/src/**", "core/src/gate/x.rs", true),
            // `**` 匹配零段，故 `a/**` 也命中 `a` 本身（git pathspec 同语义）。
            // 真实比对里暂存项都是文件，该退化情形不会命中。
            ("core/src/**", "core/src", true),
            ("core/src/**", "core/srcx/a.rs", false),
            ("core/src/*.rs", "core/src/ac.rs", true),
            ("core/src/*.rs", "core/src/gate/x.rs", false), // `*` 不跨 `/`
            ("**/*.rs", "ac.rs", true),                     // 零段：命中根级
            ("**/*.rs", "a/b/ac.rs", true),
            ("**", "a/b/c.rs", true),
            ("cli/src/cli.rs", "cli/src/cli.rs", true),
            ("cli/src/cli.rs", "cli/src/main.rs", false),
            ("a?c/x", "abc/x", true),
            ("a?c/x", "ac/x", false),
            ("*", "a", true),
            ("*", "a/b", false), // 单段 `*` 不跨 `/`
            // 默认 exempt 形态
            (".gates/**", ".gates/requirements/REQ-001-ac.md", true),
            ("target/**", "target/debug/req-guard", true),
            (".gitignore", ".gitignore", true),
        ];
        for (pat, path, want) in cases {
            assert_eq!(
                *want,
                glob_match(pat, path),
                "glob_match({pat:?}, {path:?}) 应为 {want}"
            );
        }
    }

    // ---------- declared ----------

    #[test]
    fn declared_解析去注释去重保序() {
        let s = sec("core/src/**   # 跨层级\n\ncli/src/cli.rs\ncore/src/**\n");
        let (paths, bad, _) = declared(&s, 1);
        assert_eq!(
            vec!["core/src/**".to_string(), "cli/src/cli.rs".to_string()],
            paths
        );
        assert!(bad.is_empty(), "注释与空行不是错误：{bad:?}");
    }

    #[test]
    fn declared_归一与BadPath() {
        let s = sec(".\\core\\src\\**\n./cli/src\n../outside/**\n/abs/x\n");
        let (paths, bad, _) = declared(&s, 1);
        assert!(
            paths.contains(&"core/src/**".to_string()),
            "反斜杠应归一：{paths:?}"
        );
        assert!(
            paths.contains(&"cli/src".to_string()),
            "./ 前缀应去掉：{paths:?}"
        );
        assert_eq!(2, bad.len(), "`..` 与绝对路径都应被拒：{bad:?}");
        // 第 3 行声明（1=标题 2=空行 3=BEGIN 4=第1条 …）→ `../outside/**` 在第 6 行
        assert_eq!(6, bad[0].1, "须给出行号：{bad:?}");
        assert_eq!(7, bad[1].1, "须给出行号：{bad:?}");
    }

    #[test]
    fn declared_多个块合并而非只取第一个() {
        // 只取第一个块 = 第二个块被静默忽略 → 人写了声明却被判"未声明"，
        // 且拦截理由指向错误的块。合并全部块才是"宽容但不静默"。
        let s = format!(
            "## 2. 技术方案\n\n{BEGIN}\nsrc/**\n{END}\n\n别的说明\n\n{BEGIN}\ndocs/**\n{END}\n"
        );
        let (paths, bad, _) = declared(&s, 1);
        assert_eq!(vec!["src/**".to_string(), "docs/**".to_string()], paths);
        assert!(bad.is_empty(), "{bad:?}");
    }

    #[test]
    fn declared_缺块返回空() {
        let (paths, bad, _) = declared("## 2. 技术方案\n\n没有块\n", 1);
        assert!(paths.is_empty() && bad.is_empty());
    }

    // ---------- check_changes：并集 / strict / exempt ----------

    fn decl(id: &str, items: &[&str]) -> (String, Vec<String>) {
        (
            id.to_string(),
            items.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn 并集口径_两侧文件都放行() {
        let d = vec![decl("REQ-001", &["core/**"]), decl("REQ-002", &["cli/**"])];
        assert!(check_changes(Path::new("."), &d, &["core/src/a.rs".into()]).is_empty());
        assert!(check_changes(Path::new("."), &d, &["cli/src/b.rs".into()]).is_empty());
        let bad = check_changes(Path::new("."), &d, &["docs/x.md".into()]);
        assert_eq!(1, bad.len());
        assert_eq!(TouchIssueKind::NotDeclared, bad[0].kind);
    }

    #[test]
    fn done清单不参与并集() {
        // collect_declarations 会剔除 done；此处直接验判定层：并集里没有的就没有保护
        let d = vec![decl("REQ-001", &["core/**"])];
        assert_eq!(
            1,
            check_changes(Path::new("."), &d, &["cli/x.rs".into()]).len(),
            "已归档清单的声明不在并集内 → 应拦"
        );
    }

    #[test]
    fn exempt默认含gates与target() {
        let d = vec![decl("REQ-001", &["core/**"])];
        assert!(check_changes(
            Path::new("."),
            &d,
            &[".gates/requirements/REQ-001-ac.md".into()]
        )
        .is_empty());
        assert!(check_changes(Path::new("."), &d, &["target/debug/req-guard".into()]).is_empty());
        // 门禁自己写的 .gitignore 不能被自己拦（append_gitignore 的产物）
        assert!(check_changes(Path::new("."), &d, &[".gitignore".into()]).is_empty());
    }

    #[test]
    fn 无清单时fail_closed() {
        let bad = check_changes(Path::new("."), &[], &["core/src/a.rs".into()]);
        assert_eq!(1, bad.len());
        assert_eq!(TouchIssueKind::NoRequirement, bad[0].kind);
        assert!(bad[0].message.contains("fail-closed"));
    }

    #[test]
    fn 空声明块报错() {
        let d = vec![decl("REQ-001", &[])];
        let bad = check_changes(Path::new("."), &d, &["core/src/a.rs".into()]);
        assert!(bad
            .iter()
            .any(|i| i.kind == TouchIssueKind::EmptyDeclaration));
    }

    #[test]
    fn BadPath条目被剔除但仍报错() {
        let d = vec![decl(
            "REQ-001",
            &["core/**", "\u{0}BAD_PATH\u{0}7\u{0}../outside/**"],
        )];
        let issues = check_changes(Path::new("."), &d, &["outside/x".into()]);
        assert!(issues.iter().any(|i| i.kind == TouchIssueKind::BadPath));
        assert!(
            issues.iter().any(|i| i.kind == TouchIssueKind::NotDeclared),
            "被剔除的条目不提供保护 → 仍应拦"
        );
    }

    #[test]
    fn 未声明项的三条出路都在消息里() {
        let d = vec![decl("REQ-001", &["core/**"])];
        let m = &check_changes(Path::new("."), &d, &["cli/x.rs".into()])[0].message;
        assert!(m.contains("touch --declare"), "{m}");
        assert!(m.contains("--no-verify"), "{m}");
        assert!(m.contains("改方案"), "{m}");
    }

    // ---------- to_source_refs：§3.8 派生 ----------

    #[test]
    fn source_refs_派生降级glob并去重() {
        let got = to_source_refs(&[
            "core/src/**".into(),
            "core/src/*.rs".into(),
            "cli/src/cli.rs".into(),
            "docs/**".into(),
            "docs/README.md".into(),
        ]);
        assert_eq!(
            vec![
                "core/src".to_string(),
                "cli/src/cli.rs".to_string(),
                "docs".to_string(),
                "docs/README.md".to_string()
            ],
            got,
            "带 glob 降级成目录并去重；无通配符的原样保留"
        );
        assert!(
            got.iter().all(|p| !p.contains('*') && !p.contains('?')),
            "派生结果不得残留 glob（source_refs 不支持）"
        );
    }

    #[test]
    fn source_refs_派生保序() {
        let got = to_source_refs(&["z/a.rs".into(), "b/**".into(), "a/**".into()]);
        assert_eq!(vec!["z/a.rs", "b", "a"], got);
    }

    // ---------- 枚举覆盖门槛 ----------

    #[test]
    fn touch_枚举覆盖门槛_每个kind都有用例() {
        use std::collections::BTreeSet;
        let mut covered: BTreeSet<TouchIssueKind> = BTreeSet::new();
        covered.extend(
            check_changes(Path::new("."), &[], &["a".into()])
                .iter()
                .map(|i| i.kind),
        );
        covered.extend(
            check_changes(Path::new("."), &[decl("R", &[])], &["a".into()])
                .iter()
                .map(|i| i.kind),
        );
        covered.extend(
            check_changes(Path::new("."), &[decl("R", &["core/**"])], &["a".into()])
                .iter()
                .map(|i| i.kind),
        );
        covered.extend(
            check_changes(
                Path::new("."),
                &[decl("R", &["\u{0}BAD_PATH\u{0}7\u{0}x"])],
                &["a".into()],
            )
            .iter()
            .map(|i| i.kind),
        );
        covered.extend(
            check_changes(
                Path::new("."),
                &[decl("R", &["\u{0}SECTION_NOT_FOUND\u{0}标题坏"])],
                &["a".into()],
            )
            .iter()
            .map(|i| i.kind),
        );
        // MissingBlock / UnclosedBlock 走 block_state → 哨兵 → check_changes 这条链
        assert_eq!(BlockState::Missing, block_state("## 2. 技术方案\n"));
        assert_eq!(
            BlockState::Unclosed,
            block_state(&format!("{BEGIN}\ncore/**\n"))
        );
        assert_eq!(BlockState::Found, block_state(&sec("core/**")));
        for tag in ["MISSING_BLOCK", "UNCLOSED_BLOCK"] {
            covered.extend(
                check_changes(
                    Path::new("."),
                    &[decl("R", &[&format!("\u{0}{tag}\u{0}缺块")])],
                    &["a".into()],
                )
                .iter()
                .map(|i| i.kind),
            );
        }
        for k in [
            TouchIssueKind::MissingBlock,
            TouchIssueKind::UnclosedBlock,
            TouchIssueKind::EmptyDeclaration,
            TouchIssueKind::BadPath,
            TouchIssueKind::NotDeclared,
            TouchIssueKind::SectionNotFound,
            TouchIssueKind::NoRequirement,
        ] {
            assert!(
                covered.contains(&k),
                "TouchIssueKind::{k:?} 没有任何用例命中"
            );
        }
    }
}

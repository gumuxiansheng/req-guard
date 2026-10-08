//! 有效改动行统计（`git diff -U0` 解析 + 双侧注释状态机）。
//!
//! # 为什么不能直接数 diff 行
//!
//! `git diff --numstat` 给出的是**物理**增删行。把「加 120 行注释」和「加 120 行逻辑」
//! 算成同一个数，分级就失去意义（REQ-019 §2.1 / G2）。
//!
//! # 为什么不能按 diff 行逐行判注释
//!
//! `-U0` 的 diff 只含变更行、不含上下文。一个**跨 hunk 的块注释**（`/*` 在未改动的
//! 上下文里，`*/` 在变更行里）无法从 diff 本身判定；字符串字面量里的 `//`
//! （`"https://x"`）也会被逐行正则误剔。
//!
//! 因此采用**按文件全量重建**：解析 hunk 头拿到两侧行号，再取两侧**全文**各跑一遍
//! 注释状态机，最后用行号过滤。
//!
//! # 方向性：fail-closed
//!
//! 认不出扩展名 ⇒ 不剔除注释（等于全算）⇒ 倾向高估档位。低估才是门禁失效；
//! 高估只是多走一次流程。内容读不到（`git show` 失败）⇒ **报错**，不按 0 放行。

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::error::{GateError, Result};

/// 变更集来源（三个上下文各一种，与 [`crate::resolve::PathSource`] 同构但更窄：
/// 这里只关心「要读哪一侧的全文」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// L2 pre-commit：`git diff --cached`，新侧 = 索引，旧侧 = `HEAD`。
    Staged,
    /// L3 CI：`git diff <base>...HEAD`，新侧 = `HEAD`，旧侧 = `<base>`。
    Range(String),
}

impl Side {
    /// 新侧（变更后）内容的 git 规格串（`git show <spec>` 的 `<spec>`）。
    ///
    /// 索引侧的规格是 `:<path>` —— **一个**冒号。写成 `::path` 会被 git 当成
    /// 「引用名以冒号开头」而报 unknown revision，症状是「所有暂存文件都读不到新侧
    /// 内容」，看起来像 fail-closed 生效了，实际是规格串拼错（实施期踩到）。
    fn new_spec(&self, path: &str) -> String {
        match self {
            Side::Staged => format!(":{path}"),
            Side::Range(_) => format!("HEAD:{path}"),
        }
    }

    /// 旧侧（变更前）内容的 git 规格串。
    fn old_spec(&self, path: &str) -> String {
        match self {
            Side::Staged => format!("HEAD:{path}"),
            Side::Range(base) => format!("{base}:{path}"),
        }
    }

    /// 人读的两侧来源（报错文案用；不给规格串是为了不把 `::` 那类拼写错误暴露出去）。
    fn new_label(&self) -> &'static str {
        match self {
            Side::Staged => "索引",
            Side::Range(_) => "HEAD",
        }
    }

    fn old_label(&self) -> String {
        match self {
            Side::Staged => "HEAD".to_string(),
            Side::Range(base) => base.clone(),
        }
    }
}

/// 一个变更文件的物理行号明细（**未剔除注释**，是 `-U0` 解析的直接产物）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawFile {
    pub path: String,
    /// 新增行在**新侧全文**里的 1-based 行号。
    pub added: Vec<usize>,
    /// 删除行在**旧侧全文**里的 1-based 行号。
    pub deleted: Vec<usize>,
    /// 二进制文件：无法按行判注释，按 1 有效行计（REQ-019 B-03）。
    pub binary: bool,
}

/// 一个变更文件的有效行明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    /// 有效新增行（剔除注释行与空行）。
    pub added: usize,
    /// 有效删除行（剔除注释行与空行）。
    pub deleted: usize,
    pub binary: bool,
}

impl FileStat {
    /// 该文件的有效行总数 = 有效新增 + 有效删除。
    ///
    /// 删除侧必须计入：删掉一段校验代码与加一段同样重。
    pub fn effective(&self) -> usize {
        self.added + self.deleted
    }
}

/// 一次变更集的有效行统计结果。
#[derive(Debug, Clone, Default)]
pub struct Stat {
    pub files: Vec<FileStat>,
}

impl Stat {
    pub fn total(&self) -> usize {
        self.files.iter().map(|f| f.effective()).sum()
    }
}

/// 注释语法（按扩展名映射；**未知扩展名一律 [`Syntax::None]`** = 不剔除）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// `//` 行注释 + `/* */` 可嵌套块注释 + 字符串/字符字面量（Rust）。
    Rust,
    /// `#` 行注释，尊重引号（sh / py / yml / ps1 / toml）。
    Hash { quote_aware: bool },
    /// 无注释语法（md / txt / json）：**全算**。
    None,
}

/// 按扩展名取注释语法（**纯函数**，便于 table-driven 单测）。
///
/// 未知扩展名 → [`Syntax::None`]（不剔除，fail-closed）。
pub fn syntax_of(path: &str) -> Syntax {
    match ext_of(path) {
        "rs" => Syntax::Rust,
        "sh" | "bash" | "zsh" | "py" | "ps1" | "yml" | "yaml" | "toml" | "cfg" | "ini" => {
            Syntax::Hash { quote_aware: true }
        }
        // 文档的每一行都是内容；JSON 无注释语法。
        _ => Syntax::None,
    }
}

fn ext_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => "",
    }
}

// ───────────────────────────── git 调用 ─────────────────────────────

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        // `core.quotePath=false`：否则中文路径会被转义成 `"docs/\350\247\200\202..."`，
        // 后续 glob 匹配必然落空 —— 那是「档位算错」的静默来源。
        .args(["-c", "core.quotePath=false"])
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| GateError::External {
            command: "git".to_string(),
            status: None,
            stderr: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(GateError::External {
            command: format!("git {}", args.join(" ")),
            status: Some(out.status),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 读一个 git 规格里的文件内容（`git show <spec>`，如 `HEAD:core/src/gate.rs`）。
///
/// **读不到不是「空文件」**：对象不存在（新增文件读旧侧、删除文件读新侧）由调用方
/// 用「那一侧本来就没有变更行」消化；除此之外读不到一律报错（B-05 fail-closed）。
fn git_show(root: &Path, spec: &str) -> Result<Option<String>> {
    let out = Command::new("git")
        .args(["-c", "core.quotePath=false", "show", spec])
        .current_dir(root)
        .output()
        .map_err(|e| GateError::External {
            command: "git show".to_string(),
            status: None,
            stderr: e.to_string(),
        })?;
    if out.status.success() {
        return Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()));
    }
    // 对象不存在（新增 / 删除 / 未提交）：返回 `None` 交调用方判断，不当成空内容。
    Ok(None)
}

// ───────────────────────────── diff 解析 ─────────────────────────────

/// 解析 `git diff -U0` 的输出（**纯函数**）。
///
/// 只取物理行号，不判注释 —— 注释判定必须看全文，见模块文档。
pub fn parse_diff(text: &str) -> Vec<RawFile> {
    let mut out: Vec<RawFile> = Vec::new();
    let mut cur: Option<RawFile> = None;
    // 当前 hunk 的行号游标（`@@ -a,b +c,d @@` 之后按增删推进）。
    let mut old_at = 0usize;
    let mut new_at = 0usize;
    let mut in_hunk = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(f) = cur.take() {
                out.push(f);
            }
            cur = Some(RawFile {
                path: strip_ab_prefix(&git_header_path(rest, " b/")),
                ..Default::default()
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            f.binary = true;
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            if let Some((o, n)) = parse_hunk_header(line) {
                old_at = o;
                new_at = n;
                in_hunk = true;
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        if line.starts_with('+') {
            f.added.push(new_at);
            new_at += 1;
        } else if line.starts_with('-') {
            f.deleted.push(old_at);
            old_at += 1;
        } else if line.starts_with(' ') {
            // `-U0` 不应出现上下文行；出现也不改计数（防止游标漂移的连锁误判）。
            old_at += 1;
            new_at += 1;
        }
    }
    if let Some(f) = cur.take() {
        out.push(f);
    }
    out
}

/// `diff --git a/x b/y` 头里取 `b/` 侧路径（**纯函数**）。
///
/// 取不到时退化为「去掉 `a/` 的那一侧」——重命名与含空格路径下 `diff --git` 的两个
/// 片段可能不完整，真正的权威路径以 `+++ b/...` 为准（见 [`path_of_plus_plus`]）。
fn git_header_path(rest: &str, prefix: &str) -> String {
    if let Some(idx) = rest.rfind(prefix) {
        let p = &rest[idx + prefix.len()..];
        let p = p.trim_end_matches('"');
        if !p.is_empty() {
            return p.to_string();
        }
    }
    rest.split_whitespace()
        .next()
        .map(strip_ab_prefix)
        .unwrap_or_default()
}

fn strip_ab_prefix(p: &str) -> String {
    let p = p.trim();
    if let Some(r) = p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")) {
        return r.trim_matches('"').replace('\\', "/");
    }
    p.trim_matches('"').replace('\\', "/")
}

/// 解析 hunk 头 `@@ -a,b +c,d @@` → `(旧侧起始行号, 新侧起始行号)`（**纯函数**）。
///
/// 缺省的计数按 1 处理（`@@ -3 +4 @@` 是合法的省略写法）。
pub fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let body = line.strip_prefix("@@ ")?;
    let body = body.split(" @@").next()?;
    let mut parts = body.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start = old.split(',').next()?;
    let new_start = new.split(',').next()?;
    Some((old_start.parse().ok()?, new_start.parse().ok()?))
}

// ───────────────────────────── 注释状态机 ─────────────────────────────

/// 全文扫描 → 「哪些行是注释行 / 空行」的集合（1-based）。
///
/// 判定口径：**一行内除注释外没有任何记号**才算注释行。因此
/// `code(); // 尾注` 里的代码使该行计入有效行，而纯 `// x`、块注释内部行不计入。
/// 字符串字面量算「记号」（不是注释）——`"https://x"` 因此计入有效行（U-05）。
pub fn comment_lines(content: &str, syntax: Syntax) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    if syntax == Syntax::None {
        // 无注释语法：仍然要剔除**空行**（G2 明写「剔除注释行与空行」）。
        for (i, l) in content.lines().enumerate() {
            if l.trim().is_empty() {
                out.insert(i + 1);
            }
        }
        return out;
    }
    let mut st = Machine::new(syntax);
    for (i, l) in content.lines().enumerate() {
        if st.feed(l) {
            out.insert(i + 1);
        }
    }
    out
}

/// 逐行推进的注释状态机（可跨行保持：块注释状态必须跨行延续）。
struct Machine {
    syntax: Syntax,
    /// 块注释嵌套深度（Rust 可嵌套块注释）。
    block: usize,
    /// 处于字符串字面量内（含 `'` / `"` 与 raw string）。
    string: Option<char>,
    /// 转义：下一个字符是字面量的一部分。
    escape: bool,
    /// raw string 的 `#` 个数（`r###"…"###`）。
    raw_hashes: usize,
    /// 处于单引号字符字面量内（Rust 生命周期 `'a` 不算，故只在 `-->` 类语境下启发式处理）。
    in_char: bool,
    /// 处于行注释内（`//` 或 `#`）。
    line_comment: bool,
    /// 本行尚未推进过（块注释内部判「含代码」只需要看行首一次）。
    at_line_start: bool,
}

impl Machine {
    fn new(syntax: Syntax) -> Self {
        Machine {
            syntax,
            block: 0,
            string: None,
            escape: false,
            raw_hashes: 0,
            in_char: false,
            line_comment: false,
            at_line_start: true,
        }
    }

    /// 喂一行；返回「该行是否整行都是注释 / 空行」。
    fn feed(&mut self, line: &str) -> bool {
        if line.trim().is_empty() {
            self.at_line_start = true;
            // 空行不改变任何状态（块注释里的空行仍是注释行）。
            return true;
        }
        let mut has_code = false;
        self.at_line_start = true;
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();

            // ── 字符串 / 字符字面量内部 ───────────────────────────────
            if let Some(q) = self.string {
                if self.escape {
                    self.escape = false;
                } else if c == '\\' {
                    self.escape = true;
                } else if c == q {
                    if self.raw_hashes > 0 {
                        // raw string 结束：连续 N 个 `#`
                        let mut seen = 0usize;
                        while next == Some('#') && seen < self.raw_hashes {
                            seen += 1;
                            i += 1;
                        }
                        if seen == self.raw_hashes {
                            self.string = None;
                            self.raw_hashes = 0;
                        }
                    } else {
                        self.string = None;
                    }
                }
                i += 1;
                continue;
            }
            if self.in_char {
                if self.escape {
                    self.escape = false;
                } else if c == '\\' {
                    self.escape = true;
                } else if c == '\'' {
                    self.in_char = false;
                }
                i += 1;
                continue;
            }

            // ── 块注释内部 ────────────────────────────────────────────
            if self.block > 0 {
                // 「整行都在块注释里」不等于「这一行是注释」：被注释掉的代码仍是代码改动
                // （下一轮就会被放开），漏掉它就是低估档位。故：剥掉行首空白与注释装饰
                //（`*` / `#`）后，若含代码记号（`;` `=` `{` `}` `(` `)` `=>` `::` 之一），
                // 该行计入有效行（U-02 的「块内含代码的那一行仍计入」）。
                //
                // 这是**启发式**且刻意偏保守：散文注释里出现「=」会被多算（多走一次流程），
                // 反向漏算才是门禁失效。
                if self.at_line_start && code_inside_comment(line) {
                    has_code = true;
                }
                self.at_line_start = false;
                if c == '/' && next == Some('*') {
                    self.block += 1;
                    i += 2;
                    continue;
                }
                if c == '*' && next == Some('/') {
                    self.block -= 1;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            // ── 行注释内部 ────────────────────────────────────────────
            if self.line_comment {
                i += 1;
                continue;
            }

            // ── 记号 ──────────────────────────────────────────────────
            match self.syntax {
                Syntax::Rust => {
                    if c == '/' && next == Some('/') {
                        self.line_comment = true;
                        i += 2;
                        continue;
                    }
                    if c == '/' && next == Some('*') {
                        self.block = 1;
                        i += 2;
                        continue;
                    }
                    if c == '"' {
                        self.string = Some('"');
                        has_code = true;
                        i += 1;
                        continue;
                    }
                    if c == 'r' {
                        // raw string：r"…" / r#"…"#（启发式：仅当后面确实是 `"` 才认）
                        if let Some(hashes) = raw_prefix(&chars, i) {
                            if chars.get(i + 1 + hashes) == Some(&'"') {
                                self.string = Some('"');
                                self.raw_hashes = hashes;
                                has_code = true;
                                i += 2 + hashes;
                                continue;
                            }
                        }
                    }
                    if c == '\'' {
                        // `'` 可能是生命周期（`&'a str`）。仅当闭合在同行且不成对时才当字符字面量。
                        if looks_like_char_literal(&chars, i) {
                            self.in_char = true;
                            has_code = true;
                            i += 1;
                            continue;
                        }
                        has_code = true;
                        i += 1;
                        continue;
                    }
                    has_code = true;
                    i += 1;
                }
                Syntax::Hash { quote_aware } => {
                    if quote_aware && (c == '"' || c == '\'') {
                        self.string = Some(c);
                        has_code = true;
                        i += 1;
                        continue;
                    }
                    if c == '#' {
                        self.line_comment = true;
                        i += 1;
                        continue;
                    }
                    has_code = true;
                    i += 1;
                }
                Syntax::None => {
                    has_code = true;
                    i += 1;
                }
            }
        }
        // 行尾收尾：行注释不跨行，其余状态跨行保持。
        self.line_comment = false;
        !has_code
    }
}

/// 块注释**内部**的一行是否「含代码」（**纯函数**，见 `feed` 里的口径说明）。
fn code_inside_comment(line: &str) -> bool {
    let mut t = line.trim_start();
    for deco in ["*", "#", "//"] {
        if let Some(rest) = t.strip_prefix(deco) {
            t = rest.trim_start();
        }
    }
    if t.is_empty() {
        return false;
    }
    [";", "=", "{", "}", "(", ")", "=>", "::"]
        .iter()
        .any(|mark| t.contains(mark))
}

/// `r#"…"#` 的 `#` 个数（`chars[i] == 'r'`，返回紧跟其后的连续 `#` 数）。
fn raw_prefix(chars: &[char], i: usize) -> Option<usize> {
    let mut n = 0usize;
    while chars.get(i + 1 + n) == Some(&'#') {
        n += 1;
    }
    Some(n)
}

/// `'x'` / `'\n'` 形态判定：闭合引号必须在同行且不是字母数字（否则是生命周期标注）。
fn looks_like_char_literal(chars: &[char], i: usize) -> bool {
    let Some(&q) = chars.get(i + 1) else {
        return false;
    };
    if q == '\\' {
        return chars.get(i + 3) == Some(&'\'');
    }
    if q.is_alphanumeric() || q == '_' || q == ' ' {
        return false;
    }
    chars.get(i + 2) == Some(&'\'')
}

// ───────────────────────────── 有效行统计（薄壳） ─────────────────────────────

/// 统计一次变更集的有效改动行（读 git）。
///
/// 流程：`-U0` 解析行号 → 取双侧全文跑注释状态机 → 用行号过滤 → 相加。
pub fn stat(root: &Path, side: &Side, paths: &[String]) -> Result<Stat> {
    let raw = raw_files(root, side, paths)?;
    let mut out = Stat::default();
    for f in raw {
        out.files.push(file_stat(root, side, f)?);
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// 取变更集里指定路径（空 = 全部）的 `-U0` 解析结果。
fn raw_files(root: &Path, side: &Side, paths: &[String]) -> Result<Vec<RawFile>> {
    let mut args: Vec<String> = vec!["diff".into(), "-U0".into(), "--no-color".into()];
    match side {
        Side::Staged => args.push("--cached".into()),
        Side::Range(base) => {
            args.push(format!("{base}...HEAD"));
        }
    }
    args.push("--".into());
    args.extend(paths.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let text = git(root, &refs)?;
    Ok(parse_diff(&text))
}

/// 单个文件的有效行统计（读 git）。
pub fn file_stat(root: &Path, side: &Side, raw: RawFile) -> Result<FileStat> {
    if raw.binary {
        return Ok(FileStat {
            path: raw.path,
            added: usize::from(!raw.added.is_empty() || !raw.deleted.is_empty()),
            deleted: 0,
            binary: true,
        });
    }
    let syntax = syntax_of(&raw.path);
    let skip_new = if raw.added.is_empty() {
        BTreeSet::new()
    } else {
        let content = git_show(root, &side.new_spec(&raw.path))?.ok_or_else(|| {
            GateError::Validation(format!(
                "读不到变更后内容（git show {}:{} 失败），无法统计有效改动行。\n\
                 按 fail-closed 处理：宁可报错，也不按 0 有效行放行。",
                side.new_label(),
                raw.path
            ))
        })?;
        comment_lines(&content, syntax)
    };
    let skip_old = if raw.deleted.is_empty() {
        BTreeSet::new()
    } else {
        let content = git_show(root, &side.old_spec(&raw.path))?.ok_or_else(|| {
            GateError::Validation(format!(
                "读不到变更前内容（git show {}:{} 失败），无法统计有效删除行。\n\
                 按 fail-closed 处理：宁可报错，也不按 0 有效行放行。",
                side.old_label(),
                raw.path
            ))
        })?;
        comment_lines(&content, syntax)
    };
    Ok(FileStat {
        added: raw.added.iter().filter(|n| !skip_new.contains(n)).count(),
        deleted: raw.deleted.iter().filter(|n| !skip_old.contains(n)).count(),
        path: raw.path,
        binary: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 注释状态机（T2 的地基用例） ──────────────────────────────
    #[test]
    fn u01_只改注释有效行为零() {
        let content = (0..10)
            .map(|i| format!("// 注释 {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let skip = comment_lines(&content, Syntax::Rust);
        let added: Vec<usize> = (1..=10).filter(|n| !skip.contains(n)).collect();
        assert!(added.is_empty(), "注释行未被剔除：{added:?}");
    }

    #[test]
    fn u03_三行代码五行注释得三() {
        let content = "let a = 1;\n// c1\nlet b = 2;\n/// doc\n//! inner\nlet c = 3;";
        let skip = comment_lines(content, Syntax::Rust);
        let kept: Vec<usize> = (1..=6).filter(|n| !skip.contains(n)).collect();
        assert_eq!(kept, vec![1, 3, 6]);
    }

    #[test]
    fn u02_块注释跨hunk含代码行仍计入() {
        // `/*` 在第 1 行（未改动上下文），中间含代码的行在变更行里，`*/` 收尾。
        let content =
            "/* 开头\n   这是块内纯注释\n   let x = 1; 块里的代码\n   还有注释\n*/\nlet y = 2;";
        let skip = comment_lines(content, Syntax::Rust);
        let kept: Vec<usize> = (1..=6).filter(|n| !skip.contains(n)).collect();
        assert_eq!(kept, vec![3, 6], "块内含代码的行必须计入有效行");
    }

    #[test]
    fn u02b_块注释开闭同在变更行判得对() {
        let content = "fn f() {}\n/* a */\nfn g() {}";
        let skip = comment_lines(content, Syntax::Rust);
        let kept: Vec<usize> = (1..=3).filter(|n| !skip.contains(n)).collect();
        assert_eq!(kept, vec![1, 3]);
    }

    #[test]
    fn u04_删注释与删代码两侧分别算() {
        let old = "let a = 1;\n// c1\n// c2\nlet b = 2;";
        let skip = comment_lines(old, Syntax::Rust);
        let deleted: Vec<usize> = [1, 2, 3, 4]
            .into_iter()
            .filter(|n| !skip.contains(n))
            .collect();
        assert_eq!(deleted, vec![1, 4], "删除 2 行注释 + 2 行代码 → 有效删除 2");
    }

    #[test]
    fn u05_字符串内双斜杠不误剔() {
        let content = "let u = \"https://x\";\n// 真注释\nlet v = \"a//b\";";
        let skip = comment_lines(content, Syntax::Rust);
        let kept: Vec<usize> = (1..=3).filter(|n| !skip.contains(n)).collect();
        assert_eq!(kept, vec![1, 3]);
    }

    #[test]
    fn u05b_原始字符串内注释符号不误剔() {
        let content = "let s = r#\"http://a // b\"#;\n// 真注释";
        let skip = comment_lines(content, Syntax::Rust);
        let kept: Vec<usize> = (1..=2).filter(|n| !skip.contains(n)).collect();
        assert_eq!(kept, vec![1]);
    }

    #[test]
    fn u05c_生命周期标注不算字符字面量() {
        let content = "fn f<'a>(x: &'a str) -> &'a str {\n    x\n}";
        let skip = comment_lines(content, Syntax::Rust);
        assert!(skip.is_empty(), "生命周期标注被误判为字符字面量：{skip:?}");
    }

    #[test]
    fn u06_文档不剔除仅剔空行() {
        let content = "# 标题\n\n正文一行\n\n- 列表";
        let skip = comment_lines(content, syntax_of("README.md"));
        assert_eq!(skip.into_iter().collect::<Vec<_>>(), vec![2, 4]);
    }

    #[test]
    fn u07_未知扩展名不剔除() {
        assert_eq!(syntax_of("a/b.unknownext"), Syntax::None);
        assert_eq!(syntax_of("noext"), Syntax::None);
        assert_eq!(syntax_of("a/中文.文件"), Syntax::None);
    }

    #[test]
    fn hash_语法尊重引号() {
        let content = "A=\"http://x\" # 真注释\nB=1\n# 整行注释";
        let skip = comment_lines(content, syntax_of("x.sh"));
        assert_eq!(
            skip.into_iter().collect::<Vec<_>>(),
            vec![3],
            "带代码的行必须计入，字符串里的 // 不得被误剔"
        );
        let yml = "key: value # 行尾注释";
        let skip = comment_lines(yml, syntax_of("x.yml"));
        assert!(skip.is_empty());
    }

    #[test]
    fn json_无注释语法只剔空行() {
        let content = "{\n  \"a\": 1\n}";
        let skip = comment_lines(content, syntax_of("x.json"));
        assert!(skip.is_empty());
    }

    #[test]
    fn 空行在块注释内也判为注释行() {
        let content = "/* a\n\nb */\nlet x = 1;";
        let skip = comment_lines(content, Syntax::Rust);
        assert_eq!(skip.into_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    // ── diff 解析 ────────────────────────────────────────────────
    #[test]
    fn u08_多处hunk行号与手算一致() {
        let diff = "diff --git a/core/src/a.rs b/core/src/a.rs\n\
index 111..222 100644\n\
--- a/core/src/a.rs\n\
+++ b/core/src/a.rs\n\
@@ -1,0 +2,2 @@\n\
+let a = 1;\n\
+let b = 2;\n\
@@ -10,2 +11,1 @@\n\
-let x = 1;\n\
-let y = 2;\n\
+let z = 3;\n\
diff --git a/b.md b/b.md\n\
--- a/b.md\n\
+++ b/b.md\n\
@@ -1 +1 @@\n\
-旧\n\
+新\n";
        let files = parse_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "core/src/a.rs");
        assert_eq!(files[0].added, vec![2, 3, 11]);
        assert_eq!(files[0].deleted, vec![10, 11]);
        assert_eq!(files[1].path, "b.md");
        assert_eq!(files[1].added, vec![1]);
        assert_eq!(files[1].deleted, vec![1]);
    }

    #[test]
    fn hunk_头支持省略计数与章节标题() {
        assert_eq!(parse_hunk_header("@@ -3 +4 @@"), Some((3, 4)));
        assert_eq!(parse_hunk_header("@@ -1,2 +3,4 @@ fn f()"), Some((1, 3)));
        assert_eq!(parse_hunk_header("not a hunk"), None);
    }

    #[test]
    fn 二进制文件被标记且不产生行号() {
        let diff = "diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ\n";
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert!(files[0].binary);
        assert!(files[0].added.is_empty() && files[0].deleted.is_empty());
    }

    #[test]
    fn 重命名无hunk时有效行为零() {
        let diff = "diff --git a/old.rs b/new.rs\nsimilarity index 100%\nrename from old.rs\nrename to new.rs\n";
        let files = parse_diff(diff);
        assert_eq!(files[0].path, "new.rs");
        assert!(files[0].added.is_empty() && files[0].deleted.is_empty());
    }

    #[test]
    fn 中文路径不被转义() {
        let diff = "diff --git a/docs/设计/x.md b/docs/设计/x.md\n--- a/docs/设计/x.md\n+++ b/docs/设计/x.md\n@@ -1 +1 @@\n+内容\n";
        let files = parse_diff(diff);
        assert_eq!(files[0].path, "docs/设计/x.md");
    }
}

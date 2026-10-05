//! 清单正文的**轻量 Markdown 渲染**（零新增依赖）。
//!
//! 为什么要渲染：审核人真正要读的是**内容**，不是 `- [ ]`、`**`、`` ` `` 这些标记。
//! 过去 GUI 把 `core::requirement::section_of` 的切片原样塞进只读文本框（等宽纯文本），
//! 段落、任务勾选框、表格全靠人脑翻译，审阅体验明显落后于编辑器里的同一份清单。
//!
//! 为什么不用 `egui_extras` / `egui_markdown`：
//! - **供应链**：本项目 core 零依赖、GUI 才允许重量依赖，而正文渲染是纯展示需求，
//!   为它新增一条 crates.io 依赖链（pulldown-cmark / syntect / …）不值得；
//! - **安全**：渲染器**永不激活链接、永不加载图片**。清单正文由 AI 生成，
//!   一个 `[点我](https://evil.tld)` 若能被审核人一点就唤起浏览器，
//!   等于给 AI 留了一条"骗人类点击"的通道——门禁工具尤其不能开这个口子。
//!   链接 / 图片只渲染成**带下划线的纯文本**，真实地址以悬停提示展示（要看就瞄一眼，不点）。
//! - **可控**：HTML 注释（`<!-- GATE:… -->` 标记行）整块跳过，不再糊在正文里干扰阅读。
//!
//! 渲染范围刻意"少而够用"：标题 / 段落 / 有序无序列表 / 任务列表 / 围栏代码块 /
//! 表格 / 引用 / 分隔线 + 行内的 `**粗**` `*斜*` `~~删~~` `` `码` ``。
//! 不支持的语法一律按**纯文本原样显示**（不吞、不报错）——
//! 渲染只是"更好读"的叠加层，宁可显示成原文，也不能让人看不到清单里的某句话。
//!
//! 两条被真实缺陷教出来的纪律，改渲染代码前先读 [`cells_of`] 与 [`table`] 的注释：
//! - **竖线只在真的分列时才是分隔符**：代码段内的 `` `a|b` ``、转义的 `\|` 都算内容，
//!   见 [`cells_of`]；
//! - **宽度只有版心一个来源**：段落 / 列表 / 标题 / 表格共用同一栏宽度，
//!   表格列宽也由它分配，见 [`show`] 与 [`distribute`]。
//!
//! 分层：解析（[`parse`] / [`parse_inlines`]，纯函数、不碰 egui、可单测）与渲染
//! （[`show`]）分离，测试断言的是"解析出的结构"，不需要跑窗口。

use crate::palette;
use eframe::egui;
use std::sync::Arc;

/// 块级元素。
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    List(List),
    Code { lang: String, text: String },
    Quote(Vec<Block>),
    Table(Table),
    Rule,
}

/// 列表（`ordered = false` 为无序列表）。
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    /// 有序列表的起始编号（`3. …` → 3）。
    pub start: u64,
    pub items: Vec<ListItem>,
}

/// 列表项。
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// `Some(true/false)` = GFM 任务列表项（`- [x]` / `- [ ]`）；`None` = 普通项。
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// 表格。
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub header: Vec<Vec<Inline>>,
    /// 每列对齐（由 `|:---:|---:|` 那行分隔行给出）。
    pub aligns: Vec<Align>,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// 列对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// 行内元素。
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    /// 链接：只渲染文本，`dest` 仅作悬停提示，**不可点击**（见模块注释的安全取舍）。
    Link {
        text: Vec<Inline>,
        dest: String,
    },
    /// 图片：只渲染替代文本，**不发起任何加载**。
    Image {
        alt: String,
        dest: String,
    },
}

// ===================== 解析 =====================

/// 解析整段 Markdown 文本为块序列。
pub fn parse(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        // HTML 注释整块跳过（模板里的 `<!-- GATE:… -->` 标记行不是给人看的）。
        if t.starts_with("<!--") {
            i += 1;
            // 单行注释（`<!-- … -->` 同一行内闭合）到此为止，否则继续找闭合行。
            if !lines[i - 1].contains("-->") {
                while i < lines.len() && !lines[i].contains("-->") {
                    i += 1;
                }
                i += 1;
            }
            continue;
        }
        if t.is_empty() {
            i += 1;
            continue;
        }
        if let Some(fence) = fence_of(t) {
            let lang = t[fence.len()..].trim().to_string();
            let mut text = String::new();
            i += 1;
            while i < lines.len() {
                let l = lines[i].trim_start();
                if fence_of(l).is_some_and(|f| f.len() >= fence.len()) {
                    i += 1;
                    break;
                }
                text.push_str(lines[i]);
                text.push('\n');
                i += 1;
            }
            blocks.push(Block::Code { lang, text });
            continue;
        }
        if is_rule(t) {
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if let Some((level, rest)) = heading_of(t) {
            blocks.push(Block::Heading {
                level,
                inlines: parse_inlines(rest),
            });
            i += 1;
            continue;
        }
        // 引用块：连续 `> ` 行整体取出，去前缀后**递归解析**（引用里可以再放列表 / 代码块）。
        if t.starts_with('>') {
            let mut inner = String::new();
            while i < lines.len() {
                let l = lines[i].trim_start();
                if let Some(rest) = l.strip_prefix('>') {
                    inner.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                    inner.push('\n');
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block::Quote(parse(&inner)));
            continue;
        }
        // 表格：当前行含 `|` 且下一行是 `| --- |` 形式的对齐分隔行。
        if t.contains('|') && lines.get(i + 1).is_some_and(|n| is_delimiter_row(n.trim())) {
            let aligns = aligns_of(lines[i + 1].trim());
            let header = inlines_of_row(lines[i].trim());
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && is_table_row(lines[i].trim()) {
                rows.push(inlines_of_row(lines[i].trim()));
                i += 1;
            }
            blocks.push(Block::Table(Table {
                header,
                aligns,
                rows,
            }));
            continue;
        }
        if list_marker(lines[i]).is_some() {
            let (list, eaten) = parse_list(&lines[i..]);
            blocks.push(list);
            i += eaten;
            continue;
        }
        // 段落：吃到空行 / 下一个块级起始为止。
        let mut text = String::new();
        while i < lines.len() {
            let t = lines[i].trim_start();
            if t.is_empty()
                || is_rule(t)
                || heading_of(t).is_some()
                || fence_of(t).is_some()
                || t.starts_with('>')
                || list_marker(lines[i]).is_some()
            {
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(t);
            i += 1;
        }
        blocks.push(Block::Paragraph(parse_inlines(&text)));
    }
    blocks
}

/// 解析行内元素。
pub fn parse_inlines(src: &str) -> Vec<Inline> {
    let c: Vec<char> = src.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        // 反斜杠转义。
        if ch == '\\' && i + 1 < c.len() {
            push_char(&mut buf, c[i + 1]);
            i += 2;
            continue;
        }
        // 行内代码：取最近的下一个反引号，成对才算代码，否则原样保留。
        if ch == '`' {
            if let Some(end) = find(&c, i + 1, '`') {
                push_text(&mut out, &mut buf);
                out.push(Inline::Code(c[i + 1..end].iter().collect()));
                i = end + 1;
                continue;
            }
            buf.push(ch);
            i += 1;
            continue;
        }
        // 图片 / 链接：括号不闭合就当普通字符。
        if ch == '!' || ch == '[' {
            let is_img = ch == '!' && c.get(i + 1) == Some(&'[');
            let lb = if is_img { i + 1 } else { i };
            if let Some((text, dest, end)) = link_of(&c, lb) {
                push_text(&mut out, &mut buf);
                if is_img {
                    out.push(Inline::Image { alt: text, dest });
                } else {
                    out.push(Inline::Link {
                        text: parse_inlines(&text),
                        dest,
                    });
                }
                i = end;
                continue;
            }
        }
        // 强调：`**` `__` 优先于 `*` `_`；`~~` 为删除线。
        if ch == '~' && c.get(i + 1) == Some(&'~') {
            if let Some(inner) = delim(&c, i + 2, '~', 2) {
                push_text(&mut out, &mut buf);
                out.push(Inline::Strikethrough(parse_inlines(&inner)));
                i += 2 + inner.chars().count() + 2;
                continue;
            }
        }
        if ch == '*' || ch == '_' {
            // `_` 在词内不算强调（CommonMark 的 intraword 规则）：
            // 否则 `req_guard_hook.sh`、`snake_case` 会被拆成斜体，标识符就不可读了。
            if ch == '_' && i > 0 && is_word_char(c[i - 1]) {
                buf.push(ch);
                i += 1;
                continue;
            }
            let wide = c.get(i + 1) == Some(&ch);
            let n = if wide { 2 } else { 1 };
            if let Some(inner) = delim(&c, i + n, ch, n) {
                push_text(&mut out, &mut buf);
                let inlines = parse_inlines(&inner);
                out.push(if wide {
                    Inline::Strong(inlines)
                } else {
                    Inline::Emphasis(inlines)
                });
                i += n + inner.chars().count() + n;
                continue;
            }
            // 孤立标记当普通字符（`_` / `*` 极常出现在标识符与算式里，如 `a_b * 2`）。
            push_char(&mut buf, ch);
            i += 1;
            continue;
        }
        push_char(&mut buf, ch);
        i += 1;
    }
    push_text(&mut out, &mut buf);
    out
}

/// 内嵌字体缺字形时的**降级替换**：AI 写的清单很爱用 `✅` / `❌` / `⭐`，
/// 而 Noto Sans SC 没有这些 emoji 的字形（emoji 在 Noto Color Emoji 里，本项目没内嵌），
/// 不降级就会在界面上显示成 `?`（豆腐块），审核人看到的是" inexplicable 的问号"。
/// 换成字体里有的等义单字符（`✓ ✗ ⚠ ★` 都在子集范围内）——信息不丢，也不必为几个符号再内嵌一份字体。
fn fallback_glyph(c: char) -> Option<char> {
    match c {
        '✅' | '☑' | '✔' => Some('\u{2713}'),  // ✓
        '❌' | '✖' | '❎' => Some('\u{2717}'), // ✗
        '⭐' => Some('\u{2605}'),              // ★
        '\u{fe0f}' => None,                    // 变体选择符：跟着前一个符号一起被替换掉了
        _ => None,
    }
}

fn push_char(buf: &mut String, c: char) {
    // VS16（U+FE0F）只在 emoji 后面成对出现，去掉它、保留被替换后的符号。
    if c == '\u{fe0f}' {
        return;
    }
    buf.push(fallback_glyph(c).unwrap_or(c));
}

fn push_text(out: &mut Vec<Inline>, buf: &mut String) {
    if !buf.is_empty() {
        out.push(Inline::Text(std::mem::take(buf)));
    }
}

/// 从 `lb`（`[` 的下标）开始解析 `[text](dest)`，返回文本 / 地址 / 右括号之后的下标。
fn link_of(c: &[char], lb: usize) -> Option<(String, String, usize)> {
    if c.get(lb) != Some(&'[') {
        return None;
    }
    // 找与 `lb` 处 `[` 配对的 `]`（跳过链接文字里的嵌套方括号）。
    let mut depth = 0i32;
    let mut close = None;
    for (k, ch) in c.iter().enumerate().skip(lb) {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if c.get(close + 1) != Some(&'(') {
        return None;
    }
    let text: String = c[lb + 1..close].iter().collect();
    let mut dest = String::new();
    let mut quoted = false;
    let mut k = close + 2;
    while k < c.len() && (c[k] != ')' || quoted) {
        if c[k] == '"' {
            quoted = !quoted;
        }
        dest.push(c[k]);
        k += 1;
    }
    if k >= c.len() {
        return None;
    }
    Some((text, dest.split_whitespace().collect(), k + 1))
}

/// 从 `start` 起找长度为 `n` 的成对分隔符，返回中间内容（空内容不算命中）。
fn delim(c: &[char], start: usize, ch: char, n: usize) -> Option<String> {
    let mut k = start;
    while k + n <= c.len() {
        if c[k..k + n].iter().all(|&x| x == ch) {
            let inner: String = c[start..k].iter().collect();
            if !inner.is_empty() {
                return Some(inner);
            }
        }
        k += 1;
    }
    None
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() && c.is_ascii() || c == '_'
}

fn find(c: &[char], start: usize, ch: char) -> Option<usize> {
    (start..c.len()).find(|&k| c[k] == ch)
}

/// 列表标记 → (缩进宽度, 任务勾选态, 有序编号, 正文)。
type Marker = (usize, Option<bool>, u64, String);

fn list_marker(line: &str) -> Option<Marker> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    let body = if let Some(rest) = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
    {
        rest
    } else if let Some((_, rest)) = split_ordered(t) {
        rest
    } else {
        return None;
    };
    let (checked, body) = match task_mark(body) {
        Some((c, rest)) => (Some(c), rest),
        None => (None, body),
    };
    Some((indent, checked, 0, body.to_string()))
}

/// `1. ` / `1) ` → (编号, 正文)；不是有序列表则 `None`。
fn split_ordered(t: &str) -> Option<(u64, &str)> {
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &t[digits.len()..];
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    digits.parse().ok().map(|n| (n, rest))
}

/// GFM 任务标记：`[ ]` / `[x]`（`x` 大小写不敏感）。
fn task_mark(body: &str) -> Option<(bool, &str)> {
    let rest = body.strip_prefix('[')?;
    let checked = match rest.chars().next()? {
        ' ' | ']' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    let rest = rest[1..].strip_prefix(']')?;
    Some((checked, rest.strip_prefix(' ').unwrap_or(rest)))
}

/// 解析连续的同级列表项，返回列表与"消费掉的行数"（缩进更深的行并入上一项 → 嵌套列表）。
fn parse_list(lines: &[&str]) -> (Block, usize) {
    let Some((base, _, _, _)) = list_marker(lines[0]) else {
        return (Block::Paragraph(parse_inlines(lines[0])), 1);
    };
    let mut ordered = false;
    let mut start = 1;
    let mut items: Vec<ListItem> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((indent, checked, _num, body)) = list_marker(lines[i]) else {
            break;
        };
        if indent < base {
            break;
        }
        if indent > base {
            // 缩进更深：并入上一项继续解析（`- 外\n  - 内` → 外项里含一个子列表）。
            let Some(last) = items.last_mut() else {
                break;
            };
            let mut text = body;
            let mut j = i + 1;
            while let Some(l) = lines.get(j) {
                let ind = l.len() - l.trim_start().len();
                if ind > base || (l.trim().is_empty() && next_indent(lines, j) > base) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(l.trim_start());
                    j += 1;
                } else {
                    break;
                }
            }
            last.blocks = parse(&text);
            i = j;
            continue;
        }
        if items.is_empty() {
            ordered = split_ordered(lines[i].trim_start()).is_some();
            start = split_ordered(lines[i].trim_start()).map_or(1, |(n, _)| n);
        }
        // 本项正文 = 首行内容 + 其后更深缩进 / 顶格续行（AI 常这么写）。
        let mut text = body;
        let mut j = i + 1;
        while let Some(l) = lines.get(j) {
            if l.trim().is_empty() {
                if next_indent(lines, j) > base {
                    text.push('\n');
                    j += 1;
                    continue;
                }
                break;
            }
            let ind = l.len() - l.trim_start().len();
            if ind > base || list_marker(l).is_none() {
                text.push('\n');
                text.push_str(l.trim_start());
                j += 1;
                continue;
            }
            break;
        }
        items.push(ListItem {
            checked,
            blocks: parse(&text),
        });
        i = j;
    }
    (
        Block::List(List {
            ordered,
            start,
            items,
        }),
        i,
    )
}

fn next_indent(lines: &[&str], j: usize) -> usize {
    lines
        .get(j + 1)
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(0)
}

/// 行内结构的纯文本化（渲染标题文本 / 测试断言 / 调试都用它）。
pub fn inline_text(v: &[Inline]) -> String {
    let mut s = String::new();
    for i in v {
        match i {
            Inline::Text(t) | Inline::Code(t) => s.push_str(t),
            Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c) => {
                s.push_str(&inline_text(c))
            }
            Inline::Link { text, .. } => s.push_str(&inline_text(text)),
            Inline::Image { alt, .. } => s.push_str(alt),
        }
    }
    s
}

fn fence_of(t: &str) -> Option<&'static str> {
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// `---` / `***` / `___`（允许中间有空格，长度 ≥ 3）。
fn is_rule(t: &str) -> bool {
    let s: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && matches!(s[0], '-' | '*' | '_') && s.iter().all(|&c| c == s[0])
}

fn heading_of(t: &str) -> Option<(u8, &str)> {
    let level = t.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    t[level..]
        .strip_prefix(' ')
        .map(|rest| (level as u8, rest.trim()))
}

/// 表格的对齐分隔行：`| --- | :---: |`（首尾竖线可省）。
fn is_delimiter_row(t: &str) -> bool {
    let cells = cells_of(t);
    cells.len() >= 2
        && cells.iter().all(|c| {
            let c = c.trim();
            !c.is_empty() && c.contains('-') && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

/// 表格的数据行：含 `|`，且不是分隔线 / 标题（首尾竖线同样可省）。
fn is_table_row(t: &str) -> bool {
    t.contains('|') && !is_rule(t) && heading_of(t).is_none()
}

fn aligns_of(t: &str) -> Vec<Align> {
    cells_of(t)
        .iter()
        .map(|c| {
            let c = c.trim();
            match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            }
        })
        .collect()
}

/// 表格一行的各格（已解析行内）。
fn inlines_of_row(t: &str) -> Vec<Vec<Inline>> {
    cells_of(t).iter().map(|c| parse_inlines(c)).collect()
}

/// 拆分表格行：去掉首尾竖线后按 `|` 切，逐格 trim。
///
/// **`|` 不是见到就切**。两种竖线必须放过，否则 AI 写的表格会被切碎：
/// - 反斜杠转义的 `\|`（GFM 明确支持把竖线写进单元格里）；
/// - 反引号代码段里的竖线 —— 清单里"字段用 `file|anchor|replacement` 分隔"这类
///   行几乎都这么写，按列切开就变成多出好几列、内容整体错位（`预估工时` 被甩到最后）。
///
/// 单元格原文**不剥反斜杠**，原样交给 [`parse_inlines`]：GFM 里反斜杠转义在代码段内
/// 不生效，这条规则该由行内解析器解释一次；拆列与行内各解释一遍，迟早会对不上。
fn cells_of(t: &str) -> Vec<String> {
    let t = t.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let cs: Vec<char> = t.chars().collect();
    let mut cells = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' if i + 1 < cs.len() => {
                // 转义对：两个字符一起进本格，`\` 留给行内解析器去解释。
                buf.push('\\');
                buf.push(cs[i + 1]);
                i += 2;
            }
            '`' => {
                let n = tick_run(&cs, i);
                match tick_close(&cs, i + n, n) {
                    // 有等长闭合 → 整段代码原样进本格（段内竖线不切）。
                    Some(end) => {
                        buf.extend(&cs[i..end + n]);
                        i = end + n;
                    }
                    // 没有等长闭合 → 这几个反引号只是普通字符（GFM：不成对就不是代码段）。
                    None => {
                        buf.push('`');
                        i += 1;
                    }
                }
            }
            '|' => {
                cells.push(buf.trim().to_string());
                buf.clear();
                i += 1;
            }
            c => {
                buf.push(c);
                i += 1;
            }
        }
    }
    cells.push(buf.trim().to_string());
    cells
}

/// 从 `i` 起连续反引号的个数。
fn tick_run(c: &[char], i: usize) -> usize {
    1 + c[i + 1..].iter().take_while(|x| **x == '`').count()
}

/// 从 `start` 起找**恰好 n 个**反引号的闭合段，返回其起始下标。
///
/// 长度不同的反引号串不算闭合（GFM 代码段按等长成对闭合）：`` `a``b` `` 里的
/// `` `` `` 不该结束单个反引号开头的代码段。
fn tick_close(c: &[char], start: usize, n: usize) -> Option<usize> {
    let mut i = start;
    while i < c.len() {
        if c[i] == '`' {
            let run = tick_run(c, i);
            if run == n {
                return Some(i);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// 渲染缓存：同一段、同一份原文只解析一次。
///
/// egui 是即时模式、每帧重画：不清缓存就得每帧重跑一遍解析（还要反复克隆原文）。
/// 键是 `(段下标, 原文)`——两者都相同才复用，正文一改立刻失效，不会渲染出上一份内容。
#[derive(Default)]
pub struct Cache(Option<(usize, String, Vec<Block>)>);

impl Cache {
    /// 取第 `step` 段的解析结果，必要时重新解析。
    pub fn get(&mut self, step: usize, text: &str) -> &[Block] {
        let fresh = self
            .0
            .as_ref()
            .is_some_and(|(s, src, _)| *s == step && src == text);
        if !fresh {
            self.0 = Some((step, text.to_string(), parse(text)));
        }
        &self.0.as_ref().expect("上一行刚写入").2
    }

    /// 作废（切换需求 / 重新读盘正文后调用）。
    pub fn clear(&mut self) {
        self.0 = None;
    }
}

// ===================== 渲染 =====================

/// 渲染块序列（垂直流式排布；纵向滚动交给外层 `ScrollArea`）。
///
/// **版心宽度是整篇正文唯一的宽度基准**：进来时 `ui.available_width()` 就是版心，
/// 往下每一层只能是"上层宽度减去一个固定量"（引用块的边框留白、列表符号的缩进），
/// 不能有哪一层自己另问一个宽度——那样同段里就会出现"这段行数多、那段行数少"。
/// 两处最容易破这条的地方：列表（一项一行，见 [`list_block`]）与表格
/// （列宽由版心分配，见 [`table`]）。
pub fn show(ui: &mut egui::Ui, blocks: &[Block]) {
    // 正文允许框选复制：审核人常要摘一段回评论里，渲染不能把这条路堵死。
    ui.style_mut().interaction.selectable_labels = true;
    for (i, b) in blocks.iter().enumerate() {
        // 每块一个 id 作用域：同段里出现多个表格 / 勾选框也不会互相串 id。
        ui.push_id(i, |ui| {
            if i > 0 {
                ui.add_space(6.0);
            }
            block(ui, b);
        });
    }
}

fn block(ui: &mut egui::Ui, b: &Block) {
    match b {
        Block::Heading { level, inlines } => {
            let size = match level {
                1 => 22.0,
                2 => 19.0,
                3 => 17.0,
                _ => 15.0,
            };
            ui.label(
                egui::RichText::new(inline_text(inlines))
                    .size(size)
                    .strong(),
            );
        }
        Block::Paragraph(inlines) => {
            if !inlines.is_empty() {
                inline_label(ui, inlines);
            }
        }
        Block::List(list) => list_block(ui, list),
        Block::Code { lang, text } => code_block(ui, lang, text),
        Block::Quote(inner) => {
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .corner_radius(4)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .show(ui, |ui| show(ui, inner));
        }
        Block::Table(t) => table(ui, t),
        Block::Rule => {
            ui.separator();
        }
    }
}

/// 列表：**一项一行**。
///
/// 用 `ui.horizontal_top` 而不是 `ui.horizontal_wrapped`：项内文字由内层 `ui.vertical`
/// 自己折行，外层并不需要 wrap 布局；而 wrap 布局会把"这一行还剩多少宽度"掺进折行与
/// 对齐（egui 的 `Label` 在 wrap 布局里还会额外按首行缩进重排）—— 万一两项被排到同一行，
/// 第二项的折行宽度就成了"版心 − 上一项宽度"，同一列表里每行字数不一样。
/// 规则写死成一行一项，这个自由度就不要了（宽度基准见 [`show`]）。
fn list_block(ui: &mut egui::Ui, list: &List) {
    for (i, item) in list.items.iter().enumerate() {
        ui.horizontal_top(|ui| {
            ui.add_space(LIST_INDENT_W);
            match item.checked {
                // 任务项渲染成灰色只读勾选框：`[x]` 已完成 / `[ ]` 未完成。
                // 只读是刻意的：正文由 AI / 编辑器维护，界面只做审核决策，不代改清单。
                Some(done) => {
                    let mut v = done;
                    ui.add_enabled(false, egui::Checkbox::new(&mut v, ""))
                        .on_hover_text(if done {
                            "已勾选（清单由 AI / 编辑器维护，界面只读）"
                        } else {
                            "未勾选（清单由 AI / 编辑器维护，界面只读）"
                        });
                }
                None => {
                    if list.ordered {
                        ui.label(format!("{}.", list.start + i as u64));
                    } else {
                        ui.label("•");
                    }
                }
            }
            ui.vertical(|ui| match item.blocks.as_slice() {
                // 紧凑列表：项内只有一段就直接跟在符号后面（否则会多出一大片空隙）。
                [Block::Paragraph(inlines)] => inline_label(ui, inlines),
                blocks => show(ui, blocks),
            });
        });
    }
}

fn code_block(ui: &mut egui::Ui, lang: &str, text: &str) {
    egui::Frame::new()
        .fill(ui.visuals().code_bg_color)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(4)
        .show(ui, |ui| {
            if !lang.is_empty() {
                ui.label(
                    egui::RichText::new(lang)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            // 横向滚动 + 不折行：折行会把代码的缩进结构搅乱（层级对不上）。
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.label(egui::RichText::new(text).monospace());
            });
        });
}

/// 表格：**先量后排**——每格先用 egui 的排版器出一份"不折行"的 `Galley` 量自然宽，
/// 把自然宽按比例摊到**版心宽度**上得到列宽（见 [`distribute`]），再把放不下的格子
/// 按列宽折行排第二遍，最后把每个 `Galley` 贴到该列的固定 `x` 上。
///
/// 这块踩过三个坑，都写在这儿免得再犯：
/// 1. `egui::Grid` 的 `num_columns` 会让**最后一列吃掉剩余宽度**——右对齐的「风险 / 工期」
///    被甩到面板最右，表头与数值之间一大段空白，看着像表格坏掉；
///    加一个空占位列能挡住，但那样又**没法给列设软上限**（设了之后每格矩形都被撑到上限宽，
///    右对齐又飘到上限外，等于换个坑再踩一遍）。
/// 2. 自己按"中文 1 字宽、ASCII 0.55 字宽"估列宽：估偏小文字就溢到隔壁列，
///    偏大右对齐就飘走；粗体表头、全角标点更是没法算准。
/// 3. `ui.allocate_ui_with_layout(vec2(w, 0), ..)` **不是固定宽**：
///    它给子 ui 的 `max_rect` 是 w，但父游标是按子 ui 的 `min_rect`（= 内容实际宽度）推进的，
///    于是每一行的列起点都跟着自己的内容跑——表头与数据反而错位（这正是"不像表格"的成因）。
///
/// 宽度这条单独记一笔（GUI 上"表格忽宽忽窄、每行字数忽多忽少"就是它）：
/// **表宽只能由版心宽度决定，不能由内容决定。** 早先列宽上限写死 300px、整表又按内容收缩，
/// 于是两列短表只有 200px 宽、三列长表顶到面板边缘还带横向滚动条——同一篇正文里表格
/// 一会儿贴左、一会儿满宽，单元格每行能显示的字数也跟着变。现在列宽 = 自然宽按比例摊到
/// 版心宽度上：表与正文同宽、右边缘对齐，放不下时才压缩（压缩优先短列，长列折行）。
///
/// 代价：单元格文字是直接画的，**不能框选复制**——要整段复制切顶栏「原文」视图即可。
fn table(ui: &mut egui::Ui, t: &Table) {
    let cols = t
        .header
        .len()
        .max(t.rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }
    let style = ui.style().clone();

    // ---- 1) 逐格按"不折行"排一遍，量出各列自然宽 ----
    // `Painter::layout_job` 走 egui 的 galley 缓存，同样的 job 每帧只真排一次，代价可忽略。
    let head_nat: Vec<Arc<egui::Galley>> = (0..cols)
        .map(|i| cell_galley(ui, t.header.get(i), true, &style, f32::INFINITY))
        .collect();
    let rows_nat: Vec<Vec<Arc<egui::Galley>>> = t
        .rows
        .iter()
        .map(|r| {
            (0..cols)
                .map(|i| cell_galley(ui, r.get(i), false, &style, f32::INFINITY))
                .collect()
        })
        .collect();

    // ---- 2) 列宽 = 自然宽按比例摊到版心宽度上 ----
    let spacing = ui.spacing().item_spacing.x;
    let gaps = spacing * cols.saturating_sub(1) as f32;
    // 表头是粗体、必须按粗体量（细体量出来的宽度偏小，标题会被挤出列外）。
    let natural: Vec<f32> = (0..cols)
        .map(|i| {
            std::iter::once(&head_nat[i])
                .chain(rows_nat.iter().map(|r| &r[i]))
                .map(|g| g.size().x)
                .fold(0.0f32, f32::max)
                + CELL_PAD_X * 2.0
        })
        .collect();
    let widths = distribute(
        &natural,
        (ui.available_width() - gaps).max(MIN_COL_W),
        MIN_COL_W,
    );
    let total_w: f32 = widths.iter().sum::<f32>() + gaps;

    // ---- 3) 定稿：列宽放得下的格子沿用自然宽那份（不白排两遍），放不下的按列宽折行 ----
    let header_cells: Vec<Cell> = (0..cols)
        .map(|i| {
            fit_cell(&head_nat[i], widths[i], |w| {
                cell_galley(ui, t.header.get(i), true, &style, w)
            })
        })
        .collect();
    let data_rows: Vec<Vec<Cell>> = t
        .rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            (0..cols)
                .map(|i| {
                    fit_cell(&rows_nat[r][i], widths[i], |w| {
                        cell_galley(ui, row.get(i), false, &style, w)
                    })
                })
                .collect()
        })
        .collect();

    // ---- 4) 画：每行先按精确宽度占位，再把各格贴到本列的对齐位置 ----
    egui::ScrollArea::horizontal()
        .id_salt(ui.id().with("md_table_scroll"))
        .auto_shrink([true, false])
        .show(ui, |ui| {
            ui.set_min_width(total_w);
            let rows =
                std::iter::once((true, &header_cells)).chain(data_rows.iter().map(|r| (false, r)));
            for (row_no, (header, cells)) in rows.enumerate() {
                let row_h = cells
                    .iter()
                    .map(|c| c.galley().size().y)
                    .fold(0.0f32, f32::max)
                    + CELL_PAD_Y * 2.0;
                let (_id, rect) = ui.allocate_space(egui::vec2(total_w, row_h));
                paint_row_bg(ui, rect, header, row_no);
                let mut x = rect.min.x;
                for (i, c) in cells.iter().enumerate() {
                    let w = widths[i];
                    let g = c.galley();
                    let gw = g.size().x;
                    let gx = match align_of(t, i) {
                        Align::Left => x + CELL_PAD_X,
                        Align::Center => x + (w - gw) / 2.0,
                        Align::Right => x + w - gw - CELL_PAD_X,
                    };
                    let gy = rect.min.y + (row_h - g.size().y) / 2.0;
                    ui.painter()
                        .galley(egui::pos2(gx, gy), g.clone(), ui.visuals().text_color());
                    x += w + spacing;
                }
            }
        });
}

/// 单元格最终用哪份排版：列宽够宽就用不折行那份，放不下才折行。
enum Cell {
    /// 不折行的排版（列宽放得下时直接用它，省一次重排）。
    Plain(Arc<egui::Galley>),
    /// 按列宽折行后的排版。
    Wrapped(Arc<egui::Galley>),
}

impl Cell {
    fn galley(&self) -> &Arc<egui::Galley> {
        match self {
            Cell::Plain(g) | Cell::Wrapped(g) => g,
        }
    }
}

fn fit_cell(
    natural: &Arc<egui::Galley>,
    col_w: f32,
    layout: impl FnOnce(f32) -> Arc<egui::Galley>,
) -> Cell {
    let w = col_w - CELL_PAD_X * 2.0;
    if natural.size().x <= w {
        Cell::Plain(natural.clone())
    } else {
        Cell::Wrapped(layout(w))
    }
}

/// 把各列自然宽按比例摊成 `target` 总宽（减去列间距后的可用宽度）。
///
/// - 自然宽合计 ≤ `target`（表比版心窄）：按比例放大到**刚好填满版心**。
///   表与正文同宽、右边缘对齐，视线不会因为"这张表只占半屏"而横向跳。
/// - 合计 > `target`（表比版心宽）：按比例压缩，压到 `min_w` 的列就钉在下限，
///   剩下的预算在还能压的列里再按比例分——**短列先保住完整内容，长列去折行**。
/// - 连下限都放不下（列多且面板窄）：每列都是 `min_w`，此时表比面板还宽，
///   交给横向滚动条，别把字挤成一列竖条。
fn distribute(natural: &[f32], target: f32, min_w: f32) -> Vec<f32> {
    if natural.is_empty() {
        return Vec::new();
    }
    let sum: f32 = natural.iter().sum();
    if sum <= target {
        let k = if sum > 0.0 { target / sum } else { 0.0 };
        return natural.iter().map(|w| w * k).collect();
    }
    // 压缩：每轮都从**原始自然宽**重新按比例缩放，被压到下限的列退出，预算重分。
    let mut out = natural.to_vec();
    let mut active: Vec<usize> = (0..natural.len()).collect();
    let mut budget = target;
    loop {
        let total: f32 = active.iter().map(|&i| natural[i]).sum();
        if total <= f32::EPSILON {
            break;
        }
        for &i in &active {
            out[i] = natural[i] * budget / total;
        }
        let clamped: Vec<usize> = active.iter().copied().filter(|&i| out[i] < min_w).collect();
        if clamped.is_empty() {
            break;
        }
        for &i in &clamped {
            out[i] = min_w;
        }
        active.retain(|i| !clamped.contains(i));
        budget -= min_w * clamped.len() as f32;
        if budget <= 0.0 {
            // 剩下的列连下限都分不到：全部按下限收尾（总宽超了版心 → 交给横向滚动）。
            for &i in &active {
                out[i] = min_w;
            }
            break;
        }
    }
    out
}

/// 单元格排版：`wrap_w` 为 `f32::INFINITY` 表示不折行（量自然宽时用）。
fn cell_galley(
    ui: &egui::Ui,
    content: Option<&Vec<Inline>>,
    header: bool,
    style: &egui::Style,
    wrap_w: f32,
) -> Arc<egui::Galley> {
    ui.painter().layout_job(inline_job(
        ui,
        content.map(Vec::as_slice).unwrap_or_default(),
        wrap_w,
        style,
        |rt| if header { rt.strong() } else { rt },
    ))
}

/// 底色分带：表头一条实底、数据行隔行浅底（不画横线，靠色带分，读起来更像表）。
fn paint_row_bg(ui: &egui::Ui, rect: egui::Rect, header: bool, row_no: usize) {
    let color = if header {
        ui.visuals().widgets.noninteractive.weak_bg_fill
    } else if row_no.is_multiple_of(2) {
        ui.visuals().faint_bg_color
    } else {
        return;
    };
    ui.painter().rect_filled(rect, 0.0, color);
}

/// 列宽下限：至少放得下一小段文字加两侧留白（空列也点得着，宽表不至于挤成竖条）。
const MIN_COL_W: f32 = 56.0;
/// 单元格留白：文字不贴着隔壁列。
const CELL_PAD_X: f32 = 8.0;
const CELL_PAD_Y: f32 = 3.0;
/// 列表符号前的缩进（有序无序同宽，两种列表看起来才是一套的）。
const LIST_INDENT_W: f32 = 16.0;

fn align_of(t: &Table, i: usize) -> Align {
    t.aligns.get(i).copied().unwrap_or(Align::Left)
}

/// 行内 → 一个可折行的 `LayoutJob`（段落与列表项用这条路径）。
///
/// egui 0.36 的 `RichText` 只能带**一种**样式，不再支持旧版的 `push` 拼多段；
/// 段内混排（`**粗**`、`` `码` ``）必须靠 `append_to` 累加进同一个 job，
/// 否则一行里每个片段各占一个 label，折行位置与基线都会散掉。
///
/// `wrap_w` 是折行宽度：表格单元格给列宽，标签给 `available_width()`。
/// 注意 `ui.label` 会用自己的 `available_width()` 覆盖 job 里的折行宽度
/// （egui `Label::layout_in_ui`），所以标签这条路只能靠"块内宽度本身一致"来保证版心统一。
fn inline_job(
    ui: &egui::Ui,
    v: &[Inline],
    wrap_w: f32,
    style: &egui::Style,
    deco: impl Fn(egui::RichText) -> egui::RichText,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: wrap_w,
            ..Default::default()
        },
        ..Default::default()
    };
    for i in v {
        deco(rt_of(ui, i)).append_to(
            &mut job,
            style,
            egui::FontSelection::Default,
            egui::Align::Center,
        );
    }
    job
}

/// 段落 / 列表项：按当前可用宽度折行（可用宽度由块内布局保证一致，见 [`show`]）。
fn inline_label(ui: &mut egui::Ui, v: &[Inline]) {
    let style = ui.style().clone();
    let w = ui.available_width();
    ui.label(inline_job(ui, v, w, &style, |rt| rt));
}

/// 单个行内元素 → `RichText`；`Link` / `Image` 一律降级成**不可点**的纯文本（见模块注释）。
fn rt_of(ui: &egui::Ui, i: &Inline) -> egui::RichText {
    match i {
        Inline::Text(t) => egui::RichText::new(t.clone()),
        Inline::Code(t) => egui::RichText::new(t.clone()).code(),
        Inline::Strong(c) => joined(c).strong(),
        Inline::Emphasis(c) => joined(c).italics(),
        Inline::Strikethrough(c) => joined(c).strikethrough(),
        // 链接：加下划线让人一眼看出"这里原本是链接"，但**没有点击行为**。
        Inline::Link { text, .. } => joined(text).color(palette::link(ui)).underline(),
        Inline::Image { alt, .. } => egui::RichText::new(format!("\u{1F5BC} {}", alt)).italics(),
    }
}

fn joined(v: &[Inline]) -> egui::RichText {
    egui::RichText::new(inline_text(v))
}

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言"渲染后仍看得到这段文字"：审核工具宁可丑，不可丢内容。
    fn assert_visible(blocks: &[Block], needle: &str) {
        let text = blocks.iter().map(block_text).collect::<Vec<_>>().join("\n");
        assert!(
            text.contains(needle),
            "渲染文本里找不到 {:?}：\n{}",
            needle,
            text
        );
    }

    fn block_text(b: &Block) -> String {
        match b {
            Block::Heading { inlines, .. } | Block::Paragraph(inlines) => inline_text(inlines),
            Block::Code { text, .. } => text.clone(),
            Block::Rule => "---".into(),
            Block::Quote(bs) => bs_text(bs),
            Block::List(l) => l
                .items
                .iter()
                .map(|it| bs_text(&it.blocks))
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Table(t) => {
                let head = t
                    .header
                    .iter()
                    .map(|c| inline_text(c))
                    .collect::<Vec<_>>()
                    .join(" | ");
                let rows = t
                    .rows
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|c| inline_text(c))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{}\n{}", head, rows)
            }
        }
    }

    fn bs_text(blocks: &[Block]) -> String {
        blocks.iter().map(block_text).collect::<Vec<_>>().join("\n")
    }

    /// 离屏跑一帧的上下文，**必须**带内嵌字体。
    ///
    /// `Context::default()` 里没有可用的中文字形，长出来的 galley 尺寸全是 0，
    /// 任何几何断言都会拿到 NaN —— 也就是说没装字体的离屏测试只能验"有没有画东西"，
    /// 验不了"画在哪、画多宽"。窗口启动用的是同一份定义（[`crate::fonts`]）。
    fn ctx_with_fonts() -> eframe::egui::Context {
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(crate::fonts::definitions());
        ctx
    }

    /// 离屏跑帧的输入：**给一个正常大小的窗口**。
    ///
    /// `RawInput::default()` 的屏幕是两万点宽的"无限大"画布，任何宽度断言都会因为
    /// "怎么都不换行"而失去意义（表宽、折行宽度看起来都对，其实没被考验）。
    /// 800×600 与窗口默认的 1000×680 同量级。
    fn raw() -> eframe::egui::RawInput {
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::pos2(0.0, 0.0),
                eframe::egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn 任务列表解析出勾选态() {
        let b = parse("- [x] 已完成\n- [ ] 未完成\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表：{:?}", b[0])
        };
        assert_eq!(l.items.len(), 2);
        assert_eq!(l.items[0].checked, Some(true));
        assert_eq!(l.items[1].checked, Some(false));
        assert_visible(&b, "未完成");
    }

    #[test]
    fn 有序列表保留起始编号() {
        let b = parse("3. 三\n4. 四\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表")
        };
        assert!(l.ordered && l.start == 3);
        assert_visible(&b, "四");
    }

    #[test]
    fn 标题层级与行内样式() {
        let b = parse("### 小节 **粗** `码`\n");
        let Block::Heading { level, inlines } = &b[0] else {
            panic!("应为标题")
        };
        assert_eq!(*level, 3);
        assert!(matches!(inlines[1], Inline::Strong(_)));
        assert!(matches!(inlines[3], Inline::Code(_)));
    }

    #[test]
    fn 围栏代码块不解析内部标记() {
        let b = parse("```rust\nlet a = **1**;\n```\n");
        let Block::Code { lang, text } = &b[0] else {
            panic!("应为代码块")
        };
        assert_eq!(lang, "rust");
        assert_eq!(text, "let a = **1**;\n");
    }

    #[test]
    fn 表格解析出对齐() {
        let b = parse("| 项 | 值 |\n| :--- | ---: |\n| a | 1 |\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格")
        };
        assert_eq!(t.aligns, vec![Align::Left, Align::Right]);
        assert_eq!(t.rows.len(), 1);
        assert_visible(&b, "a");
    }

    #[test]
    fn 字体缺字形的符号降级为等义字符() {
        // 内嵌字体没有 ✅/❌ 的字形，不降级就显示成 `?`。
        assert_eq!(
            inline_text(&parse_inlines("✅ 通过　❌ 未做")),
            "✓ 通过　✗ 未做"
        );
        assert_eq!(inline_text(&parse_inlines("⚠️ 注意")), "⚠ 注意");
        assert_eq!(inline_text(&parse_inlines("⭐ 重要")), "★ 重要");
        // 没有被降级的字符原样保留。
        assert_eq!(inline_text(&parse_inlines("正常 ✓ 字符")), "正常 ✓ 字符");
    }

    #[test]
    fn 表格首尾竖线可省() {
        let b = parse("项 | 值\n--- | ---\na | 1\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.rows.len(), 1);
        assert_visible(&b, "a");
    }

    #[test]
    fn 表格单元格内的竖线不切列() {
        // 真实缺陷：清单里"字段用 `file|anchor|replacement` 分隔"这类行，
        // 按 `|` 硬切就变成多出好几列、内容整体错位（列数还会被数据行顶大）。
        let b = parse(
            "| 编号 | 子任务 | 预估工时 |\n| --- | --- | --- |\n\
             | T1 | 变异清单文件格式（`file|anchor|replacement|test-filter|理由`） | 2h |\n",
        );
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.header.len(), 3);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].len(), 3);
        // `inline_text` 是结构文本化（代码段不加反引号），列内容一格都没丢。
        assert_eq!(inline_text(&t.rows[0][0]), "T1");
        assert_eq!(
            inline_text(&t.rows[0][1]),
            "变异清单文件格式（file|anchor|replacement|test-filter|理由）"
        );
        assert_eq!(inline_text(&t.rows[0][2]), "2h");
    }

    #[test]
    fn 表格里转义的竖线也不切列() {
        // GFM：`\|` 与代码段内的竖线同义，都是"内容里的竖线"。
        let cells = cells_of(r"| a\|b | `c|d` | e |");
        assert_eq!(cells, vec!["a\\|b", "`c|d`", "e"]);
        // 反斜杠由行内解析器解释（GFM 里代码段内不解释转义）。
        assert_eq!(inline_text(&parse_inlines(&cells[0])), "a|b");
        assert_eq!(inline_text(&parse_inlines(&cells[1])), "c|d");
    }

    #[test]
    fn 未闭合的反引号仍按列分隔() {
        // 不成对的反引号不是代码段（GFM），否则整行会被并成一格、后面几列全丢。
        assert_eq!(cells_of("| a | b`c | d |"), vec!["a", "b`c", "d"]);
    }

    #[test]
    fn 列宽按自然宽比例摊到版心宽度() {
        // 比版心窄 → 放大到刚好填满（表与正文同宽、右边缘对齐）。
        assert_eq!(distribute(&[100.0, 200.0], 600.0, 56.0), vec![200.0, 400.0]);
        // 比版心宽 → 压缩，短列钉在下限保内容，剩下的宽度长列自己承担（长列去折行）。
        let w = distribute(&[1000.0, 100.0], 200.0, 56.0);
        assert_eq!(w, vec![144.0, 56.0], "{w:?}");
        // 等比压缩时不超版心、也不低于下限。
        let w = distribute(&[400.0, 400.0], 600.0, 56.0);
        assert!((w.iter().sum::<f32>() - 600.0).abs() < 0.5, "{w:?}");
        assert!(w.iter().all(|x| *x >= 56.0), "{w:?}");
        // 连下限都放不下 → 每列都是下限（表比面板还宽，交给横向滚动）。
        assert_eq!(
            distribute(&[300.0, 300.0, 300.0], 100.0, 56.0),
            vec![56.0, 56.0, 56.0]
        );
        assert!(distribute(&[], 100.0, 56.0).is_empty());
    }

    #[test]
    fn 表格宽度等于版心而不是等于内容() {
        // 两列短表曾经只有百来 px 宽（列宽上限 300px + 整表按内容收缩），
        // 与同页段落不齐；表头底色的宽度就是整表宽度，拿它跟版心比最直接。
        let ctx = ctx_with_fonts();
        let blocks = parse("一段正文。\n\n| 项 | 值 |\n| --- | --- |\n| a | 1 |\n");
        let measure = std::cell::Cell::new(0.0f32);
        let mut out = ctx.run_ui(raw(), |ui| {
            measure.set(ui.available_width());
            show(ui, &blocks);
        });
        out.textures_delta.clear();
        let table_w = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Rect(r) => Some(r.rect.width()),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(
            (table_w - measure.get()).abs() < 1.0,
            "表宽 {table_w} 应等于版心宽 {}（表与正文同宽）",
            measure.get()
        );
        // 段落与表格是**同一个**版心：正文的折行宽度也就是版心宽度。
        let para_wrap = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("一段正文") => {
                    Some(t.galley.job.wrap.max_width)
                }
                _ => None,
            })
            .expect("应画出段落文字");
        assert!(
            (para_wrap - measure.get()).abs() < 1.0,
            "段落折行宽度 {para_wrap} 应等于版心宽 {}",
            measure.get()
        );
        // 单元格文字不得越出版心右边缘（列宽分配错了就会越界）。
        let right = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some(t.visual_bounding_rect().max.x),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(
            right <= measure.get() + 1.0,
            "文字右边缘 {right} 不应越出版心宽 {}",
            measure.get()
        );
    }

    #[test]
    fn 列表一项占一行且折行宽度一致() {
        // 守住的不变量（不是复现某个旧缺陷）：两项必须在两行上，且折行宽度完全一致。
        // 若列表改回 `horizontal_wrapped`，两项一旦排到同一行，第二项的折行宽度就是
        // "版心 − 上一项宽度"，这条断言会先炸。
        let ctx = ctx_with_fonts();
        let blocks = parse("- 甲\n- 乙\n");
        let mut out = ctx.run_ui(raw(), |ui| show(ui, &blocks));
        out.textures_delta.clear();
        let rows: Vec<(String, f32, f32)> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.visual_bounding_rect().min.y,
                    t.galley.job.wrap.max_width,
                )),
                _ => None,
            })
            .collect();
        let item = |name: &str| {
            rows.iter()
                .find(|(t, _, _)| t.trim() == name)
                .unwrap_or_else(|| panic!("没画出行内文本 {name}：{rows:?}"))
        };
        let (y_a, wrap_a) = {
            let (_, y, w) = item("甲");
            (y, w)
        };
        let (y_b, wrap_b) = {
            let (_, y, w) = item("乙");
            (y, w)
        };
        assert!(
            (y_a - y_b).abs() >= 8.0,
            "两项应在两行上（y {y_a} vs {y_b}）：{rows:?}"
        );
        assert!(
            (wrap_a - wrap_b).abs() < 0.5,
            "两项的折行宽度应一致：{wrap_a} vs {wrap_b}"
        );
    }

    #[test]
    fn 门禁标记注释整块跳过() {
        let b = parse("## 1. 需求分解\n\n<!-- GATE:STEP name=decomposition -->\n\n- [ ] 背景\n");
        assert!(!bs_text(&b).contains("GATE"), "GATE 标记行不该出现在正文里");
        assert!(matches!(b[0], Block::Heading { .. }));
        assert!(matches!(b[1], Block::List(_)));
    }

    #[test]
    fn 链接与图片只降级为文本不丢内容() {
        let v = parse_inlines("见 [文档](https://a.tld/x) 与 ![图](a.png)");
        let Inline::Link { dest, .. } = &v[1] else {
            panic!("应为链接：{:?}", v[1])
        };
        assert_eq!(dest, "https://a.tld/x");
        assert!(v.iter().any(|i| matches!(i, Inline::Image { .. })));
        let t = inline_text(&v);
        assert!(t.contains("文档") && t.contains("图"));
    }

    #[test]
    fn 未闭合语法按原文保留() {
        let v = parse_inlines("未闭合 **粗 与 `码");
        assert_eq!(inline_text(&v), "未闭合 **粗 与 `码");
    }

    #[test]
    fn 标识符里的下划线不被当成斜体() {
        assert_eq!(
            inline_text(&parse_inlines("see req_guard_hook.sh")),
            "see req_guard_hook.sh"
        );
    }

    #[test]
    fn 引用块内可嵌套列表() {
        let b = parse("> 提示：\n> - 一\n> - 二\n");
        let Block::Quote(inner) = &b[0] else {
            panic!("应为引用")
        };
        assert!(inner.iter().any(|x| matches!(x, Block::List(_))));
        assert_visible(&b, "二");
    }

    #[test]
    fn 嵌套列表并入父项() {
        let b = parse("- 外\n  - 内\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表")
        };
        assert_eq!(l.items.len(), 1);
        assert!(l.items[0]
            .blocks
            .iter()
            .any(|x| matches!(x, Block::List(_))));
        assert_visible(&b, "内");
    }

    #[test]
    fn 顶格续行并入所属列表项() {
        let b = parse("- 第一项\n  续行说明\n- 第二项\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表")
        };
        assert_eq!(l.items.len(), 2);
        assert_visible(&b, "续行说明");
    }

    #[test]
    fn 缓存按段与原文取值_改文即换内容() {
        let mut c = Cache::default();
        assert!(matches!(c.get(0, "## a")[0], Block::Heading { .. }));
        assert!(matches!(c.get(0, "- [ ] 一")[0], Block::List(_)));
        // 同键反复取：内容必须一致（缓存命中与重新解析等价，用户看不出差别）。
        let again = c.get(0, "- [ ] 一").to_vec();
        assert_eq!(&again, c.get(0, "- [ ] 一"));
        // 正文改了 → 立刻换成新内容（否则审核人看的是上一版清单）。
        assert!(matches!(&c.get(0, "- [ ] 二")[0], Block::List(l)
            if l.items[0].blocks[0] == Block::Paragraph(parse_inlines("二"))));
        // 段下标变了 → 取的是新一段的内容（不是上一段的缓存）。
        assert!(matches!(c.get(1, "## b")[0], Block::Heading { .. }));
        c.clear();
        assert!(matches!(c.get(1, "## b")[0], Block::Heading { .. }));
    }

    #[test]
    fn 渲染路径不panic且真的画出了东西() {
        // egui 可以脱离窗口跑一帧：用来守住"渲染代码在真实 egui 调用下不炸"这条底线。
        let src = "## 1. 需求分解\n\n> 提示 **加粗** 与 `代码`\n\n\
                   - [x] 已完成\n- [ ] 未完成\n  - 嵌套项\n\n1. 第一\n2. 第二\n\n\
                   | 项 | 值 |\n| :--- | ---: |\n| 工时 | 2d |\n\
                   | 超长单元格 | 这一格很长很长很长很长很长很长很长很长很长很长很长很长，用来逼出折行路径 |\n\n\
                   ```bash\nreq-guard approve REQ-001\n```\n\n---\n\n\
                   见 [文档](https://a.tld) 与 ~~废弃~~ 条目\n";
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui, &parse(src)));
        // 离屏跑帧没有渲染器去消费纹理增量，必须显式 clear，否则 epaint 在 Drop 时 panic。
        out.textures_delta.clear();
        // 有形状输出 = 确实排出了可见内容（一条 shape 都没画说明根本没跑起来）。
        assert!(
            !out.shapes.is_empty(),
            "渲染后应有绘制输出，说明正文真的画出来了"
        );
    }

    #[test]
    fn 模板首段能解析出全部要素() {
        let src = "## 1. 需求分解\n\n> **AI 需求门禁清单**：三段全通过才允许编码。\n\n\
                   - [ ] 背景与问题：为什么要做\n- [x] 已确认范围\n\n\
                   | 项 | 值 |\n| --- | --- |\n| 工时 | 2d |\n\n\
                   ```bash\nreq-guard approve REQ-001\n```\n\n---\n";
        let blocks = parse(src);
        assert!(matches!(blocks[0], Block::Heading { level: 2, .. }));
        assert!(matches!(blocks[1], Block::Quote(_)));
        assert!(matches!(blocks[2], Block::List(_)));
        assert!(matches!(blocks[3], Block::Table(_)));
        assert!(matches!(blocks[4], Block::Code { .. }));
        assert!(matches!(blocks[5], Block::Rule));
        assert_visible(&blocks, "req-guard approve REQ-001");
    }
}

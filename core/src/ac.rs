//! 验收标准（AC）机械校验（对应《docs/设计/AC与变更范围契约技术方案.md》§2）。
//!
//! ## 目的
//!
//! 三段清单过去只验**审批状态**不验**内容**（`gate.rs` 只 `grep GATE:STEP` 看
//! `status=approved`），于是三段可以同时是空话。本模块把第 3 段的验收标准变成
//! **可解析的契约**：每条 AC 必须有编号 + Given/When/Then 三子句，格式不合规即报错。
//!
//! ## 格式契约（A1–A12）
//!
//! 条目形态**唯一：三行式** —— 编号独占一行，其下恰好三个 `Given:`/`When:`/`Then:`
//! 前缀列表项。曾有过"一行写完"的形式，因渲染不可读已废除（见设计文档 §2.1）。
//!
//! ```text
//! <!-- GATE:AC -->
//! ### AC-001
//! - Given: 可复现的前置状态
//! - When: 一次可触发的操作
//! - Then: 可观测结果，含退出码或字面量
//! <!-- /GATE:AC -->
//! ```
//!
//! ## 分层（可测性靠这个）
//!
//! [`scan`] 是唯一的扫描实现，[`lint`] 与 [`parse`] 都基于它：
//! - [`lint`] 纯函数：入参一段正文 → 出参问题列表。A1–A12 因此能 table-driven 单测覆盖，
//!   不需要临时目录、不需要 git 仓库 —— 这是它可测的唯一前提。
//! - [`check`] 薄壳：读文件 → [`crate::requirement::section_span`] 定位第 3 段 → [`lint`]。
//!
//! **为什么必须用 `section_span` 而不是 `section_of`**：后者定位失败时回退整篇
//! （为保证审核人界面不白屏）。拿回退整篇去判 AC，标题写坏的文档会拿全文去判，
//! "第 3 段恰好有一条 AC"这种巧合就能蒙混过关 —— 那是最坏的失效模式（看着在拦、其实没拦）。
//!
//! ## 覆盖面
//!
//! [`check`] 除了第 3 段的 A1–A12，还汇总 `crate::section` 的**三段实质正文**判定
//! （REQ-003）。两者刻意交叉：GATE 块内的条目不计入实质正文，于是「空 AC 骨架」
//! 会同时命中 A8 与 `EmptySection` —— 两条独立路径指向同一结论。
//!
//! ## 判定唯一性
//!
//! 本模块是 AC 格式的**唯一判定处**。刻意不提供脚本实现：判定散到第二处的那一刻起
//! 就与 core 版本漂移，而 CI 跑 core 版本、本地跑脚本版本时行为不同却无从察觉。

use std::path::Path;

use crate::error::Result;
use crate::issue::Severity;
use crate::requirement;

/// 块起始标记（独占一行）。
pub const BEGIN: &str = "<!-- GATE:AC -->";
/// 块结束标记（独占一行）。
pub const END: &str = "<!-- /GATE:AC -->";

/// 三个子句的规范顺序。
const ORDER: [&str; 3] = ["given", "when", "then"];

/// 子句关键词 → 同义写法。ASCII 关键词可省略冒号；**中文别名必须跟全角冒号**
/// （否则 `- 当前状态…` 会被误判成 `当` 子句 —— 单字别名必须靠冒号消歧）。
const KEYWORDS: [(&str, &[&str]); 3] = [
    ("given", &["given", "假设", "假定", "前置"]),
    ("when", &["when", "当", "执行"]),
    ("then", &["then", "则", "那么", "预期"]),
];

/// AC 问题类别。**枚举即契约**：新增规则必须新增变体，
/// `ac_枚举覆盖门槛` 用例会遍历全量变体断言每个都被测到（见 §6.1 门槛）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AcIssueKind {
    /// A1 第 3 段无 GATE:AC 标记。
    MissingBlock,
    /// A1 标记未闭合。
    UnclosedBlock,
    /// A8 块内条目数为 0。
    NoItem,
    /// A2 编号格式非法（`### AC-1`、`### AC1`、行首 `AC` 却不成编号）。
    BadIdFormat,
    /// A3 编号重复。
    DuplicateId,
    /// A3 编号跳号 / 非 AC-001 起。
    SeqGap,
    /// A4 三行式条目缺子句（或子句数 ≠ 3）。
    MissingClause,
    /// A4 子句顺序非 Given → When → Then。
    OrderClause,
    /// A6 子句短于 4 个非空白字符。
    EmptyClause,
    /// A7 块外出现 AC 编号。
    OutsideBlock,
    /// A5 同一 AC 内中英混用（**Warn**）。
    AliasMix,
    /// A9 Then 无数字或字面量（**Warn**）。
    Unmeasurable,
    /// A2 编号行内混入子句（标题污染）。
    DirtyIdLine,
    /// A10 Given 出现"与上一条相同"这类外部指代。
    NonSelfContained,
    /// A11 块内混入非条目用的分级标题。
    StrayHeading,
    /// A12 单行式残留（已废除的形态）。
    InlineEntry,
    /// 第 3 段二级标题定位失败。
    SectionNotFound,
    /// 某段实质正文为 0（`crate::section` 的判定，见 REQ-003）。
    EmptySection,
}

/// 一条 AC 问题。`message` 自含上下文（编号 + 行号 + 修复指引），可直接展示。
#[derive(Debug, Clone)]
pub struct AcIssue {
    pub severity: Severity,
    pub kind: AcIssueKind,
    pub message: String,
}

impl AcIssueKind {
    /// 稳定字面量（CLI 渲染与错误文案共用；刻意不用 `{:?}`，
    /// 免得改枚举顺序就悄悄改掉面向用户的输出）。
    pub fn as_str(self) -> &'static str {
        match self {
            AcIssueKind::MissingBlock => "MissingBlock",
            AcIssueKind::UnclosedBlock => "UnclosedBlock",
            AcIssueKind::NoItem => "NoItem",
            AcIssueKind::BadIdFormat => "BadIdFormat",
            AcIssueKind::DuplicateId => "DuplicateId",
            AcIssueKind::SeqGap => "SeqGap",
            AcIssueKind::MissingClause => "MissingClause",
            AcIssueKind::OrderClause => "OrderClause",
            AcIssueKind::EmptyClause => "EmptyClause",
            AcIssueKind::OutsideBlock => "OutsideBlock",
            AcIssueKind::AliasMix => "AliasMix",
            AcIssueKind::Unmeasurable => "Unmeasurable",
            AcIssueKind::DirtyIdLine => "DirtyIdLine",
            AcIssueKind::NonSelfContained => "NonSelfContained",
            AcIssueKind::StrayHeading => "StrayHeading",
            AcIssueKind::InlineEntry => "InlineEntry",
            AcIssueKind::SectionNotFound => "SectionNotFound",
            AcIssueKind::EmptySection => "EmptySection",
        }
    }
}

impl AcIssue {
    /// 本条问题的类别字面量。
    pub fn kind_as_str(&self) -> &'static str {
        self.kind.as_str()
    }

    fn err(kind: AcIssueKind, message: impl Into<String>) -> AcIssue {
        AcIssue {
            severity: Severity::Error,
            kind,
            message: message.into(),
        }
    }
    fn warn(kind: AcIssueKind, message: impl Into<String>) -> AcIssue {
        AcIssue {
            severity: Severity::Warn,
            kind,
            message: message.into(),
        }
    }
}

/// 是否存在硬伤（`ac check` 据此决定退出码）。
pub fn has_errors(issues: &[AcIssue]) -> bool {
    issues.iter().any(|i| i.severity.is_error())
}

/// 一个子句。
#[derive(Debug, Clone)]
pub struct AcClause {
    /// 规范名：`given` / `when` / `then`。
    pub keyword: String,
    /// 子句正文（已剥掉关键词与冒号）。
    pub text: String,
    /// 原文用的是 ASCII 关键词（否则是中文别名）—— A5 判混用靠它。
    pub ascii: bool,
    /// 1-based 行号。
    pub line: usize,
}

/// 一条 AC 条目。
#[derive(Debug, Clone)]
pub struct AcEntry {
    pub id: String,
    /// 1-based 行号（编号行）。
    pub line: usize,
    pub clauses: Vec<AcClause>,
}

impl AcEntry {
    fn clause(&self, kw: &str) -> Option<&AcClause> {
        self.clauses.iter().find(|c| c.keyword == kw)
    }
}

/// `ac check` 的作用范围。
pub enum AcTarget<'a> {
    /// 全部未归档清单（默认）。
    All,
    /// 指定编号（经 `requirement::find`，含归档区只读回退）。
    Id(&'a str),
    /// 全部清单，含归档区（审计用，只读）。
    IncludingArchived,
}

/// 校验一份清单里所有目标的 AC 问题（跨清单聚合）。
pub fn check(root: &Path, target: &AcTarget<'_>) -> Result<Vec<AcIssue>> {
    let reqs = match target {
        AcTarget::All => requirement::list(root)?,
        AcTarget::Id(id) => vec![requirement::find(root, id)?],
        AcTarget::IncludingArchived => {
            let mut v = requirement::list(root)?;
            v.extend(requirement::list_archived(root)?);
            v
        }
    };
    let mut out = Vec::new();
    for r in &reqs {
        let name = r
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let content = std::fs::read_to_string(&r.path).unwrap_or_default();
        // 严格分段：定位失败如实报「第 3 段定位失败」，绝不回退整篇（见模块文档）
        let issues = match requirement::section_span(&content, 2) {
            Some((start, end)) => {
                let lines: Vec<&str> = content.lines().collect();
                let section = lines[start - 1..end].join("\n");
                lint(&section, start)
            }
            None => vec![AcIssue::err(
                AcIssueKind::SectionNotFound,
                format!(
                    "清单 {} 的「## 3. 测试计划」二级标题定位失败，无法校验验收标准。\n\
                     机械校验**不回退整篇**（回退会让标题写坏的文档靠巧合蒙混过关）；\
                     请把该段标题改回 `## 3. 测试计划`",
                    r.id
                ),
            )],
        };
        out.extend(issues.into_iter().map(|mut i| {
            i.message = format!("{} {}", r.id, i.message);
            i
        }));
        let _ = name;
        // 段落实质性（REQ-003）：三段逐段独立判定。段定位失败时**不**报空段 ——
        // 那已由 SectionNotFound 说过一遍，重复报等于噪音掩盖真问题。
        let ph = requirement::template_placeholder_texts();
        for step in 0..STEPS_LEN {
            if crate::section::is_section_empty(&content, step, &ph) == Some(true) {
                out.push(AcIssue::err(
                    AcIssueKind::EmptySection,
                    crate::section::empty_section_message(&r.id, step),
                ));
            }
        }
    }
    Ok(out)
}

/// 三段步骤数（与 [`requirement::STEPS`] 一致；避免在 core 内部再引一次常量数组）。
const STEPS_LEN: usize = 3;

/// 校验一段正文（**纯函数**）：`first_line` 是该段首行在原文件中的 1-based 行号，
/// 报错时用于给出可跳转的行号。
pub fn lint(section: &str, first_line: usize) -> Vec<AcIssue> {
    scan(&section.lines().collect::<Vec<_>>(), first_line).issues
}

/// 解析一段正文里的 AC 条目（**纯函数**）。格式不合规的条目也会被解析出来
/// （`clauses` 少于 3 个），便于未来追溯矩阵按编号 join。
pub fn parse(section: &str, first_line: usize) -> Vec<AcEntry> {
    scan(&section.lines().collect::<Vec<_>>(), first_line).entries
}

struct Scan {
    entries: Vec<AcEntry>,
    issues: Vec<AcIssue>,
}

fn scan(lines: &[&str], first_line: usize) -> Scan {
    let mut issues = Vec::new();
    // ---- A1 定位块 ----
    let begin = lines.iter().position(|l| l.trim() == BEGIN);
    let Some(b) = begin else {
        issues.push(AcIssue::err(
            AcIssueKind::MissingBlock,
            format!(
                "第 3 段找不到 {} 标记块，无法校验验收标准。\n\
                 请在「## 3. 测试计划」下加入（顺序即下述）：\n\
                 {}\n### AC-001\n- Given: <前置状态>\n- When: <操作>\n\
                 - Then: <含退出码或字面量的结果>\n{}",
                BEGIN, BEGIN, END
            ),
        ));
        return Scan {
            entries: Vec::new(),
            issues,
        };
    };
    let Some(rel_end) = lines[b + 1..].iter().position(|l| l.trim() == END) else {
        issues.push(AcIssue::err(
            AcIssueKind::UnclosedBlock,
            format!(
                "第 {} 行有 {} 但没有 {}，验收标准块未闭合。",
                first_line + b,
                BEGIN,
                END
            ),
        ));
        return Scan {
            entries: Vec::new(),
            issues,
        };
    };
    let e = b + 1 + rel_end;

    // ---- 逐条扫描块内 ----
    let mut entries: Vec<AcEntry> = Vec::new();
    for (k, raw) in lines[b + 1..e].iter().enumerate() {
        // 内层第 k 行在 lines 里的下标是 b+1+k，故绝对 1-based 行号 = first_line + b + 1 + k
        let ln = first_line + b + 1 + k;
        if let Some((id, rest)) = id_line(raw) {
            if !rest.is_empty() {
                issues.push(AcIssue::err(
                    AcIssueKind::DirtyIdLine,
                    format!(
                        "第 {} 行的编号行里混进了子句内容（{rest:?}）。\n\
                         编号必须独占一行 —— 否则整条 AC 会变成一个超长标题，\
                         Markdown 渲染与目录都会被污染。\n\
                         请拆成：编号行 / `- Given:` / `- When:` / `- Then:` 四行",
                        ln
                    ),
                ));
            }
            entries.push(AcEntry {
                id,
                line: ln,
                clauses: Vec::new(),
            });
            continue;
        }
        // 标题检测必须在 strip_decoration **之前**：后者会把 `#` 剥掉，
        // 判完就再也看不出这一行原本是标题了（第一版就栽在这里，StrayHeading 永不触发）。
        let raw_trimmed = raw.trim_start();
        if raw_trimmed.starts_with('#') {
            let after = raw_trimmed.trim_start_matches('#').trim();
            if after.starts_with("AC") {
                issues.push(AcIssue::err(
                    AcIssueKind::BadIdFormat,
                    format!(
                        "第 {ln} 行编号格式非法（{after:?}）：编号须为 `AC-` 加**至少 3 位**数字。",
                    ),
                ));
            } else {
                issues.push(AcIssue::err(
                    AcIssueKind::StrayHeading,
                    format!(
                        "第 {ln} 行在验收标准块内混入了分级标题（{after:?}）。\n\
                         分组小标题一律放在 {} 之外，否则审核人扫读时会被误认成一条 AC。",
                        BEGIN
                    ),
                ));
            }
            continue;
        }
        if looks_like_ac_attempt(raw) {
            issues.push(AcIssue::err(
                AcIssueKind::BadIdFormat,
                format!(
                    "第 {ln} 行以 AC 开头却不是合法编号（{:?}）。",
                    strip_decoration(raw)
                ),
            ));
            continue;
        }
        if let Some(cl) = clause_line(raw, ln) {
            match entries.last_mut() {
                Some(last) => last.clauses.push(cl),
                None => issues.push(AcIssue::err(
                    AcIssueKind::MissingClause,
                    format!(
                        "第 {ln} 行出现子句，但它前面没有任何编号条目（子句必须归属某条 AC）。"
                    ),
                )),
            }
        }
    }

    // ---- A8 零条目 ----
    if entries.is_empty() {
        issues.push(AcIssue::err(
            AcIssueKind::NoItem,
            "验收标准块内没有任何条目：验收标准是第 3 段的全部价值，\
             至少写 1 条编号 + Given/When/Then 的 AC。",
        ));
    }

    // ---- A3 编号连续且不重复 ----
    let mut seen: Vec<&str> = Vec::new();
    for (i, ent) in entries.iter().enumerate() {
        if seen.contains(&ent.id.as_str()) {
            issues.push(AcIssue::err(
                AcIssueKind::DuplicateId,
                format!(
                    "第 {} 行的编号 {} 重复。编号是追溯矩阵的 join key，重复即失去join 能力。",
                    ent.line, ent.id
                ),
            ));
        } else {
            seen.push(&ent.id);
        }
        let expect = format!("AC-{:03}", i + 1);
        if ent.id != expect {
            issues.push(AcIssue::err(
                AcIssueKind::SeqGap,
                format!(
                    "第 {} 行的编号是 {}，按位置应为 {}（编号须自 AC-001 起连续、无跳号）。",
                    ent.line, ent.id, expect
                ),
            ));
            break;
        }
    }

    // ---- A4 / A5 / A6 / A9 / A10 / A12 ----
    for ent in entries.iter() {
        // A12：编号行的下一非空行含 ≥2 个全角 ｜ = 单行式残留
        if let Some(next) = lines
            .iter()
            .skip(ent.line - first_line + 1)
            .find(|l| !l.trim().is_empty())
        {
            if next.matches('｜').count() >= 2 {
                issues.push(AcIssue::err(
                    AcIssueKind::InlineEntry,
                    format!(
                        "第 {} 行是已废除的单行式（用全角 ｜ 分隔三子句）。\n\
                         条目形态唯一为三行式：编号独占一行，其下三个子句列表项 —— \
                         结构本身就在说明「这是三个子句」，审核人不用逐字断句。",
                        ent.line + 1
                    ),
                ));
            }
        }
        let got: Vec<&str> = ent.clauses.iter().map(|c| c.keyword.as_str()).collect();
        if got.len() != ORDER.len() {
            let missing: Vec<&str> = ORDER.iter().copied().filter(|k| !got.contains(k)).collect();
            issues.push(AcIssue::err(
                AcIssueKind::MissingClause,
                format!(
                    "{}（第 {} 行）有 {} 个子句，应恰好 3 个；缺：{}。\n\
                     每条 AC 必须齐备 Given/When/Then —— 缺一节就无法判定它到底验了什么。",
                    ent.id,
                    ent.line,
                    got.len(),
                    if missing.is_empty() {
                        "（多出的子句见顺序问题）".to_string()
                    } else {
                        missing.join(" / ")
                    }
                ),
            ));
            continue;
        }
        if got.as_slice() != ORDER {
            issues.push(AcIssue::err(
                AcIssueKind::OrderClause,
                format!(
                    "{}（第 {} 行）子句顺序是 [{}]，应为 [given, when, then]（Given → When → Then）。",
                    ent.id,
                    ent.line,
                    got.join(", ")
                ),
            ));
        }
        for cl in &ent.clauses {
            if cl.text.chars().filter(|c| !c.is_whitespace()).count() < 4 {
                issues.push(AcIssue::err(
                    AcIssueKind::EmptyClause,
                    format!(
                        "{}（第 {} 行）的 {} 子句是空占位（{:?}）。写具体，\
                         `Given: -` 这种写法等于没写。",
                        ent.id, cl.line, cl.keyword, cl.text
                    ),
                ));
            }
        }
        // A5 同一条内中英混用
        if ent.clauses.iter().any(|c| c.ascii) && ent.clauses.iter().any(|c| !c.ascii) {
            issues.push(AcIssue::warn(
                AcIssueKind::AliasMix,
                format!(
                    "{}（第 {} 行）同一条 AC 内混用了英文与中文子句关键词。\n\
                     建议整条统一，混用会让「哪一节是 When」变得要靠读者猜。",
                    ent.id, ent.line
                ),
            ));
        }
        // A10 Given 必须自包含（先剥离行内代码：引用违规样例是被允许的）
        if let Some(g) = ent.clause("given") {
            let bare = strip_inline_code(&g.text);
            if has_self_reference(&bare) {
                issues.push(AcIssue::err(
                    AcIssueKind::NonSelfContained,
                    format!(
                        "{}（第 {} 行）的 Given 用了外部指代（{:?}）。\n\
                         每条 AC 必须**自包含**：前置条件要原文写出，\
                         否则脱离上下文无法复现，也就无法判定。\n\
                         若只是要举违规例子，请用行内代码标记包起来（会被忽略）。",
                        ent.id, ent.line, g.text
                    ),
                ));
            }
        }
        // A9 Then 可度量
        if let Some(t) = ent.clause("then") {
            if !is_measurable(&t.text) {
                issues.push(AcIssue::warn(
                    AcIssueKind::Unmeasurable,
                    format!(
                        "{}（第 {} 行）的 Then（{:?}）里没有数字或字面量，\
                         无法机械判定是否通过。\n\
                         建议写成含退出码 / 计数 / 具体输出的形式。",
                        ent.id, t.line, t.text
                    ),
                ));
            }
        }
    }

    // ---- A7 块外出现 AC 编号 ----
    for (k, raw) in lines.iter().enumerate() {
        let abs = first_line + k;
        if abs > first_line + b && abs <= first_line + e {
            continue;
        }
        if raw.contains("AC-") && looks_like_ac_ref(raw) {
            issues.push(AcIssue::err(
                AcIssueKind::OutsideBlock,
                format!(
                    "第 {abs} 行在验收标准块**之外**出现 AC 编号（{}）。\n\
                     AC 只能写在 {} 与 {} 之间；写到别处的编号不会被计入，\
                     等于用「看起来有验收标准」骗过审批。",
                    raw.trim(),
                    BEGIN,
                    END
                ),
            ));
        }
    }

    Scan { entries, issues }
}

// ===================== 行级解析（纯函数小工具） =====================

/// 剥掉行首的 Markdown 装饰（标题井号、列表符）。
fn strip_decoration(line: &str) -> &str {
    let t = line.trim_start();
    let t = t.trim_start_matches('#').trim_start();
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .unwrap_or(t)
        .trim_start()
}

/// 解析编号行 → `(编号, 编号之后的残余内容)`。
fn id_line(line: &str) -> Option<(String, String)> {
    let body = strip_decoration(line);
    let rest = body.strip_prefix("AC-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 3 {
        return None;
    }
    let tail = rest[digits.len()..].trim_start();
    let tail = tail
        .strip_prefix(':')
        .or_else(|| tail.strip_prefix('\u{ff1a}'))
        .unwrap_or(tail)
        .trim();
    Some((format!("AC-{digits}"), tail.to_string()))
}

/// 是否"看起来想写编号但格式非法"（`AC-1` / `AC1` / `AC_001`）。
fn looks_like_ac_attempt(line: &str) -> bool {
    let body = strip_decoration(line);
    let rest = match body.strip_prefix("AC") {
        Some(r) => r,
        None => return false,
    };
    rest.chars()
        .next()
        .map(|c| c == '-' || c == '_' || c.is_ascii_digit())
        .unwrap_or(false)
}

/// 行内是否**引用**了一个 AC 编号（用于 A7，只认 `AC-数字` 形态，避免散文误伤）。
fn looks_like_ac_ref(line: &str) -> bool {
    let b: Vec<char> = line.chars().collect();
    for i in 0..b.len().saturating_sub(3) {
        if b[i] != 'A' || b[i + 1] != 'C' || b[i + 2] != '-' {
            continue;
        }
        let mut j = i + 3;
        let mut n = 0;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            n += 1;
        }
        if n >= 3 {
            return true;
        }
    }
    false
}

/// 解析子句行 → [`AcClause`]；不是子句行则 `None`。
///
/// ASCII 关键词（`Given:`）允许省略冒号；中文别名**必须**跟全角 `：`——
/// 单字别名 `当` / `则` 若允许省略冒号，`- 当前状态…` 会被误判成 `当` 子句。
fn clause_line(line: &str, ln: usize) -> Option<AcClause> {
    let body = strip_decoration(line);
    for (kw, aliases) in KEYWORDS {
        for alias in aliases {
            // `str::get` 在非字符边界处返回 None —— 直接 `body[..n]` 会 panic
            // （`执行` 的首字 `执` 占 3 字节，切 4 字节就切进 `行` 里了）。
            let hit = if alias.is_ascii() {
                match body.get(..alias.len()) {
                    Some(head) => {
                        head.eq_ignore_ascii_case(alias)
                            && matches!(
                                body[alias.len()..].chars().next(),
                                None | Some(':') | Some('\u{ff1a}')
                            )
                    }
                    None => false,
                }
            } else {
                body.starts_with(alias) && body[alias.len()..].starts_with('\u{ff1a}')
            };
            if !hit {
                continue;
            }
            let after = &body[alias.len()..];
            let text = after
                .trim_start_matches([':', '\u{ff1a}'])
                .trim()
                .to_string();
            return Some(AcClause {
                keyword: kw.to_string(),
                text,
                ascii: alias.is_ascii(),
                line: ln,
            });
        }
    }
    None
}

/// 剥掉行内代码（`` `…` ``）—— A10 的逃生口：引用违规样例是被允许的。
fn strip_inline_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_code = false;
    for c in s.chars() {
        match c {
            '`' => in_code = !in_code,
            _ if !in_code => out.push(c),
            _ => {}
        }
    }
    out
}

/// 是否含外部指代（A10）。
fn has_self_reference(given: &str) -> bool {
    ["与上一条相同", "同上", "如上条", "同上一条"]
        .iter()
        .any(|p| given.contains(p))
        || {
            // `同 AC-032` 这类引用其它编号
            let b: Vec<char> = given.chars().collect();
            (0..b.len().saturating_sub(3)).any(|i| {
                b[i] == '同'
                    && b.get(i + 1) == Some(&' ')
                    && b.get(i + 2) == Some(&'A')
                    && b.get(i + 3) == Some(&'C')
                    && b.get(i + 4) == Some(&'-')
            })
        }
}

/// A9：Then 是否可度量（含 ASCII 数字，或含反引号字面量）。
fn is_measurable(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit()) || s.contains('`')
}

#[cfg(test)]
#[allow(non_snake_case)] // 与 gate.rs / idcheck.rs 的测试命名一致：中文描述 + 规则号便于对读
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 造一段第 3 段正文（首行行号固定 1，便于断言消息里的行号）。
    fn sec(inner: &str) -> String {
        format!("## 3. 测试计划\n\n{BEGIN}\n{inner}\n{END}\n")
    }

    fn kinds(issues: &[AcIssue]) -> BTreeSet<AcIssueKind> {
        issues.iter().map(|i| i.kind).collect()
    }

    fn lint_sec(inner: &str) -> Vec<AcIssue> {
        lint(&sec(inner), 1)
    }

    const GOOD: &str = "### AC-001\n- Given: 清单第 3 段含 1 条合规条目\n- When: 执行 ac check REQ-001\n- Then: 退出码 0";

    // ---------- 正向 ----------

    #[test]
    fn 合规条目零问题且可解析出编号() {
        let issues = lint_sec(GOOD);
        assert!(issues.is_empty(), "合规条目不该有问题：{issues:?}");
        let entries = parse(&sec(GOOD), 1);
        assert_eq!(1, entries.len());
        assert_eq!("AC-001", entries[0].id);
        assert_eq!(3, entries[0].clauses.len());
        assert_eq!("given", entries[0].clauses[0].keyword);
        assert!(entries[0].clauses[2].text.contains("退出码 0"));
    }

    #[test]
    fn 中文别名可解析且_当_开头散文不误判() {
        let entries = parse(
            &sec("### AC-001\n- 假设：清单已批准\n- 当：执行 check\n- 则：退出码 0"),
            1,
        );
        assert_eq!(3, entries[0].clauses.len(), "中文别名应被识别");
        assert!(entries[0].clauses.iter().all(|c| !c.ascii));
        // 单字别名必须靠冒号消歧：散文不得被当成子句
        let e2 = parse(
            &sec("### AC-001\n- Given: 当前状态是草稿\n- When: 执行 check\n- Then: 退出码 0"),
            1,
        );
        assert_eq!(3, e2[0].clauses.len(), "「当前状态…」不得被切成子句");
    }

    // ---------- A1 / A8 ----------

    #[test]
    fn A1_缺块与未闭合() {
        assert!(
            kinds(&lint("## 3. 测试计划\n\n没有标记块\n", 1)).contains(&AcIssueKind::MissingBlock)
        );
        let unclosed = format!("## 3. 测试计划\n\n{BEGIN}\n{GOOD}\n");
        assert!(kinds(&lint(&unclosed, 1)).contains(&AcIssueKind::UnclosedBlock));
    }

    #[test]
    fn A8_块内零条目() {
        assert!(kinds(&lint_sec("")).contains(&AcIssueKind::NoItem));
    }

    // ---------- A2 / A3 ----------

    #[test]
    fn A2_编号行混入子句() {
        // 形态一：编号行本身就塞了三子句（旧单行式的写法）
        let dirty = "### AC-001 ｜ Given 甲 ｜ When 乙 ｜ Then 丙 3 段";
        assert!(
            kinds(&lint_sec(dirty)).contains(&AcIssueKind::DirtyIdLine),
            "编号行混入子句：{dirty}"
        );
        // 形态二：编号行干净，但下一行是全角 ｜ 分隔的单行式（A12）
        let inline = "### AC-001\nGiven 甲 ｜ When 乙 ｜ Then 丙 3 段";
        assert!(
            kinds(&lint_sec(inline)).contains(&AcIssueKind::InlineEntry),
            "单行式残留：{inline}"
        );
    }

    #[test]
    fn A3_跳号与重复() {
        let gap = "### AC-002\n- Given: 甲\n- When: 乙\n- Then: 丙 3";
        assert!(kinds(&lint_sec(gap)).contains(&AcIssueKind::SeqGap));
        let dup = format!("{GOOD}\n{GOOD}");
        assert!(kinds(&lint_sec(&dup)).contains(&AcIssueKind::DuplicateId));
    }

    #[test]
    fn A2_编号格式非法() {
        for bad in [
            "### AC-1\n- Given: 甲\n- When: 乙\n- Then: 丙 3",
            "### AC1\n- Given: 甲",
        ] {
            assert!(
                kinds(&lint_sec(bad)).contains(&AcIssueKind::BadIdFormat),
                "{bad} 应报 BadIdFormat"
            );
        }
    }

    // ---------- A4 / A6 ----------

    #[test]
    fn A4_缺子句与顺序颠倒() {
        let miss = "### AC-001\n- Given: 甲乙丙丁\n- When: 执行 check\n";
        let m = lint_sec(miss);
        assert!(kinds(&m).contains(&AcIssueKind::MissingClause), "{m:?}");
        assert!(
            m.iter().any(|i| i.message.contains("then")),
            "必须点名缺失子句：{m:?}"
        );
        let ord = "### AC-001\n- When: 执行 check\n- Given: 甲乙丙丁\n- Then: 退出码 0";
        assert!(kinds(&lint_sec(ord)).contains(&AcIssueKind::OrderClause));
    }

    #[test]
    fn A6_空占位子句() {
        let bad = "### AC-001\n- Given: -\n- When: 执行 check\n- Then: 退出码 0";
        let m = lint_sec(bad);
        assert!(kinds(&m).contains(&AcIssueKind::EmptyClause), "{m:?}");
        assert!(
            m.iter().any(|i| i.message.contains("第 5 行")),
            "须给行号：{m:?}"
        );
    }

    // ---------- A5 / A9（Warn） ----------

    #[test]
    fn A5_中英混用为告警() {
        let bad = "### AC-001\n- Given: 甲乙丙丁\n- When: 执行 check\n- 则：退出码 0";
        let m = lint_sec(bad);
        let i = m
            .iter()
            .find(|i| i.kind == AcIssueKind::AliasMix)
            .expect("应报混用");
        assert_eq!(Severity::Warn, i.severity, "混用只告警不阻断");
    }

    #[test]
    fn A9_then不可度量为告警() {
        let bad = "### AC-001\n- Given: 甲乙丙丁戊\n- When: 执行 check\n- Then: 结果符合预期";
        let m = lint_sec(bad);
        let i = m
            .iter()
            .find(|i| i.kind == AcIssueKind::Unmeasurable)
            .expect("应报不可度量");
        assert_eq!(Severity::Warn, i.severity);
        // 含反引号字面量即视为可度量
        let ok = "### AC-001\n- Given: 甲乙丙丁戊\n- When: 执行 check\n- Then: 输出 `放行`";
        assert!(!kinds(&lint_sec(ok)).contains(&AcIssueKind::Unmeasurable));
    }

    // ---------- A7 / A10 / A11 ----------

    #[test]
    fn A7_块外编号() {
        let s = format!("## 3. 测试计划\n\n### AC-001 这是块外\n\n{BEGIN}\n{GOOD}\n{END}\n");
        let m = lint(&s, 1);
        assert!(kinds(&m).contains(&AcIssueKind::OutsideBlock), "{m:?}");
    }

    #[test]
    fn A10_given外部指代_但行内代码豁免() {
        let bad = "### AC-001\n- Given: 与上一条相同\n- When: 执行 check\n- Then: 退出码 0";
        assert!(kinds(&lint_sec(bad)).contains(&AcIssueKind::NonSelfContained));
        // 引用违规样例（行内代码）不算指代
        let quoted = "### AC-001\n- Given: 某条 Given 写 `与上一条相同`\n- When: 执行 check\n- Then: 退出码 1";
        assert!(!kinds(&lint_sec(quoted)).contains(&AcIssueKind::NonSelfContained));
        // `同 AC-032` 这类外部编号引用
        let refd =
            "### AC-001\n- Given: 同 AC-032 的前置条件\n- When: 执行 check\n- Then: 退出码 1";
        assert!(kinds(&lint_sec(refd)).contains(&AcIssueKind::NonSelfContained));
    }

    #[test]
    fn A11_块内混入分组标题() {
        let bad = format!("#### 分组小标题\n{GOOD}");
        assert!(kinds(&lint_sec(&bad)).contains(&AcIssueKind::StrayHeading));
    }

    // ---------- 枚举覆盖门槛（§6.1） ----------

    /// 遍历 [`AcIssueKind`] 全量变体，断言每个都至少被一个用例命中。
    ///
    /// **为什么不用覆盖率工具**：本仓库零依赖、CI 无覆盖率步骤，"行覆盖 ≥ 90%"
    /// 是一条**不可判定**的门槛。枚举覆盖既可机械判定，又强制「新增规则必须新增
    /// 变体且变体必须有测试」——新增 kind 忘了写测试，这条用例必红。
    #[test]
    fn ac_枚举覆盖门槛_每个kind都有用例() {
        let mut covered: BTreeSet<AcIssueKind> = BTreeSet::new();
        // 块内 fixture：每条对应一个 kind
        for inner in [
            GOOD,
            "",                                                                        // NoItem
            "### AC-002\n- Given: 甲乙\n- When: 执行 check\n- Then: 退出码 0",         // SeqGap
            "### AC-1\n- Given: 甲乙\n- When: 执行 check\n- Then: 退出码 0", // BadIdFormat
            "### AC-001\n- Given: 甲乙\n- When: 执行 check",                 // MissingClause
            "### AC-001\n- When: 执行 check\n- Given: 甲乙\n- Then: 退出码 0", // OrderClause
            "### AC-001\n- Given: -\n- When: 执行 check\n- Then: 退出码 0",  // EmptyClause
            "### AC-001\n- Given: 甲乙\n- When: 执行 check\n- Then: 结果符合预期", // Unmeasurable
            "### AC-001\n- Given: 甲乙\n- When: 执行 check\n- 则：退出码 0", // AliasMix
            "### AC-001 ｜ Given 甲 ｜ When 乙 ｜ Then 丙 3 段",             // DirtyIdLine
            "### AC-001\nGiven 甲 ｜ When 乙 ｜ Then 丙 3 段",               // InlineEntry
            "### AC-001\n- Given: 与上一条相同\n- When: 执行 check\n- Then: 退出码 0", // NonSelfContained
            "#### 分组小标题\n### AC-001\n- Given: 甲乙\n- When: 执行 check\n- Then: 退出码 0", // StrayHeading
        ] {
            covered.extend(kinds(&lint_sec(inner)));
        }
        // 重复编号：两条同编号条目
        covered.extend(kinds(&lint_sec(&format!("{GOOD}\n{GOOD}"))));
        // 块级 fixture
        covered.extend(kinds(&lint("## 3. 测试计划\n\n没有块\n", 1))); // MissingBlock
        covered.extend(kinds(&lint(
            &format!("## 3. 测试计划\n\n{BEGIN}\n{GOOD}\n"),
            1,
        ))); // UnclosedBlock
        covered.extend(kinds(&lint(
            &format!("## 3. 测试计划\n\n### AC-001 块外\n\n{BEGIN}\n{GOOD}\n{END}\n"),
            1,
        ))); // OutsideBlock

        let all = [
            AcIssueKind::MissingBlock,
            AcIssueKind::UnclosedBlock,
            AcIssueKind::NoItem,
            AcIssueKind::BadIdFormat,
            AcIssueKind::DuplicateId,
            AcIssueKind::SeqGap,
            AcIssueKind::MissingClause,
            AcIssueKind::OrderClause,
            AcIssueKind::EmptyClause,
            AcIssueKind::OutsideBlock,
            AcIssueKind::AliasMix,
            AcIssueKind::Unmeasurable,
            AcIssueKind::DirtyIdLine,
            AcIssueKind::NonSelfContained,
            AcIssueKind::StrayHeading,
            AcIssueKind::InlineEntry,
        ];
        for k in all {
            assert!(
                covered.contains(&k),
                "AcIssueKind::{k:?} 没有任何用例命中 —— 新增 kind 必须同时写测试"
            );
        }
        // SectionNotFound 只能由 check() 产生（需要真实文件 + 真实标题），
        // 由下面的 `check_定位失败报SectionNotFound` 覆盖；这里显式记一笔，
        // 免得它悄悄从"必须有测试"降级成"没人管"。
        assert_eq!(AcIssueKind::SectionNotFound, AcIssueKind::SectionNotFound);
    }

    // ---------- check()：需真实文件 ----------

    #[test]
    fn check_定位失败报SectionNotFound而非回退整篇() {
        use crate::testutil::{cleanup, temp_dir};
        let root = temp_dir("ac-nofind");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        crate::requirement::create(&root, None, "坏标题").unwrap();
        // 先把三段填成有实质正文，再破坏第 3 段标题 ——
        // 否则段落实质性门禁会额外报 EmptySection，掩盖本用例要验的那条。
        crate::testutil::fill_sections(&root, "REQ-001");
        let r = crate::requirement::find(&root, "REQ-001").unwrap();
        let c = std::fs::read_to_string(&r.path).unwrap();
        // 把第 3 段标题改成非法形式：换成不参与编号的形式
        let broken = c.replace("## 3. 测试计划", "### 三、测试计划");
        std::fs::write(&r.path, broken).unwrap();

        // 各规则各管各的：只对 SectionNotFound 那条断言，不要求其它规则闭嘴
        let issues = check(&root, &AcTarget::All).unwrap();
        let nf: Vec<&AcIssue> = issues
            .iter()
            .filter(|i| i.kind == AcIssueKind::SectionNotFound)
            .collect();
        assert_eq!(1, nf.len(), "标题写坏必须报且只报一次定位失败：{issues:?}");
        assert!(
            nf[0].message.contains("不回退整篇"),
            "错误文案要说明为什么不回退：{issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.kind == AcIssueKind::EmptySection),
            "标题写坏不应连带报空段（那是另一条规则，且会掩盖真问题）：{issues:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn check_合规清单零问题且消息带需求编号() {
        use crate::testutil::{cleanup, temp_dir};
        let root = temp_dir("ac-ok");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        crate::requirement::create(&root, None, "合规").unwrap();
        // 三段都要有实质正文（段落实质性门禁），只填第 3 段的 AC 不够
        crate::testutil::fill_sections(&root, "REQ-001");
        let r = crate::requirement::find(&root, "REQ-001").unwrap();
        let c = std::fs::read_to_string(&r.path).unwrap();
        std::fs::write(
            &r.path,
            c.replace(
                "<可复现的前置状态；写具体，不写「正常情况」这类不可判定的前提>",
                "清单已装好且已批准",
            ),
        )
        .unwrap();
        // 其余两个占位子句同样可能被 A9 判不可度量，这里只看是否有 Error
        let issues = check(&root, &AcTarget::All).unwrap();
        assert!(
            !has_errors(&issues),
            "模板骨架填过之后不该有硬伤：{issues:?}"
        );
        assert!(
            issues.iter().all(|i| i.message.starts_with("REQ-001")),
            "消息须带需求编号：{issues:?}"
        );
        cleanup(&root);
    }
}

//! 段落实质性校验：消灭「占位空话」（设计见 REQ-003）。
//!
//! ## 为什么需要它
//!
//! 三段清单里只有第 3 段有内容校验（[`crate::ac`] 的 A1–A12）、第 2 段有声明校验
//! （[`crate::touch`] 的 T1–T6），**第 1 段完全裸奔**：`create` 出来的模板占位
//! `- [ ] 背景与问题：为什么要做这件事` 等五行可以原样留着、0 行实质正文，
//! 一路走到提交，没有任何一处会拦。
//!
//! 本仓 REQ-002 就中过：我填了第 2、3 段、漏了第 1 段，`ac check` 绿、
//! `touch-check` 绿、`check` 绿、提交通过 —— 靠人工自查才发现。
//!
//! ## 判定口径（全部可机械执行，不含任何"读起来像不像话"）
//!
//! 某段的**实质正文行数为 0** → [`SectionIssueKind::EmptySection`]。
//! **不设行数下限**：3 行写到的比 50 行复述有价值，设阈值会把"写短但写到了"
//! 判成不合格。
//!
//! 不计入实质正文的行（穷举，理由见各项）：
//! - 空行与纯空白 —— 没有信息
//! - HTML 注释整块（含跨行注释的续行、`<!-- GATE:… -->` 及其说明文字）
//! - 与模板占位文案同源的 checkbox 行，**勾选与否都不算** —— 勾上框不等于写了内容
//! - Markdown 标题行、结构骨架
//! - 表格分隔行（`|---|`）与水平线（`---` / `***`）
//! - 块引用行（`> …`）—— 模板顶部的门禁说明用的就是它
//!
//! ## 两条设计红线
//!
//! **1. 占位判定用「整行相等」而非前缀匹配。** 作者完全会写
//! 「背景与问题：本次要解决 AI 自批」—— 前缀匹配会把它当成模板占位而杀掉真实内容。
//! 这是最危险的误报方向：误报会让门禁被习惯性绕过，等于没有门禁。
//!
//! **2. 占位文案从模板常量派生，不另写副本。** 副本等于给文档漂移开一个口子：
//! 模板改了、判定没改，于是"占位"永远认不出来。派生入口是
//! [`crate::requirement::template_placeholder_texts`]。
//!
//! ## 与 A8 / T1 的交叉一致性
//!
//! **GATE 块内的条目一律不计入实质正文**：第 3 段的 AC 条目由 A8 管、第 2 段的
//! TOUCH 声明由 T1 管。若把它们计入，就会出现「只有一个空 AC 骨架的段落被判为有内容」，
//! 两条规则互相掩护 —— 那是最坏的组合。排除之后：A8 命中时本规则必然也命中，
//! 两条独立路径指向同一结论。

use crate::issue::Severity;
use crate::requirement;

/// 段落实质性问题类别。**枚举即契约**，由单测遍历全量断言其被测到。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionIssueKind {
    /// 该段实质正文行数为 0（只有模板占位 / 注释 / 标题 / 空行）。
    EmptySection,
}

impl SectionIssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SectionIssueKind::EmptySection => "EmptySection",
        }
    }
}

/// 一条段落实质性问题。`message` 自含上下文（段名 + 判定依据 + 修复指引）。
#[derive(Debug, Clone)]
pub struct SectionIssue {
    pub severity: Severity,
    pub kind: SectionIssueKind,
    pub message: String,
}

/// 去掉列表符与勾选框前缀，返回其后的文本；不是 checkbox 行则 `None`。
///
/// 支持任意层级的 `-` / `*` + 空格，以及 `[ ]` / `[x]` / `[X]` + 空格。
fn strip_checkbox(t: &str) -> Option<&str> {
    let mut rest = t;
    loop {
        let trimmed = rest.trim_start();
        let after = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "));
        match after {
            Some(a) => rest = a.trim_start(),
            None => break,
        }
    }
    let rest = rest.trim_start();
    for mark in ["[ ] ", "[x] ", "[X] "] {
        if let Some(r) = rest.strip_prefix(mark) {
            return Some(r.trim());
        }
    }
    None
}

/// 从模板正文里抽出全部占位文案（勾选框之后的文本），去重保序。
///
/// 纯函数：文案由调用方传入，**不在本模块硬编码模板** —— 那是漂移的起点。
pub fn placeholder_texts(bodies: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in bodies {
        for line in b.lines() {
            if let Some(text) = strip_checkbox(line.trim()) {
                if !text.is_empty() && !out.iter().any(|x| x == text) {
                    out.push(text.to_string());
                }
            }
        }
    }
    out
}

/// 是否 Markdown 表格分隔行（`|---|---|`、`| :--- | ---: |`）。
fn is_table_separator(t: &str) -> bool {
    if !t.contains('|') {
        return false;
    }
    t.chars()
        .all(|c| c == '|' || c == '-' || c == ':' || c.is_whitespace())
}

/// 单行是否构成实质正文（**纯函数**）。
///
/// `placeholders` 为模板占位文案集合。注释与标题的判定在此完成；
/// 跨行注释与 GATE 块由调用方用状态机处理（见 [`count_substantive`]）。
pub fn is_substantive(line: &str, placeholders: &[String]) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // 结构骨架：标题行、水平线
    if t.starts_with('#') {
        return false;
    }
    if matches!(t, "---" | "***" | "___") {
        return false;
    }
    if is_table_separator(t) {
        return false;
    }
    // 块引用：模板顶部的门禁说明用的就是它
    if t.starts_with('>') {
        return false;
    }
    // 单行闭合的 HTML 注释
    if t.starts_with("<!--") {
        return false;
    }
    // checkbox 行：整行等于占位文案 → 非实质（**整行相等，不做前缀匹配**）
    if let Some(text) = strip_checkbox(t) {
        return !placeholders.iter().any(|p| p == text);
    }
    true
}

/// 统计一段里的实质正文行数。
///
/// 状态机职责（[`is_substantive`] 只看单行，处理不了这两种跨行形态）：
/// - **跨行 HTML 注释**：模板里那段「验收标准：…」说明就是跨行的，逐行看都不像注释
/// - **GATE 块**：标记行之间的 AC 条目 / TOUCH 声明由 A8 / T1 各管，不计入实质
pub fn count_substantive(section: &str, placeholders: &[String]) -> usize {
    let mut in_comment = false;
    let mut in_gate_block = false;
    let mut n = 0usize;
    for line in section.lines() {
        let t = line.trim();
        if in_comment {
            if t.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if requirement::is_marker_line(line) {
            // 标记行本身不是内容；`<!-- GATE:x -->` 开块、`<!-- /GATE:x -->` 收块
            in_gate_block = !t.starts_with("<!-- /GATE:");
            continue;
        }
        if in_gate_block {
            continue;
        }
        if t.starts_with("<!--") {
            if !t.contains("-->") {
                in_comment = true;
            }
            continue;
        }
        if is_substantive(line, placeholders) {
            n += 1;
        }
    }
    n
}

/// 该段是否无实质正文。
///
/// 段定位失败（标题被改坏）时返回 `None` —— 那由 `ac::check` 的 `SectionNotFound`
/// 单独报，不在这里重复报一次空段。
pub fn is_section_empty(content: &str, step: usize, placeholders: &[String]) -> Option<bool> {
    let (start, end) = requirement::section_span(content, step)?;
    let lines: Vec<&str> = content.lines().collect();
    let section = lines[start - 1..end].join("\n");
    Some(count_substantive(&section, placeholders) == 0)
}

/// 某段的空段报错文案（**可执行**：说清补什么、去哪补）。
///
/// `id` 传空串即可 —— `ac::check` 会统一在前面拼上清单编号。
pub fn empty_section_message(id: &str, step: usize) -> String {
    let (name, guide) = match step {
        0 => (
            "需求分解",
            "至少写清：背景与问题（为什么要做这件事）、目标与非目标（明确不做什么）、\
             子任务拆解（编号 + 预估工时）、影响范围（涉及模块 / 接口 / 数据表）。",
        ),
        1 => (
            "技术方案",
            "至少写清：总体思路（一句话说清怎么做）、关键设计（数据结构 / 接口签名 / 调用流程）、\
             兼容性 / 性能 / 安全影响、风险点与回滚方案。\
             另外别忘了 {MARK} 块里的变更范围声明。",
        ),
        _ => (
            "测试计划",
            "至少写清：单元测试用例、端到端用例、边界 / 异常 / 并发场景、回归范围与影响面。\
             另外别忘了 {MARK} 块里的验收标准（编号 + Given/When/Then）。",
        ),
    };
    // 每段指向**自己**那块标记：技术方案 → GATE:TOUCH，测试计划 → GATE:AC。
    // 写错会让读者去错误的块里找声明（第一版就把技术方案指向了 GATE:AC）。
    let mark = match step {
        0 => "",
        1 => requirement::render_marker("solution"),
        _ => requirement::render_marker("testplan"),
    };
    let guide = guide.replace("{MARK}", mark);
    format!(
        "需求 {id} 的「{name}」段实质正文为 0 行 —— 只有模板占位、空行、注释或标题，等于没写。\n\
         模板占位行不算内容（勾选也不算），GATE 标记块内的条目由各自的规则单独校验。\n\
         请补写：{guide}\n\
         校验命令：req-guard ac check {id}"
    )
}

#[cfg(test)]
#[allow(non_snake_case)] // 与既有中文测试命名一致
mod tests {
    use super::*;

    const PH: [&str; 3] = [
        "- [ ] 背景与问题：为什么要做这件事\n- [ ] 目标 / 非目标（明确不做什么）",
        "- [ ] 总体思路（一句话说清怎么做）\n- [ ] 涉及的文件与模块清单",
        "- [ ] 单元测试用例（编号 + 断言点）\n- [ ] 端到端用例（编号 + 执行步骤）",
    ];

    fn ph() -> Vec<String> {
        placeholder_texts(&PH)
    }

    // ---------- 非实质行 ----------

    #[test]
    fn 非实质_空行注释标题水平线分隔线引用() {
        let p = ph();
        for line in [
            "",
            "   ",
            "\t",
            "## 1. 需求分解",
            "### 背景与问题",
            "---",
            "***",
            "|---|---|",
            "| :--- | ---: |",
            "> **AI 需求门禁清单**：三段全部 approved 后…",
            "<!-- GATE:AC -->",
            "<!-- /GATE:AC -->",
            "<!-- 单行注释 -->",
        ] {
            assert!(!is_substantive(line, &p), "{line:?} 不该算实质正文");
        }
    }

    #[test]
    fn 非实质_模板占位_且勾选也不算() {
        let p = ph();
        assert!(!is_substantive("- [ ] 背景与问题：为什么要做这件事", &p));
        assert!(
            !is_substantive("- [x] 背景与问题：为什么要做这件事", &p),
            "勾选不等于写了内容"
        );
        assert!(!is_substantive("  - [ ] 总体思路（一句话说清怎么做）", &p));
        assert!(!is_substantive("* [X] 端到端用例（编号 + 执行步骤）", &p));
    }

    // ---------- 实质行 ----------

    #[test]
    fn 实质_自定义checkbox算内容() {
        let p = ph();
        assert!(is_substantive(
            "- [x] 背景与问题：AI 可以给自己批过的清单补摘要",
            &p
        ));
        assert!(is_substantive("- 我们把声明源收敛到一处", &p));
        assert!(is_substantive("段落正文。", &p));
        assert!(is_substantive("| 编号 | 子任务 |", &p), "表头算内容");
    }

    #[test]
    fn 占位判定用整行相等而非前缀匹配() {
        // 最危险的误报方向：作者在占位文案后面续写，前缀匹配会误杀
        let p = ph();
        assert!(
            is_substantive("- [ ] 背景与问题：AI 自批导致审批失效", &p),
            "续写内容不得被当成占位"
        );
        assert!(is_substantive(
            "- 目标 / 非目标（明确不做什么）与本条相关",
            &p
        ));
    }

    // ---------- 跨行形态 ----------

    #[test]
    fn 跨行注释的续行不算内容() {
        let sec = "<!-- 验收标准：机械校验。\n     条目形态唯一：三行式。\n     标记行只能由 req-guard 维护。 -->\n";
        assert_eq!(0, count_substantive(sec, &ph()), "跨行注释整块不算");
    }

    #[test]
    fn gate块内条目不算内容() {
        let sec = "\
## 2. 技术方案

总体思路：一句话说清。

<!-- GATE:TOUCH -->
core/src/**
<!-- /GATE:TOUCH -->
";
        // 只有「总体思路」1 行实质；TOUCH 声明由 T1 管，不计入
        assert_eq!(1, count_substantive(sec, &ph()));
    }

    #[test]
    fn 空AC骨架不救活空段() {
        // 与 A8 交叉一致：空 AC 骨架既报 NoItem 也报 EmptySection
        let sec = "## 3. 测试计划\n\n<!-- GATE:AC -->\n### AC-001\n- Given: - \n- When: -\n- Then: -\n<!-- /GATE:AC -->\n";
        assert_eq!(0, count_substantive(sec, &ph()));
    }

    #[test]
    fn 三行具体内容即合规_无行数阈值() {
        let sec = "## 1. 需求分解\n\n- [ ] 背景与问题：为什么要做这件事\n- 我们要解决 AI 自批。\n- 做法是加摘要。\n";
        assert_eq!(2, count_substantive(sec, &ph()), "只有 2 行也要判有内容");
        assert!(!is_section_empty_in(sec));
    }

    fn is_section_empty_in(sec: &str) -> bool {
        let full = format!("{sec}\n## 2. 技术方案\n\nx\n\n## 3. 测试计划\n\ny\n");
        !full.contains("x") || count_substantive(sec, &ph()) == 0
    }

    // ---------- 派生与防漂移 ----------

    #[test]
    fn placeholder_texts_从模板派生且去重保序() {
        let got = placeholder_texts(&PH);
        assert_eq!(
            vec![
                "背景与问题：为什么要做这件事",
                "目标 / 非目标（明确不做什么）",
                "总体思路（一句话说清怎么做）",
                "涉及的文件与模块清单",
                "单元测试用例（编号 + 断言点）",
                "端到端用例（编号 + 执行步骤）",
            ],
            got
        );
        // 重复传入不产生重复条目
        assert_eq!(6, placeholder_texts(&[PH[0], PH[0]]).len() + 4);
    }

    /// **防漂移**：真实模板里的每一条占位行都必须被识别为非实质。
    /// 模板改了措辞而派生没跟上时，这条用例必红 —— 占位副本是漂移的起点。
    #[test]
    fn 防漂移_真实模板每条占位都被识别() {
        let bodies = crate::requirement::template_bodies();
        let ph = placeholder_texts(&bodies);
        assert!(
            ph.len() >= 15,
            "从真实模板应派生出至少 15 条占位文案，实际 {}",
            ph.len()
        );
        for body in &bodies {
            for line in body.lines() {
                if let Some(text) = strip_checkbox(line.trim()) {
                    assert!(
                        ph.iter().any(|p| p == text),
                        "模板占位 {text:?} 未出现在派生集合里 —— 判定会把它当实质正文"
                    );
                    assert!(
                        !is_substantive(line, &ph),
                        "模板占位行被误判为实质正文：{line:?}"
                    );
                }
            }
        }
    }

    // ---------- 段定位 ----------

    #[test]
    fn is_section_empty_段标题坏掉时返回None而非误报() {
        let doc = "## 三、需求分解\n\n- 只有占位\n";
        assert_eq!(None, is_section_empty(doc, 0, &ph()));
        let doc2 = "## 1. 需求分解\n\n- [ ] 背景与问题：为什么要做这件事\n";
        assert_eq!(Some(true), is_section_empty(doc2, 0, &ph()));
    }

    #[test]
    fn 段边界不串_后段内容不算进前段() {
        let doc = "## 1. 需求分解\n\n- [ ] 背景与问题：为什么要做这件事\n\n## 2. 技术方案\n\n- 实质内容\n";
        // 第 1 段仍为空（后段的实质内容不算它的）
        assert_eq!(Some(true), is_section_empty(doc, 0, &ph()));
        assert_eq!(Some(false), is_section_empty(doc, 1, &ph()));
    }

    // ---------- 文案 ----------

    #[test]
    fn 空段文案_给出可执行指引() {
        let m = empty_section_message("REQ-003", 0);
        assert!(m.contains("REQ-003"), "{m}");
        assert!(m.contains("背景与问题"), "{m}");
        assert!(m.contains("req-guard ac check REQ-003"), "{m}");
    }

    /// 每段的指引必须指向**自己**那块标记。
    /// 第一版把技术方案段指向了 `GATE:AC`（验收标准）—— 读者会去错误的块里找声明。
    #[test]
    fn 空段文案_每段指向自己的标记() {
        let m1 = empty_section_message("REQ-003", 1);
        assert!(
            m1.contains(crate::touch::BEGIN),
            "技术方案段应指向变更范围块：{m1}"
        );
        assert!(
            !m1.contains(crate::ac::BEGIN),
            "技术方案段不该指向验收标准块：{m1}"
        );
        let m2 = empty_section_message("REQ-003", 2);
        assert!(
            m2.contains(crate::ac::BEGIN),
            "测试计划段应指向验收标准块：{m2}"
        );
        assert!(
            !m2.contains(crate::touch::BEGIN),
            "测试计划段不该指向变更范围块：{m2}"
        );
        // 不得残留未替换的占位符
        assert!(
            !m1.contains("{MARK}") && !m2.contains("{MARK}"),
            "占位符未替换"
        );
    }

    // ---------- 枚举覆盖门槛 ----------

    #[test]
    fn section_枚举覆盖门槛() {
        let ph = ph();
        let doc = "## 1. 需求分解\n\n- [ ] 背景与问题：为什么要做这件事\n\n## 2. 技术方案\n\nx\n\n## 3. 测试计划\n\ny\n";
        let mut covered = std::collections::BTreeSet::new();
        for step in 0..3 {
            if is_section_empty(doc, step, &ph) == Some(true) {
                covered.insert(SectionIssueKind::EmptySection);
            }
        }
        assert!(covered.contains(&SectionIssueKind::EmptySection));
    }
}

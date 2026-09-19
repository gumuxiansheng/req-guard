//! 需求编号防冲突检查（对应《docs/规范/需求编号防冲突命名规范.md》§8 的 P1 与 P3）。
//!
//! `check()`（CLI：`ids --check`）做三类检测：
//! ① **同 id 多文件**：GATE:HEAD 的 `id=` token 相同却存在多个文件——slug 不同时
//!    git 零冲突**静默共存**，`find()` 按文件名字典序任取其一，是最坏形态（Error）；
//! ② **自动编号污染**：`REQ-` 后紧跟 ≥5 位纯数字（典型为 8 位日期），会被
//!    `next_id()` 当作编号参与 max，把自增基线永久顶到日期量级（Warn；阈值取
//!    ≥5 位而非 >3 位，是为了兼容 REQ-1000+ 仓库的自然序号增长）；
//! ③ **前缀歧义**：文件名 A 恰为另一文件名的 `<A>-` 前缀（且两者 id token 不同，
//!    相同 id 已由 ① 覆盖）——`find(A)` 的前缀匹配将命中其一（Error）。
//!
//! `lint_id()`（CLI：`create --id <编号>` 时打印，**不阻断**）对命中违规形态的
//! 自定义编号给出提示：C2 数字开头、C4 超长、C3 大小写写法、非推荐档形状。
//!
//! 与项目哲学一致：判定只认 `list()` 读出的 GATE:HEAD `id=` token（Rust 真解析），
//! 不匹配人类可读文案。

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;
use crate::requirement;

/// 问题严重级：`Error` 使 `ids --check` 退出码非 0；`Warn` 仅提示不阻断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Warn => "警告",
            Severity::Error => "错误",
        }
    }

    pub fn is_error(self) -> bool {
        matches!(self, Severity::Error)
    }
}

/// 问题类别（对应规范 §8 P1 三类检测 + P3 编号 lint）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdIssueKind {
    /// ① 同 id 多文件（静默双 id）。
    Duplicate,
    /// 文件无 GATE:HEAD 或 id token 为空（list 里的幽灵需求）。
    MissingId,
    /// ② `REQ-` 后紧跟长数字段，污染 `next_id()` 自动编号。
    Pollution,
    /// ③ 文件名前缀歧义，`find()` 可能命中错误文件。
    PrefixAmbiguity,
    /// P3：编号写法偏离推荐档（lint 提示，不阻断）。
    Format,
}

/// 一条编号问题。`message` 自含上下文（涉及文件 / id / 修复建议），可直接展示。
#[derive(Debug, Clone)]
pub struct IdIssue {
    pub severity: Severity,
    pub kind: IdIssueKind,
    pub message: String,
}

/// 是否存在硬伤（`ids --check` 据此决定退出码）。
pub fn has_errors(issues: &[IdIssue]) -> bool {
    issues.iter().any(|i| i.severity.is_error())
}

/// 扫描 `.gates/requirements/`，返回全部编号问题（目录不存在视为空集，与 `list()` 一致）。
pub fn check(root: &Path) -> Result<Vec<IdIssue>> {
    let reqs = requirement::list(root)?;
    let mut issues = Vec::new();

    // ① 按 GATE:HEAD 的 id token 分组：同 token 多文件 = 静默双 id。
    let mut by_id: BTreeMap<&str, Vec<&requirement::Requirement>> = BTreeMap::new();
    for r in &reqs {
        if r.id.is_empty() {
            issues.push(IdIssue {
                severity: Severity::Warn,
                kind: IdIssueKind::MissingId,
                message: format!(
                    "文件 {} 的 GATE:HEAD 未读到 id token（幽灵需求，status/find 无法定位），请补齐机读标记行",
                    file_name(&r.path)
                ),
            });
        } else {
            by_id.entry(r.id.as_str()).or_default().push(r);
        }
    }
    for (id, group) in &by_id {
        if group.len() > 1 {
            let names: Vec<String> = group.iter().map(|r| file_name(&r.path)).collect();
            issues.push(IdIssue {
                severity: Severity::Error,
                kind: IdIssueKind::Duplicate,
                message: format!(
                    "id `{}` 被 {} 个文件同时使用：{}（slug 不同时 git 零冲突静默共存；处置：先合入者为准，后合入者换号重做并作废旧文件）",
                    id,
                    group.len(),
                    names.join("、")
                ),
            });
        }
    }

    // ② 自动编号污染：`REQ-` 后的连续数字段 ≥5 位即告警（8 位日期是典型形态）。
    for r in &reqs {
        let stem = file_stem(&r.path);
        if let Some((digits, polluted_next)) = pollution_of(&stem) {
            issues.push(IdIssue {
                severity: Severity::Warn,
                kind: IdIssueKind::Pollution,
                message: format!(
                    "文件 {} 的 `REQ-` 后紧跟 {} 位数字（{}）：会被 next_id() 当作编号，把自动编号基线顶成 {}，疑似日期开头 id（违反规范 C2：REQ- 后第一段应字母开头）",
                    file_name(&r.path),
                    digits.chars().count(),
                    digits,
                    polluted_next
                ),
            });
        }
    }

    // ③ 前缀歧义：文件名 A 恰为另一文件名的 `<A>-` 前缀。id token 相同的对已由
    //    ① 报错，此处只报 id 不同的对（含一方 id 为空）。
    let mut stems: Vec<(String, String)> = reqs
        .iter()
        .map(|r| (file_stem(&r.path), r.id.clone()))
        .collect();
    stems.sort();
    for i in 0..stems.len() {
        for j in 0..stems.len() {
            if i == j {
                continue;
            }
            let (a, id_a) = (&stems[i].0, stems[i].1.as_str());
            let (b, id_b) = (&stems[j].0, stems[j].1.as_str());
            if is_stem_prefix(a, b) && id_a != id_b {
                issues.push(IdIssue {
                    severity: Severity::Error,
                    kind: IdIssueKind::PrefixAmbiguity,
                    message: format!(
                        "文件名 {} 是 {} 的 `<A>-` 前缀（id `{}` ≠ `{}`）：find(`{}`) 将按文件名字典序命中其一，引用与判定可能落错文件（规范 C7：引用 id 必须全文，命名不得互为前缀）",
                        a, b, id_a, id_b, a
                    ),
                });
            }
        }
    }

    Ok(issues)
}

/// P3：`create --id <编号>` 的写法 lint（提示不阻断；纯数字与空串交给 `normalize_id` 处理）。
pub fn lint_id(raw: &str) -> Vec<IdIssue> {
    let s = raw.trim();
    let mut out = Vec::new();
    if s.is_empty() || s.chars().all(|c| c.is_ascii_digit()) {
        // 纯数字会被归一为 REQ-NNN 自动档，天然合规。
        return out;
    }

    // C2：日期式数字开头污染自动编号。
    let mut polluted = false;
    if let Some(rest) = s.strip_prefix("REQ-") {
        let nd = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if nd >= 5 {
            polluted = true;
            out.push(IdIssue {
                severity: Severity::Warn,
                kind: IdIssueKind::Pollution,
                message: format!(
                    "编号 `REQ-` 后紧跟 {} 位数字：疑似日期开头，会把 next_id() 的自动编号基线顶成日期量级（C2：REQ- 后第一段应字母开头）",
                    nd
                ),
            });
        }
    }

    // C4：超长（评论文件还要追加 `.comments.md`）。
    let chars = s.chars().count();
    if chars > 32 {
        out.push(IdIssue {
            severity: Severity::Warn,
            kind: IdIssueKind::Format,
            message: format!(
                "编号 {} 字符，超过 32：加 slug 与 `.comments.md` 后在 Windows 短路径环境易超限（C4）",
                chars
            ),
        });
    }

    // 推荐档形状识别（大小写不敏感识别，再按 C3 检查写法）。
    let parts: Vec<&str> = s.split('-').collect();
    match Shape::of(&parts) {
        Shape::Other => {
            // C2 污染已给出更明确的修复指引时，不再叠加泛化的"非推荐档"提示。
            if !polluted {
                out.push(IdIssue {
                    severity: Severity::Warn,
                    kind: IdIssueKind::Format,
                    message: "未匹配推荐命名档（建议 REQ-<owner>-<YYYYMMDD>-<rrrr>，见 docs/规范/需求编号防冲突命名规范.md）；自由命名仍被允许（向后兼容）".to_string(),
                });
            }
        }
        Shape::Auto => {}
        Shape::SmallTeam => {
            lint_lowercase(parts[1], "owner 段", &mut out);
        }
        Shape::Hybrid { modules } => {
            for seg in &parts[1..1 + modules] {
                lint_lowercase(seg, "模块段", &mut out);
            }
            lint_lowercase(parts[parts.len() - 3], "owner 段", &mut out);
            lint_uppercase(parts[parts.len() - 1], "随机段", &mut out);
        }
    }
    out
}

// ===================== 形状识别 =====================

/// 推荐档形状（大小写不敏感识别；细节见规范 §5/§6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `REQ-<纯数字>`（含纯数字输入归一后的形态）。
    Auto,
    /// `REQ-<owner>-<序号>`（小团队档）。
    SmallTeam,
    /// `REQ-[<mod>-]<owner>-<8位日期>-<4位随机>`（并行默认档 / monorepo 档）。
    Hybrid { modules: usize },
    /// 其余（自由命名，允许但给一次性提示）。
    Other,
}

impl Shape {
    fn of(parts: &[&str]) -> Shape {
        if parts.is_empty() || !parts[0].eq_ignore_ascii_case("REQ") {
            return Shape::Other;
        }
        match parts.len() {
            2 => {
                if is_digits(parts[1]) {
                    Shape::Auto
                } else {
                    Shape::Other
                }
            }
            3 => {
                if is_short_tag(parts[1]) && is_digits(parts[2]) {
                    Shape::SmallTeam
                } else {
                    Shape::Other
                }
            }
            4 | 5 => {
                let total = parts.len();
                let mods = &parts[1..total - 3];
                let owner = parts[total - 3];
                let date = parts[total - 2];
                let rand = parts[total - 1];
                let ok = mods.iter().all(|p| is_short_tag(p))
                    && is_short_tag(owner)
                    && date.len() == 8
                    && is_digits(date)
                    && rand.len() == 4
                    && is_crockford(rand);
                if ok {
                    Shape::Hybrid {
                        modules: mods.len(),
                    }
                } else {
                    Shape::Other
                }
            }
            _ => Shape::Other,
        }
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// owner / mod 段：2–4 位、字母开头、ASCII 字母数字。
fn is_short_tag(s: &str) -> bool {
    let n = s.chars().count();
    (2..=4).contains(&n)
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Crockford Base32（去 I/L/O/U），大小写不敏感识别。
fn is_crockford(s: &str) -> bool {
    s.chars().all(|c| {
        matches!(
            c.to_ascii_uppercase(),
            '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z'
        )
    })
}

fn lint_lowercase(seg: &str, label: &str, out: &mut Vec<IdIssue>) {
    if !seg
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        out.push(IdIssue {
            severity: Severity::Warn,
            kind: IdIssueKind::Format,
            message: format!(
                "{} `{}` 应全小写（C3：大小写只作写法约定，Windows/macOS 文件系统大小写不敏感）",
                label, seg
            ),
        });
    }
}

fn lint_uppercase(seg: &str, label: &str, out: &mut Vec<IdIssue>) {
    if !seg
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        out.push(IdIssue {
            severity: Severity::Warn,
            kind: IdIssueKind::Format,
            message: format!(
                "{} `{}` 应全大写（C3：大小写只作写法约定，Windows/macOS 文件系统大小写不敏感）",
                label, seg
            ),
        });
    }
}

// ===================== 文件名工具 =====================

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// `a` 是否为 `b` 的 `<a>-` 前缀（逐字节比较，避免循环内分配）。
fn is_stem_prefix(a: &str, b: &str) -> bool {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    y.len() > x.len() && y.starts_with(x) && y[x.len()] == b'-'
}

/// 若 stem 形如 `REQ-<≥5位数字>…`，返回（数字段, 被顶高后的下一个自动编号）。
fn pollution_of(stem: &str) -> Option<(String, String)> {
    let rest = stem.strip_prefix("REQ-")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.chars().count() < 5 {
        return None;
    }
    let next = digits
        .parse::<u32>()
        .ok()
        .map(|n| format!("REQ-{:03}", n + 1))
        .unwrap_or_else(|| "超出编号范围的数值".to_string());
    Some((digits, next))
}

// ===================== 测试 =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cleanup, temp_dir};
    use std::fs;

    /// 在 root 的 requirements 目录写一个清单文件；id 为 None 时写无 GATE:HEAD 的普通 md。
    fn w(root: &Path, fname: &str, id: Option<&str>) {
        let dir = root.join(requirement::REQ_DIR);
        fs::create_dir_all(&dir).unwrap();
        let head = match id {
            Some(i) => format!("<!-- GATE:HEAD id={} status=draft created=0 -->\n", i),
            None => String::new(),
        };
        fs::write(dir.join(fname), format!("{}# 标题\n", head)).unwrap();
    }

    fn kinds(issues: &[IdIssue]) -> Vec<IdIssueKind> {
        issues.iter().map(|i| i.kind).collect()
    }

    #[test]
    fn check_干净目录无问题() {
        let d = temp_dir("ids-clean");
        w(&d, "REQ-001.md", Some("REQ-001"));
        w(&d, "REQ-kd-20260919-A7F3.md", Some("REQ-kd-20260919-A7F3"));
        let issues = check(&d).unwrap();
        assert!(issues.is_empty(), "{:?}", issues);
        cleanup(&d);
    }

    #[test]
    fn check_同id多文件报错且不重复报前缀歧义() {
        let d = temp_dir("ids-dup");
        w(&d, "REQ-001-login.md", Some("REQ-001"));
        w(&d, "REQ-001-pay.md", Some("REQ-001"));
        let issues = check(&d).unwrap();
        assert_eq!(kinds(&issues), vec![IdIssueKind::Duplicate]);
        assert!(has_errors(&issues));
        assert!(issues[0].message.contains("REQ-001-login.md"));
        cleanup(&d);
    }

    #[test]
    fn check_日期开头污染自动编号() {
        let d = temp_dir("ids-pollute");
        w(&d, "REQ-20260919-A7F3.md", Some("REQ-20260919-A7F3"));
        let issues = check(&d).unwrap();
        assert_eq!(kinds(&issues), vec![IdIssueKind::Pollution]);
        assert!(!has_errors(&issues));
        assert!(
            issues[0].message.contains("REQ-20260920"),
            "{}",
            issues[0].message
        );
        cleanup(&d);
    }

    #[test]
    fn check_四位内数字不告警_兼容千号以上仓库() {
        let d = temp_dir("ids-pollute4");
        w(&d, "REQ-1234-login.md", Some("REQ-1234"));
        assert!(check(&d).unwrap().is_empty());
        cleanup(&d);
    }

    #[test]
    fn check_前缀歧义报错() {
        let d = temp_dir("ids-prefix");
        w(&d, "REQ-kd.md", Some("REQ-kd"));
        w(&d, "REQ-kd-20260919-A7F3.md", Some("REQ-kd-20260919-A7F3"));
        let issues = check(&d).unwrap();
        assert_eq!(kinds(&issues), vec![IdIssueKind::PrefixAmbiguity]);
        assert!(has_errors(&issues));
        cleanup(&d);
    }

    #[test]
    fn check_缺id标记告警() {
        let d = temp_dir("ids-noid");
        w(&d, "随手笔记.md", None);
        let issues = check(&d).unwrap();
        assert_eq!(kinds(&issues), vec![IdIssueKind::MissingId]);
        assert!(!has_errors(&issues));
        cleanup(&d);
    }

    #[test]
    fn lint_推荐档与存量档零提示() {
        for id in [
            "REQ-kd-20260919-A7F3",
            "REQ-kd-007",
            "REQ-pay-kd-20260919-A7F3",
            "1",
            "REQ-001",
        ] {
            assert!(lint_id(id).is_empty(), "{} → {:?}", id, lint_id(id));
        }
    }

    #[test]
    fn lint_日期开头超长与非推荐档() {
        let a = lint_id("REQ-20260919-A7F3");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, IdIssueKind::Pollution);

        let b = lint_id("REQ-kd-20260919-A7F3-aaaaaaaaaaaaa");
        assert!(b.iter().any(|i| i.message.contains("超过 32")), "{:?}", b);

        let c = lint_id("login-refactor");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, IdIssueKind::Format);
    }

    #[test]
    fn lint_大小写写法违规() {
        let a = lint_id("REQ-KD-20260919-a7f3");
        assert_eq!(a.len(), 2, "{:?}", a);
        assert!(a.iter().all(|i| i.kind == IdIssueKind::Format));

        let m = lint_id("REQ-PAY-kd-20260919-A7F3");
        assert_eq!(m.len(), 1, "{:?}", m);
        assert!(m[0].message.contains("模块"));
    }

    #[test]
    fn lint_空串与纯数字无提示() {
        assert!(lint_id("").is_empty());
        assert!(lint_id("  42  ").is_empty());
    }
}

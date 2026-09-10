//! AI 需求门禁：需求清单的生命周期管理。
//!
//! ## 目标
//! AI 在项目中实现新需求前，必须按步骤完成 **需求分解 → 技术方案 → 测试计划** 三段清单，
//! 且每段由审核人显式 `approved`，才允许进入代码开发（硬拦截见 [`crate::gate`]）。
//!
//! ## 文件格式
//! 清单是**人机双读**的 Markdown：
//! - 给人看：三段正文 + 审核记录；
//! - 给脚本看：`<!-- GATE:HEAD ... -->` / `<!-- GATE:STEP ... -->` 机读标记行，
//!   由 `.gates/hooks/req-guard-check.{sh,ps1}` 用 grep/sed 解析。
//!
//! 约定：**正文可随意编辑，GATE 标记行只能由本模块改写**（`req-guard approve`），
//! 避免 AI 自行把状态改成 approved 来绕过门禁。

use crate::error::{GateError, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// 三段清单步骤：`(步骤键, 中文名)`。**数组顺序即默认强制审核顺序**。
pub const STEPS: [(&str, &str); 3] = [
    ("decomposition", "需求分解"),
    ("solution", "技术方案"),
    ("testplan", "测试计划"),
];

/// 清单存放目录（相对项目根）。
pub const REQ_DIR: &str = ".gates/requirements";

/// 审核评论文件后缀。评论与清单正文**分离存放**（AI 禁止直接写入评论文件）。
///
/// 定义放在这里是因为它属于"文件命名约定"，而 `list()` 必须据此把评论文件
/// 从需求列表中排除——否则 `REQ-001.comments.md` 会被当成一条 id 为空的幽灵需求，
/// 拦截脚本也可能选中它（正文里没有 GATE:STEP → 三步全 pending → 恒拦截）。
pub const COMMENTS_SUFFIX: &str = ".comments.md";

/// 一个需求清单文件。
#[derive(Debug, Clone)]
pub struct Requirement {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
}

/// 步骤键 → 中文名。
pub fn step_label(step: &str) -> &'static str {
    for s in STEPS {
        if s.0 == step {
            return s.1;
        }
    }
    "未知步骤"
}

/// 校验步骤键合法。
pub fn validate_step(step: &str) -> Result<()> {
    if STEPS.iter().any(|s| s.0 == step) {
        Ok(())
    } else {
        let opts: Vec<&str> = STEPS.iter().map(|s| s.0).collect();
        Err(GateError::Validation(format!(
            "未知审核步骤: {}（可选 {}）",
            step,
            opts.join(" | ")
        )))
    }
}

/// 创建新需求清单，返回其元信息。
///
/// `id` 为 `None` 时按目录内最大编号自增（`REQ-001` → `REQ-002`）。
pub fn create(root: &Path, id: Option<&str>, title: &str) -> Result<Requirement> {
    let title = title.trim();
    if title.is_empty() {
        return Err(GateError::Validation(
            "需求标题不能为空（-t/--title）".into(),
        ));
    }
    let dir = root.join(REQ_DIR);
    fs::create_dir_all(&dir).map_err(|e| GateError::Io {
        path: Some(dir.clone()),
        source: e,
    })?;

    let id = match id {
        Some(i) => normalize_id(i)?,
        None => next_id(&dir)?,
    };
    let slug = ascii_slug(title);
    let fname = if slug.is_empty() {
        format!("{}.md", id)
    } else {
        format!("{}-{}.md", id, slug)
    };
    let path = dir.join(&fname);
    if path.exists() {
        return Err(GateError::Validation(format!(
            "需求清单已存在: {}（换个 --id 或先归档旧需求）",
            path.display()
        )));
    }

    fs::write(&path, render(&id, title)).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    Ok(Requirement {
        id,
        title: title.to_string(),
        path,
    })
}

/// 列出全部需求清单（按文件名字典序）。
pub fn list(root: &Path) -> Result<Vec<Requirement>> {
    let dir = root.join(REQ_DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| GateError::Io {
        path: Some(dir.clone()),
        source: e,
    })?;
    for e in entries {
        let e = e.map_err(|e| GateError::Io {
            path: Some(dir.clone()),
            source: e,
        })?;
        let path = e.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.ends_with(".md") || name == ".gitkeep" || name.ends_with(COMMENTS_SUFFIX) {
            continue;
        }
        let content = fs::read_to_string(&path).map_err(|e| GateError::Io {
            path: Some(path.clone()),
            source: e,
        })?;
        let id = token(head_line(&content).unwrap_or(""), "id");
        let mut title =
            first_heading(&content).unwrap_or_else(|| name.trim_end_matches(".md").to_string());
        // 一级标题按模板写作 `# REQ-001 标题`，显示时去掉重复的编号前缀。
        if !id.is_empty() {
            if let Some(rest) = title.strip_prefix(&format!("{} ", id)) {
                title = rest.trim().to_string();
            }
        }
        out.push(Requirement { id, title, path });
    }
    out.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    Ok(out)
}

/// 按 ID 定位需求（支持 `REQ-001` 精确匹配文件名 `REQ-001.md` 或 `REQ-001-<slug>.md`）。
pub fn find(root: &Path, id: &str) -> Result<Requirement> {
    let exact = format!("{}.md", id);
    let prefix = format!("{}-", id);
    for r in list(root)? {
        let name = r
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name == exact || name.starts_with(&prefix) {
            return Ok(r);
        }
    }
    Err(GateError::Validation(format!(
        "未找到需求 {}（查找目录: {}）",
        id,
        root.join(REQ_DIR).display()
    )))
}

/// 审核某一阶段：`pass=true` 通过，`pass=false` 打回。
///
/// - `strict=true` 时强制顺序：审核后一步前，其前置步骤必须已 `approved`；
/// - 审核记录（时间/审核人/结论/原因）追加到清单的"审核记录"区块；
/// - 三步全部 `approved` 时，清单整体状态置为 `approved`，门禁解锁。
pub fn review(
    root: &Path,
    id: &str,
    step: &str,
    reviewer: &str,
    pass: bool,
    reason: &str,
    strict: bool,
) -> Result<Requirement> {
    validate_step(step)?;
    let r = find(root, id)?;
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;

    if strict {
        for s in STEPS {
            if s.0 == step {
                break;
            }
            let st = step_status(&content, s.0);
            if st != "approved" {
                return Err(GateError::Validation(format!(
                    "审核顺序未满足：审核 `{}`({}) 之前，前置步骤 `{}`({}) 必须已 approved（当前: {}）。\
                     如需跳过顺序约束，请在 .gates/req-guard.yaml 中将 strict_order 设为 false",
                    step,
                    step_label(step),
                    s.0,
                    s.1,
                    if st.is_empty() { "pending" } else { &st }
                )));
            }
        }
    }

    let new_status = if pass { "approved" } else { "rejected" };
    let ts = safe_field(&crate::gate::now_str());
    let rv = safe_field(reviewer);

    let mut out = String::new();
    for line in content.lines() {
        if line.contains("GATE:STEP") && token(line, "name") == step {
            let label = token(line, "label");
            out.push_str(&format!(
                "<!-- GATE:STEP name={} label={} status={} reviewer={} updated={} -->\n",
                step, label, new_status, rv, ts
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out = set_head_status(&out, &recompute_head(&out));
    out = append_audit(
        &out,
        &format!(
            "- {} | {} | {} | {} | {}\n",
            ts,
            rv,
            step,
            new_status,
            if reason.trim().is_empty() {
                "-"
            } else {
                reason.trim()
            }
        ),
    );

    fs::write(&r.path, out).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    Ok(r)
}

/// 打印单个或多个需求的状态（含是否解锁）。
pub fn print_status(root: &Path, id: Option<&str>) -> Result<()> {
    let reqs = match id {
        Some(i) => vec![find(root, i)?],
        None => list(root)?,
    };
    if reqs.is_empty() {
        println!("尚未创建任何需求清单。");
        println!("  创建：req-guard create -t \"<需求标题>\"");
        return Ok(());
    }
    for r in reqs {
        let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
            path: Some(r.path.clone()),
            source: e,
        })?;
        let head = head_status(&content);
        println!("需求 {} {}", r.id, r.title);
        println!("  文件 : {}", r.path.display());
        println!("  状态 : {}", head);
        println!("  步骤 :");
        let mut all_ok = true;
        for s in STEPS {
            let st = step_status(&content, s.0);
            let rv = step_reviewer(&content, s.0);
            let mark = if st == "approved" { "✓" } else { " " };
            if st != "approved" {
                all_ok = false;
            }
            let shown = if st.is_empty() {
                "pending".to_string()
            } else {
                st
            };
            if rv.is_empty() || rv == "-" {
                println!("    [{}] {:<8} {}", mark, s.0, shown);
            } else {
                println!("    [{}] {:<8} {}（审核人: {}）", mark, s.0, shown, rv);
            }
        }
        if all_ok {
            println!("  判定 : 已解锁，AI 可以开始编写代码");
        } else {
            println!("  判定 : 未解锁，AI 不得编写/修改源码（门禁拦截）");
        }
        println!();
    }
    Ok(())
}

// ===================== 解析辅助 =====================

/// 取 GATE 标记行中 `key=value` 的值；未命中返回空串。
pub fn token(line: &str, key: &str) -> String {
    let prefix = format!("{}=", key);
    for t in line.split_whitespace() {
        if let Some(v) = t.strip_prefix(&prefix) {
            return v.to_string();
        }
    }
    String::new()
}

fn head_line(content: &str) -> Option<&str> {
    content.lines().find(|l| l.contains("GATE:HEAD"))
}

fn step_line<'a>(content: &'a str, step: &str) -> Option<&'a str> {
    content
        .lines()
        .find(|l| l.contains("GATE:STEP") && token(l, "name") == step)
}

/// 清单整体状态。
pub fn head_status(content: &str) -> String {
    match head_line(content) {
        Some(l) => {
            let s = token(l, "status");
            if s.is_empty() {
                "draft".to_string()
            } else {
                s
            }
        }
        None => "-".to_string(),
    }
}

/// 指定步骤的审核状态。
pub fn step_status(content: &str, step: &str) -> String {
    step_line(content, step)
        .map(|l| token(l, "status"))
        .unwrap_or_default()
}

/// 指定步骤的审核人。
pub fn step_reviewer(content: &str, step: &str) -> String {
    step_line(content, step)
        .map(|l| token(l, "reviewer"))
        .unwrap_or_default()
}

fn set_head_status(content: &str, status: &str) -> String {
    let mut out = String::new();
    for line in content.lines() {
        if line.contains("GATE:HEAD") {
            let id = token(line, "id");
            let created = token(line, "created");
            out.push_str(&format!(
                "<!-- GATE:HEAD id={} status={} created={} -->\n",
                id, status, created
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 三步全 approved → `approved`；任一 rejected → `changes_requested`；否则 `in_review`。
fn recompute_head(content: &str) -> String {
    let mut has_reject = false;
    let mut all_ok = true;
    for s in STEPS {
        match step_status(content, s.0).as_str() {
            "approved" => {}
            "rejected" => {
                has_reject = true;
                all_ok = false;
            }
            _ => all_ok = false,
        }
    }
    if all_ok {
        "approved".to_string()
    } else if has_reject {
        "changes_requested".to_string()
    } else {
        "in_review".to_string()
    }
}

fn append_audit(content: &str, entry: &str) -> String {
    let end = "<!-- /GATE:AUDIT -->";
    if let Some(pos) = content.find(end) {
        let mut out = content[..pos].to_string();
        out.push_str(entry);
        out.push_str(&content[pos..]);
        out
    } else {
        let mut out = content.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\n## 审核记录\n\n<!-- GATE:AUDIT -->\n");
        out.push_str(entry);
        out.push_str(end);
        out.push('\n');
        out
    }
}

fn first_heading(content: &str) -> Option<String> {
    content
        .lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l.trim_start_matches("# ").trim().to_string())
}

// ===================== 工具函数 =====================

/// GATE 标记行里禁止出现空白（否则 grep/awk 解析会断裂），统一替换为 `_`。
fn safe_field(s: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join("_");
    if t.is_empty() {
        "-".to_string()
    } else {
        t
    }
}

/// 归一需求编号：`1` → `REQ-001`；其余要求仅含字母/数字/连字符/下划线。
fn normalize_id(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(GateError::Validation("需求编号不能为空".into()));
    }
    if s.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = s
            .parse()
            .map_err(|_| GateError::Validation(format!("需求编号超出范围: {}", s)))?;
        return Ok(format!("REQ-{:03}", n));
    }
    let ok = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(GateError::Validation(
            "需求编号仅可含字母/数字/连字符/下划线（如 REQ-001 或 login-refactor）".into(),
        ));
    }
    Ok(s.to_string())
}

/// 扫描目录内 `REQ-<n>*` 取最大编号 +1。
fn next_id(dir: &Path) -> Result<String> {
    let mut max = 0u32;
    let entries = fs::read_dir(dir).map_err(|e| GateError::Io {
        path: Some(dir.to_path_buf()),
        source: e,
    })?;
    for e in entries {
        let e = e.map_err(|e| GateError::Io {
            path: Some(dir.to_path_buf()),
            source: e,
        })?;
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(rest) = name.strip_prefix("REQ-") {
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num.parse::<u32>() {
                if n > max {
                    max = n;
                }
            }
        }
    }
    Ok(format!("REQ-{:03}", max + 1))
}

/// 标题转 ASCII 文件名片段（中文会被过滤，结果可能为空）。
fn ascii_slug(title: &str) -> String {
    let s: String = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    s.trim_matches('-').to_ascii_lowercase()
}

// ===================== 清单模板 =====================

fn render(id: &str, title: &str) -> String {
    let ts = safe_field(&crate::gate::now_str());
    let mut s = String::new();
    s.push_str(&format!("# {} {}\n\n", id, title));
    s.push_str("> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。\n");
    s.push_str("> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；\n");
    s.push_str("> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。\n\n");
    s.push_str(&format!(
        "<!-- GATE:HEAD id={} status=draft created={} -->\n",
        id, ts
    ));
    for st in STEPS {
        s.push_str(&format!(
            "<!-- GATE:STEP name={} label={} status=pending reviewer=- updated=- -->\n",
            st.0, st.1
        ));
    }
    s.push('\n');
    s.push_str("## 1. 需求分解\n\n");
    s.push_str(DECOMPOSITION_BODY);
    s.push_str("\n## 2. 技术方案\n\n");
    s.push_str(SOLUTION_BODY);
    s.push_str("\n## 3. 测试计划\n\n");
    s.push_str(TESTPLAN_BODY);
    s.push_str("\n## 审核记录\n\n<!-- GATE:AUDIT -->\n<!-- /GATE:AUDIT -->\n");
    s
}

const DECOMPOSITION_BODY: &str = "\
- [ ] 背景与问题：为什么要做这件事
- [ ] 目标 / 非目标（明确不做什么）
- [ ] 子任务拆解（编号 + 预估工时）
- [ ] 影响范围（涉及模块 / 接口 / 数据表）
- [ ] 验收标准（可度量、可判定）
";

const SOLUTION_BODY: &str = "\
- [ ] 总体思路（一句话说清怎么做）
- [ ] 关键设计：数据结构 / 接口签名 / 调用流程
- [ ] 涉及的文件与模块清单
- [ ] 兼容性、性能与安全影响
- [ ] 风险点与回滚方案
";

const TESTPLAN_BODY: &str = "\
- [ ] 单元测试用例（编号 + 断言点）
- [ ] 端到端用例（编号 + 执行步骤）
- [ ] 边界 / 异常 / 并发场景
- [ ] 回归范围与影响面
- [ ] 覆盖率要求与验收门槛
";

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cleanup, temp_dir};

    /// 基于真实模板造出指定三段状态的清单正文。
    fn content_with(states: [&str; 3]) -> String {
        let mut c = render("REQ-001", "测试需求");
        for (s, st) in STEPS.iter().zip(states) {
            let from = format!("name={} label={} status=pending", s.0, s.1);
            let to = format!("name={} label={} status={}", s.0, s.1, st);
            c = c.replace(&from, &to);
        }
        c
    }

    #[test]
    fn token_解析键值且缺失返回空() {
        let l = "<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=寇工 updated=2026-09-11_0100 -->";
        assert_eq!(token(l, "name"), "solution");
        assert_eq!(token(l, "status"), "approved");
        assert_eq!(token(l, "reviewer"), "寇工");
        assert_eq!(token(l, "不存在"), "");
    }

    #[test]
    fn normalize_id_补零与非法拒绝() {
        assert_eq!(normalize_id("1").unwrap(), "REQ-001");
        assert_eq!(normalize_id("42").unwrap(), "REQ-042");
        assert_eq!(normalize_id("login-refactor").unwrap(), "login-refactor");
        assert!(normalize_id("bad id").is_err(), "含空格应被拒");
        assert!(normalize_id("REQ/001").is_err(), "含斜杠应被拒");
        assert!(normalize_id("   ").is_err());
    }

    #[test]
    fn safe_field_空白转下划线() {
        assert_eq!(safe_field("寇 工"), "寇_工");
        assert_eq!(safe_field("单值"), "单值");
        assert_eq!(safe_field("   "), "-");
    }

    #[test]
    fn ascii_slug_丢弃中文只留_ascii() {
        assert_eq!(ascii_slug("User Login Fix"), "user-login-fix");
        assert_eq!(ascii_slug("用户登录改造"), "");
    }

    #[test]
    fn recompute_head_三步状态机() {
        assert_eq!(recompute_head(&content_with(["approved"; 3])), "approved");
        assert_eq!(
            recompute_head(&content_with(["approved", "rejected", "pending"])),
            "changes_requested"
        );
        assert_eq!(
            recompute_head(&content_with(["approved", "pending", "pending"])),
            "in_review"
        );
        assert_eq!(recompute_head(&render("REQ-001", "草稿")), "in_review");
    }

    #[test]
    fn next_id_取目录内最大编号加一() {
        let d = temp_dir("req-next-id");
        assert_eq!(next_id(&d).unwrap(), "REQ-001");
        fs::write(d.join("REQ-001.md"), "").unwrap();
        fs::write(d.join("REQ-007.md"), "").unwrap();
        fs::write(d.join("REQ-007.comments.md"), "").unwrap();
        assert_eq!(next_id(&d).unwrap(), "REQ-008");
        cleanup(&d);
    }

    #[test]
    fn list_排除评论文件并去掉标题编号() {
        let root = temp_dir("req-list");
        let r = create(&root, None, "用户登录改造").unwrap();
        assert_eq!(r.id, "REQ-001");
        // 模拟 req-guard comment 之后的评论文件
        fs::write(
            r.path.with_file_name(format!("REQ-001{}", COMMENTS_SUFFIX)),
            "# REQ-001 审核评论\n",
        )
        .unwrap();

        let items = list(&root).unwrap();
        assert_eq!(items.len(), 1, "评论文件不得被当成需求列出");
        assert_eq!(items[0].id, "REQ-001");
        assert_eq!(items[0].title, "用户登录改造", "标题不应重复编号前缀");
        cleanup(&root);
    }

    #[test]
    fn review_强制顺序与放行() {
        let root = temp_dir("req-review");
        create(&root, None, "顺序校验").unwrap();

        let e = review(&root, "REQ-001", "solution", "寇工", true, "", true).unwrap_err();
        assert!(
            e.to_string().contains("审核顺序未满足"),
            "跳步应被拒：{}",
            e
        );

        review(&root, "REQ-001", "decomposition", "寇工", true, "", true).unwrap();
        let e = review(&root, "REQ-001", "testplan", "寇工", true, "", true).unwrap_err();
        assert!(
            e.to_string().contains("审核顺序未满足"),
            "前置未过应被拒：{}",
            e
        );

        // strict_order=false 时允许跳步
        review(&root, "REQ-001", "testplan", "寇工", true, "", false).unwrap();
        review(&root, "REQ-001", "solution", "寇工", true, "", false).unwrap();

        let r = find(&root, "REQ-001").unwrap();
        let content = fs::read_to_string(&r.path).unwrap();
        assert_eq!(step_status(&content, "testplan"), "approved");
        assert_eq!(head_status(&content), "approved", "三步齐备应整体解锁");
        assert!(content.contains("## 审核记录"), "应写入审核记录区块");
        cleanup(&root);
    }

    #[test]
    fn create_拒绝重复编号() {
        let root = temp_dir("req-create");
        create(&root, Some("REQ-001"), "第一个").unwrap();
        assert!(create(&root, Some("REQ-001"), "重复").is_err());
        assert!(create(&root, None, "   ").is_err(), "空标题应被拒");
        cleanup(&root);
    }
}

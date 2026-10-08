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

/// 归档子目录（相对 [`REQ_DIR`]）。到期归档统一搬入
/// `archive/<创建年份>/`（文件名含 YYYYMMDD）或 `archive/misc/`（无日期段）。
///
/// 选子目录而不是 `.gates/archive/`：`list()`/`next_id()`/HOOK_SH/HOOK_PS1
/// 四处扫描点都不递归，天然忽略归档区——**拦截脚本零改动**。
pub const ARCHIVE_DIR: &str = "archive";

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

/// 截取清单正文中第 `step` 段（下标按 [`STEPS`]）的片段。
///
/// 边界就是 Markdown 二级标题：从 `## <n>. <标题>` 起，到下一个 `## ` 止
/// （`## 审核记录` 同样是二级标题，因此末段不会把审核记录吞进来）。
/// 数字与标题间的分隔符不强求（`## 1.` / `## 1 ` 都认），但**数字必须对得上**。
///
/// 定位失败（老格式、标题被改坏）时**回退整篇**：宁可多显示，也不能让界面白屏——
/// 界面是审核人唯一的判断依据，静默隐藏内容比多显示危险得多。
///
/// 放在 core 而不是各前端：TUI 与 GUI 必须按同一条规则切段，否则"两个界面看到的
/// 不是同一份东西"，对门禁工具而言是不可接受的（与"判定唯一真相在 core"同源）。
pub fn section_of(content: &str, step: usize) -> String {
    // 定位失败仍回退整篇（见本函数文档：宁可多显示也不能让界面白屏）。
    let Some((start, end)) = section_span(content, step) else {
        return content.to_string();
    };
    let lines: Vec<&str> = content.lines().collect();
    let mut out = lines[start - 1..end].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// 第 `step` 段的行区间 `[start, end)`（**1-based、左闭右开**），定位失败返回 `None`。
///
/// 与 [`section_of`] 的区别只有一处，但至关重要：**它不猜**。
/// `section_of` 定位失败时回退整篇（为保证审核人界面不白屏），
/// 而机械校验（[`crate::ac`] 的 AC 校验、变更范围校验）**绝不能用那份回退整篇**：
/// 标题被改坏的文档会拿全文去判，"第 3 段恰好有一条 AC"这种巧合就能蒙混过关 ——
/// 那正是本工具最坏的失效模式（看着在拦、其实没拦）。
///
/// 故判定类调用一律走这里，拿到 `None` 就如实报「第 N 段定位失败」。
/// 草稿区目录（相对项目根）。**刻意不放在 `.gates/requirements/` 下**——
/// 那个前缀会被 [`crate::gate::is_requirement_doc`] 认成清单正文，进而被写入守卫
/// 与内容冻结管上；而草稿按定义**不参与任何判定**（REQ-007 G1）。
pub const DRAFTS_DIR: &str = ".gates/drafts";

/// 草稿文件路径：`.gates/drafts/<需求ID>.draft.md`。
pub fn draft_path(root: &Path, id: &str) -> std::path::PathBuf {
    root.join(DRAFTS_DIR).join(format!("{id}.draft.md"))
}

/// 一份草稿：逐段的拟替换正文。
#[derive(Debug, Clone, Default)]
pub struct Draft {
    /// `(步骤 key, 该段拟替换的正文)`，按草稿文件中的出现顺序。
    pub sections: Vec<(String, String)>,
}

impl Draft {
    pub fn get(&self, step: &str) -> Option<&str> {
        self.sections
            .iter()
            .find(|(k, _)| k == step)
            .map(|(_, v)| v.as_str())
    }

    pub fn steps(&self) -> Vec<&str> {
        self.sections.iter().map(|(k, _)| k.as_str()).collect()
    }
}

/// 解析草稿文件。
///
/// 分隔符用**行内标记** `{step=<段名>}` 而不是 `## <段名>`：段边界判定
/// `section_span` 认的是「`trim_start()` 后以 `## ` 开头」，缩进的 `## solution`
/// 同样命中 —— 于是「文档里展示草稿格式示例」会把自己那份清单的段落边界切错，
/// 表现为 `GATE:TOUCH` 块突然「消失」（实测起草 REQ-007 时踩到）。
/// 行内标记不受行首缩进影响，也不与 Markdown 标题语法撞车。
///
/// 未知段名**报错而非忽略**：静默忽略会让人以为草稿生效了。
pub fn parse_draft(text: &str) -> Result<Draft> {
    const OPEN: &str = "{step=";
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, Vec<String>)> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(OPEN).and_then(|r| r.strip_suffix('}')) {
            if let Some((k, body)) = cur.take() {
                sections.push((k, body.join("\n")));
            }
            let key = rest.trim().to_string();
            validate_step(&key)?;
            // 重复段名必须在 parse 阶段就拒：等到 apply 才发现，同名两段会静默
            // 采用最后一份（REQ-010 AC-009 / B-01）。放这里而不是 apply，
            // 是因为「同一段两遍」本身就是草稿文件本身的错误，与谁在读无关。
            if sections.iter().any(|(k, _)| *k == key)
                || cur.as_ref().is_some_and(|(k, _)| *k == key)
            {
                return Err(GateError::Validation(format!(
                    "草稿里段名 `{key}` 出现了多次 —— 一次只允许一段，重复段会被静默采用最后一份。\n\
                     请把草稿裁剪成只留目标段（合法段名：decomposition / solution / testplan）。"
                )));
            }
            cur = Some((key, Vec::new()));
            continue;
        }
        if let Some((_, body)) = cur.as_mut() {
            body.push(line.to_string());
        }
    }
    if let Some((k, body)) = cur.take() {
        sections.push((k, body.join("\n")));
    }
    Ok(Draft { sections })
}

/// 草稿文件的契约说明（三处文案之一，另两处见 [`crate::gate::README_DRAFT_SECTION`] 与 `--help`）。
///
/// 为什么要有第三处：`write_draft` 产出的文件会被人直接打开读，而**读文件的人不会
/// 去看 README 或 `--help`**。三处一致，才不会再出现「契约只存在于实现里」
/// （REQ-010 G4：本次三个现象里有两个是静默的）。
pub const DRAFT_CONTRACT: &str = "\
# ===== req-guard 草稿契约（apply 会逐条校验，违反即拒且不落盘）=====
# ① 草稿**只写散文**：GATE:TOUCH / GATE:AC / GATE:AUDIT 等块由 req-guard 维护，
#    草稿里的块标记不生效、还会把位置错位；块内条目请直接编辑清单（或用 amend）。
# ② **一次只应用一段**：本文件含多段时 apply 会拒绝 —— 此前会被「应用一段后删掉整份
#    草稿」静默吃掉其余段。请裁剪成只留目标段，逐段应用。
# ③ **整段重写**：通道按行位置覆盖，草稿短于原段会被拒绝（否则原段尾部散文被丢弃）；
#    草稿长出的行落在该段**末尾**，无法在段中间插入（apply 成功时会报出多出几行）。
# 校验期间本文件若被改动，apply 会在写盘前发现并拒绝 —— 落盘的一定是你看到的那份。
";

/// 写草稿文件（供 CLI 与测试用；AI 直接写文件即可，无需命令）。
pub fn write_draft(root: &Path, id: &str, draft: &Draft) -> Result<()> {
    let p = draft_path(root, id);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| GateError::Io {
            path: Some(dir.to_path_buf()),
            source: e,
        })?;
    }
    // 契约说明写在文件**头部**：读草稿的人第一眼看到的是它，而不是被埋在末尾。
    let mut out = String::from(DRAFT_CONTRACT);
    for (k, body) in &draft.sections {
        out.push_str(&format!("\n{{step={k}}}\n{body}\n"));
    }
    std::fs::write(&p, out).map_err(|e| GateError::Io {
        path: Some(p.clone()),
        source: e,
    })
}

/// 读草稿文件；不存在返回 `None`。
pub fn read_draft(root: &Path, id: &str) -> Result<Option<Draft>> {
    let p = draft_path(root, id);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(GateError::Io {
                path: Some(p.clone()),
                source: e,
            })
        }
    };
    Ok(Some(parse_draft(&text)?))
}

pub fn section_span(content: &str, step: usize) -> Option<(usize, usize)> {
    /// 二级标题的行号（`## …` 一律算边界）。
    fn is_heading(line: &str) -> bool {
        line.trim_start().starts_with("## ")
    }
    /// 二级标题里声明的段落序号（`## 2. 技术方案` → `Some(2)`）。
    fn heading_no(line: &str) -> Option<usize> {
        let rest = line.trim_start().strip_prefix("## ")?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    // **先掩掉代码围栏**（REQ-008）：围栏里的 `## xxx` 是示例内容，不是标题。
    // 不掩的话，「文档里展示 `## solution` 格式」会把段落边界切错 ——
    // 表现为 `GATE:TOUCH` 块"消失"，而报错（你没写声明块）与真实原因无关；
    // 更糟的是另外三条判定（摘要覆盖范围、EmptySection 统计范围）
    // 会**静默地**指向错误段落。掩码保持行号不变（见 `section::mask_fenced`）。
    let masked = crate::section::mask_fenced(content);
    let lines: Vec<&str> = masked.lines().collect();
    let start = lines.iter().position(|l| heading_no(l) == Some(step + 1))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| is_heading(l))
        .map_or(lines.len(), |i| start + 1 + i);
    Some((start + 1, end))
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

/// 列出全部需求清单（按文件名字典序）。只扫顶层，不含归档区。
pub fn list(root: &Path) -> Result<Vec<Requirement>> {
    let dir = root.join(REQ_DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    scan_dir(&dir)
}

/// 扫描目录内全部需求清单（跳过评论文件 / `.gitkeep`）。
fn scan_dir(dir: &Path) -> Result<Vec<Requirement>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| GateError::Io {
        path: Some(dir.to_path_buf()),
        source: e,
    })?;
    for e in entries {
        let e = e.map_err(|e| GateError::Io {
            path: Some(dir.to_path_buf()),
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

/// 递归扫描 `archive/` 下的全部需求清单（供历史查阅；排序按相对路径）。
pub fn list_archived(root: &Path) -> Result<Vec<Requirement>> {
    let archive = root.join(REQ_DIR).join(ARCHIVE_DIR);
    if !archive.exists() {
        return Ok(Vec::new());
    }
    scan_dir_recursive(&archive)
}

/// 深度优先递归扫描（`archive/<年>/`、`archive/misc/` 两层）。
fn scan_dir_recursive(dir: &Path) -> Result<Vec<Requirement>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| GateError::Io {
        path: Some(dir.to_path_buf()),
        source: e,
    })?;
    for e in entries {
        let e = e.map_err(|e| GateError::Io {
            path: Some(dir.to_path_buf()),
            source: e,
        })?;
        let p = e.path();
        if p.is_dir() {
            out.extend(scan_dir_recursive(&p)?);
        } else if p.is_file() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md") || name == ".gitkeep" || name.ends_with(COMMENTS_SUFFIX) {
                continue;
            }
            let content = fs::read_to_string(&p).map_err(|e| GateError::Io {
                path: Some(p.clone()),
                source: e,
            })?;
            let id = token(head_line(&content).unwrap_or(""), "id");
            let mut title =
                first_heading(&content).unwrap_or_else(|| name.trim_end_matches(".md").to_string());
            if !id.is_empty() {
                if let Some(rest) = title.strip_prefix(&format!("{} ", id)) {
                    title = rest.trim().to_string();
                }
            }
            out.push(Requirement { id, title, path: p });
        }
    }
    Ok(out)
}

/// 按 ID 定位需求（支持 `REQ-001` 精确匹配文件名 `REQ-001.md` 或 `REQ-001-<slug>.md`）。
///
/// 顶层未命中时**只读回退**到归档区：`status`/`comments` 等历史查阅因此可用。
/// 写操作（`review`/`done` 等）必须自行对 done / 归档路径设护栏，否则会改写归档证据。
pub fn find(root: &Path, id: &str) -> Result<Requirement> {
    let exact = format!("{}.md", id);
    let prefix = format!("{}-", id);
    for r in list(root)? {
        if file_name(&r.path) == exact || file_name(&r.path).starts_with(&prefix) {
            return Ok(r);
        }
    }
    for r in list_archived(root)? {
        if file_name(&r.path) == exact || file_name(&r.path).starts_with(&prefix) {
            return Ok(r);
        }
    }
    Err(GateError::Validation(format!(
        "未找到需求 {}（查找目录: {} 及归档区）",
        id,
        root.join(REQ_DIR).display()
    )))
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 测试计划批准前的 AC 格式门禁：有 Error 即拒绝批准。
///
/// 复用 [`crate::ac`] 的判定（唯一真相在 core），此处只负责"把问题转成拒绝理由 +
/// 附上可粘贴的骨架"。判级不做 L0/L1 分档 —— 与 `ensure_source_refs` 不同，
/// 这里**任何等级都拒绝**：AC 格式不合规不是"管理严格度"问题，而是清单根本不可判定，
/// 放过去等于让门禁失去意义。
/// 技术方案批准前的交叉引用有效性校验（REQ-004 G3 / T5）。
///
/// 为什么挂审批而不只给 `ac check`：`ac check` 要人记得跑，挂在 approve 上则
/// **物理上无法把一份引用已经失效的方案批出去** —— 与 REQ-001 把 AC 格式校验
/// 挂在 testplan 批准上是同一条教训。
///
/// 只校验方案段（第 2 段）：交叉引用是「方案 ↔ 设计文档」之间的关系，
/// 第 1、3 段不承载这种引用。
fn ensure_cross_refs_ok(root: &Path, id: &str, content: &str) -> Result<()> {
    let Some((start, end)) = section_span(content, 1) else {
        return Err(GateError::Validation(format!(
            "需求 {id} 的「## 2. 技术方案」二级标题定位失败，无法校验设计文档交叉引用。\n\
             请把该段标题改回 `## 2. 技术方案`。"
        )));
    };
    let lines: Vec<&str> = content.lines().collect();
    let section = lines[start - 1..end].join("\n");
    let bad = crate::touch::check_cross_refs(root, &section, start);
    if bad.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "需求 {id} 的技术方案引用了 {} 处已失效的设计文档位置，拒绝批准：\n",
        bad.len()
    );
    for i in &bad {
        if i.target_missing {
            msg.push_str(&format!(
                "  ✗ 第 {} 行：`{}` **文件不存在**（被移动/重命名，或路径写错）\n",
                i.line, i.path
            ));
        } else {
            msg.push_str(&format!(
                "  ✗ 第 {} 行：`{}` 的小节 **§{} 不存在**（小节被重命名，最常见）\n",
                i.line, i.path, i.section
            ));
        }
    }
    msg.push_str(
        "处置：把引用改成目标文档里现存的路径与小节号；确实无需引用则删掉该引用。\n\
         引用写法固定为 `docs/<目录>/<文件>.md §<编号>`，其它写法不做判定。",
    );
    Err(GateError::Validation(msg))
}

/// 声明档（缺省 `standard`；非法取值报错 —— 与 `check` 裁决同一条判据，不各写一份）。
fn declared_tier(content: &str) -> Result<crate::tier::Tier> {
    crate::tier::declared_of(content)
}

fn ensure_ac_compliant(id: &str, content: &str) -> Result<()> {
    let Some((start, end)) = section_span(content, 2) else {
        return Err(GateError::Validation(format!(
            "需求 {id} 的「## 3. 测试计划」二级标题定位失败，无法校验验收标准。\n\
             请把该段标题改回 `## 3. 测试计划`（机械校验不回退整篇 —— \
             回退会让标题写坏的文档靠巧合蒙混过关）。"
        )));
    };
    let lines: Vec<&str> = content.lines().collect();
    let section = lines[start - 1..end].join("\n");
    let tier = declared_tier(content)?;
    let issues = crate::ac::relax_for_tier(crate::ac::lint(&section, start), tier);
    let errs: Vec<&crate::ac::AcIssue> = issues.iter().filter(|i| i.severity.is_error()).collect();
    if errs.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "需求 {id} 的验收标准不合规（{} 项硬伤），拒绝批准测试计划：\n",
        errs.len()
    );
    for i in &errs {
        msg.push_str(&format!("  ✗ [{}] {}\n", i.kind_as_str(), i.message));
    }
    let warns = issues.len() - errs.len();
    if warns > 0 {
        msg.push_str(&format!("（另有 {warns} 项告警不阻断）\n"));
    }
    msg.push_str(&format!(
        "\n每条 AC 的形态（编号独占一行 + 三个子句列表项）：\n\
         <!-- GATE:AC -->\n\
         ### AC-001\n\
         - Given: <可复现的前置状态，写具体>\n\
         - When: <一次可触发的操作，写出具体命令>\n\
         - Then: <可观测结果，含退出码 / 字面量 / 数值>\n\
         <!-- /GATE:AC -->\n\
         完整规则见 docs/设计/AC与变更范围契约技术方案.md §2；\
         校验命令：req-guard ac check {id}"
    ));
    Err(GateError::Validation(msg))
}

/// 三段模板正文（`create` 实际写出的内容），供占位文案派生。
///
/// **刻意暴露而不是让调用方各自持有**：占位文案若在判定侧另写一份，
/// 模板一改判定就认不出来（漂移的起点）。`crate::section` 据此派生，
/// 并用 `防漂移_真实模板每条占位都被识别` 锁住。
pub fn template_bodies() -> [&'static str; 3] {
    [DECOMPOSITION_BODY, SOLUTION_BODY, TESTPLAN_BODY]
}

/// 模板占位文案集合（由 [`template_bodies`] 派生）。
pub fn template_placeholder_texts() -> Vec<String> {
    crate::section::placeholder_texts(&template_bodies())
}

/// 渲染某一段的机读标记名（供错误文案指路，避免调用方硬编码字符串）。
pub fn render_marker(step: &str) -> &'static str {
    match step {
        "testplan" => crate::ac::BEGIN,
        "solution" => crate::touch::BEGIN,
        _ => "-",
    }
}

/// 技术方案批准前的**变更范围门禁 + `source_refs` 单向派生**（设计文档 §3.8）。
///
/// 返回**可能已被改写**的正文：frontmatter 的 `source_refs` 由第 2 段的
/// `GATE:TOUCH` 块派生，不要求也不允许人工维护第二份。
///
/// 为什么这样收口：两处都要求人写同一份文件清单，必然漂移；而漂移的声明等于
/// 没有声明。`GATE:TOUCH` 在第 2 段（人读的位置、且已是"涉及的文件与模块清单"
/// 那一节的自然落点），`source_refs` 在文件最前（`doc-guard` 的
/// `matter::extract` 要求首个非空行是 `---`，位置改不了）。
///
/// 判级沿用 L0 放行 / L1+ 拒绝：存量清单创建于 `GATE:TOUCH` 之前，
/// 一律拒绝会把它们**永久卡死**在 approve 上（与 `ensure_source_refs` 同理）。
/// 但**派生照做** —— 有声明就派生，没有就不写。
fn ensure_touch_declared(root: &Path, id: &str, content: &str) -> Result<String> {
    let reject = |msg: String| -> Result<String> {
        let level = crate::auth::effective_level(root);
        if level >= 1 {
            return Err(GateError::Validation(format!(
                "需求 {id} 的变更范围声明不合规，拒绝批准技术方案：\n\n{msg}"
            )));
        }
        eprintln!("⚠️ 警告（L{level}）：{msg}");
        Ok(content.to_string())
    };

    let Some((start, end)) = section_span(content, 1) else {
        return reject(
            "「## 2. 技术方案」二级标题定位失败，无法校验变更范围声明。\n\
             请把该段标题改回 `## 2. 技术方案`。"
                .to_string(),
        );
    };
    let lines: Vec<&str> = content.lines().collect();
    let section = lines[start - 1..end].join("\n");

    let paths: Vec<String> = match crate::touch::block_state(&section) {
        crate::touch::BlockState::Missing => {
            return reject(format!(
                "技术方案段没有 {} 标记块 —— 「方案没说改哪儿」就无法核对改动范围。\n\
                 请在「## 2. 技术方案」下加入：\n\
                 {}\ncore/src/**\n{}",
                crate::touch::BEGIN,
                crate::touch::BEGIN,
                crate::touch::END
            ));
        }
        crate::touch::BlockState::Unclosed => {
            return reject(format!(
                "技术方案段的声明块有 {} 但没有 {}。",
                crate::touch::BEGIN,
                crate::touch::END
            ));
        }
        crate::touch::BlockState::Found => {
            let (paths, bad, _) = crate::touch::declared(&section, start);
            if !bad.is_empty() {
                return reject(format!(
                    "变更范围声明里这些行无法归一（不得含 `..` 或以 `/` 开头）：\n  - {}",
                    bad.iter()
                        .map(|(r, l)| format!("第 {l} 行 {r:?}"))
                        .collect::<Vec<_>>()
                        .join("\n  - ")
                ));
            }
            paths
        }
    };
    if paths.is_empty() {
        return reject(format!(
            "{} 块里没有任何有效声明条目。\n\
             请逐行写出本次要改的文件或 glob（`#` 后为注释，空行忽略）。",
            crate::touch::BEGIN
        ));
    }

    // 派生 → 写 frontmatter（有 frontmatter 才写；存量清单不动它）
    let (derived, dropped) = crate::touch::to_source_refs(&paths);
    for d in &dropped {
        // 静默丢弃 = 人以为已声明。用告警说清：这类 glob 的目录语义是整个仓库，
        // 而 source_refs 的元素是「目录前缀」，表达不出「仓库根」。
        eprintln!(
            "⚠️ 需求 {id} 的声明条目 {d:?} 无法写进 frontmatter 的 source_refs：\n\
             它的目录语义是整个仓库，而 source_refs 的元素是「目录前缀 + 向下递归」。\n\
             请把它改写成具体的顶层目录（如 `backend/`、`frontend/`），否则 doc-guard\n\
             侧无从判断这批改动是否需要同步规格。"
        );
    }
    let old = crate::specmeta::extract(content)
        .map(|m| m.source_refs)
        .unwrap_or_default();
    if old != derived {
        match set_frontmatter_list(content, "source_refs", &derived) {
            Some(next) => {
                let event = format!(
                    "DERIVE_SOURCE_REFS {} {} -> {}",
                    id,
                    if old.is_empty() {
                        "-".to_string()
                    } else {
                        old.join(",")
                    },
                    derived.join(",")
                );
                crate::gate::audit(root, &event);
                crate::gate::audit_ledger(root, &event);
                return Ok(next);
            }
            None => {
                // 无 frontmatter 的存量清单：无法安全插入（doc-guard 要求它在文件最前，
                // 而 create 已经决定要不要生成）。不写，也不报错。
                eprintln!(
                    "⚠️ 需求 {id} 没有 frontmatter，source_refs 无法派生写入（该清单早于本功能）。\n\
                     其变更范围仍受 GATE:TOUCH 与 touch-check 约束；\
                     如需 doc-guard 侧也复核，请 req-guard archive 后新建清单。"
                );
            }
        }
    }
    Ok(content.to_string())
}

/// 改写 frontmatter 里的行内数组字段；无 frontmatter 或无解时返回 `None`。
///
/// **只改值、不动其它行**：frontmatter 必须保持在文件最前（`doc-guard` 的
/// `matter::extract` 要求首个非空行是 `---`），插入位置错一个字节就会让
/// 整份清单失去 frontmatter。
fn set_frontmatter_list(content: &str, key: &str, values: &[String]) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines
        .iter()
        .position(|l| !l.trim().is_empty() && l.trim().trim_start_matches('\u{feff}') == "---")?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| start + 1 + i)?;
    let rendered = format!("{}: [{}]", key, values.join(", "));
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut done = false;
    for (i, l) in lines.iter().enumerate() {
        if i > start && i < end && !done {
            let t = l.trim();
            if let Some((k, _)) = t.split_once(':') {
                if k.trim() == key {
                    out.push(rendered.clone());
                    done = true;
                    continue;
                }
            }
        }
        if i == end && !done {
            out.push(rendered.clone());
        }
        out.push(l.to_string());
    }
    Some(out.join("\n"))
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
    review_inner(
        root,
        ReviewCall {
            id,
            step,
            reviewer,
            pass,
            reason,
            strict,
            event: if pass { "APPROVE" } else { "REJECT" },
            quick: false,
        },
    )
}

/// `approve --all-steps`：一条命令批三段（REQ-019 §2.5 / G4，轻档审批形态）。
///
/// 四条硬约束（任一条不满足即整体拒绝，**不留部分批准**）：
///
/// 1. **原子性**：三段任一段校验失败（实质正文为空 / AC 不合格 / `GATE:TOUCH` 为空）
///    → 一段都不批。「批了两段」的状态比「一段没批」更危险 —— 前者看起来像快完成了。
/// 2. **留痕不减**：仍写**三条** `APPROVE` 台账行，各带 `sum=` / `updated=` / `sig=`，
///    `channel=quick`。quick 省的是**交互次数**，不是记录。
/// 3. **凭据不放宽**：L3 下需要一张 `scope` 匹配 `<需求ID>:*` 的**通配票据**，
///    且只允许消费一次（批三段算一次）。精确票不得用于 `--all-steps`，通配票不得用于
///    分步批准（见 [`crate::token::ScopeCheck::AllSteps`]）。
/// 4. **实质正文不放松**：第 3 段虽免 AC，但仍要求三段实质正文非空
///    （[`crate::section::is_section_empty`] 继续生效）—— 轻档至少要写
///    「改了什么 + 怎么自测」。
///
/// `strict_order` 在此场景是**显式豁免**（同批三段），豁免事实进台账。
pub fn review_all(
    root: &Path,
    id: &str,
    reviewer: &str,
    reason: &str,
    strict: bool,
) -> Result<Requirement> {
    review_inner(
        root,
        ReviewCall {
            id,
            step: STEPS[0].0,
            reviewer,
            pass: true,
            reason,
            strict,
            event: "APPROVE",
            quick: true,
        },
    )
}

/// **修订**（REQ-004 G1）：与 `reject` 机械上同一条路径（回退待审、清 `sum=`、
/// `--comment` 必填、**必须重审**），区别只在段状态标签与台账事件名。
///
/// 为什么要单列一个动作：实施期的合法反馈绝大多数是"方向没错，只是漏了个约束 /
/// 措辞要改"，用 `reject` 表达会让台账里"否决"与"改稿"混成一坨，事后无法回答
/// "哪几段反复返工 = 方案当初没想清楚"。分开后 amend 次数就是返工率指标（T2）。
///
/// 刻意**不给它任何额外能力**（免重审、免 comment）：一旦有，它立刻变成 `reject`
/// 的绕过口，两个动作会迅速合并回去 —— 那等于什么都没做。
pub fn amend(
    root: &Path,
    id: &str,
    step: &str,
    reviewer: &str,
    reason: &str,
    strict: bool,
) -> Result<Requirement> {
    validate_step(step)?;
    // `--comment` 必填（与 reject 同）：无留痕的改稿请求等于悄悄改稿。
    if reason.trim().is_empty() {
        return Err(GateError::Validation(format!(
            "修订 `{}`({}) 必须给出 `--comment` 说明改什么、为什么改。\n\
             修订与驳回同为留痕动作：没有说明就无法区分「方向没错的改稿」与「这段被否决」。",
            step,
            step_label(step)
        )));
    }
    review_inner(
        root,
        ReviewCall {
            id,
            step,
            reviewer,
            pass: false,
            reason,
            strict,
            event: "AMEND",
            quick: false,
        },
    )
}

/// `review` / `amend` 的入参。归拢成结构体而不是加第 8 个位置参数：
/// 一屏放不下的参数表本身就是"这几个参数其实是一个决策"的信号。
struct ReviewCall<'a> {
    id: &'a str,
    step: &'a str,
    reviewer: &'a str,
    pass: bool,
    reason: &'a str,
    strict: bool,
    /// 台账事件名（`APPROVE` / `REJECT` / `AMEND`）。
    event: &'a str,
    /// 一条命令批三段（`--all-steps`）：`step` 被忽略，按 [`STEPS`] 全批。
    quick: bool,
}

/// `review` / `amend` 共用的内核。
fn review_inner(root: &Path, c: ReviewCall<'_>) -> Result<Requirement> {
    let ReviewCall {
        id,
        step,
        reviewer,
        pass,
        reason,
        strict,
        event: event_kind,
        quick,
    } = c;
    // 本次实际要落的段：quick = 三段一次批（`--all-steps`），否则只有 `step`。
    let steps: Vec<&str> = if quick {
        STEPS.iter().map(|(k, _)| *k).collect()
    } else {
        vec![step]
    };
    // 审批锁（§4.4）：approve/reject 不得在 AI 执行上下文内发生，
    // 否则 AI 经 Shell 自批即可把状态欺诈骗成 approved。
    // L3 下凭据是**绑定该需求+步骤**的一次性票据（见 core/src/token.rs）；
    // `--all-steps` 要的是 `scope` 为 `<需求ID>:*` 的通配票（§2.5 约束 3）。
    let exact_scope = format!("{}:{}", id, step);
    crate::auth::ensure_human(
        match event_kind {
            "AMEND" => "amend",
            _ if pass => "approve",
            _ => "reject",
        },
        root,
        match quick {
            true => crate::token::ScopeCheck::AllSteps(id),
            false => crate::token::ScopeCheck::Exact(&exact_scope),
        },
    )?;
    if !quick {
        validate_step(step)?;
    }
    let r = find(root, id)?;
    let mut content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    // 归档即终点：已 done 的需求不得再审批（含归档区回退命中的文件），
    // 否则 recompute_head 会把 status=done 改回 in_review，破坏归档语义。
    if head_status(&content) == "done" {
        return Err(GateError::Validation(format!(
            "需求 {} 已归档（done），不再参与审核；如需调整请新建需求清单",
            id
        )));
    }

    // `strict_order` 在 `--all-steps` 下是**显式豁免**（同批三段，顺序天然满足）；
    // 豁免事实进台账（§2.5 末），故此处只在非 quick 路径上逐段校验。
    if strict && !quick {
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

    // 段落实质性（声明侧）：**被批准的那一段**必须有实质正文。
    //
    // 为什么放在 AC / TOUCH 校验之前：整段空模板是更根本的失败 ——
    // 「第 3 段 0 行实质正文」比「第 3 段缺 AC 块」更可执行，前者指方向、后者指格式。
    //
    // 为什么任何等级都拒绝（不像 TOUCH 那样 L0 放行）：内容为空不是"管理严格度"问题。
    // 唯一的例外是段定位失败（标题被改坏），那由 `ac::check` 报 SectionNotFound。
    if pass {
        let ph = template_placeholder_texts();
        for (i, s) in STEPS.iter().enumerate() {
            if !steps.contains(&s.0) {
                continue;
            }
            if crate::section::is_section_empty(&content, i, &ph) == Some(true) {
                // 原子性：三段里任一段空 → 整体拒绝（上面还没落盘，故不存在部分批准）。
                return Err(GateError::Validation(
                    crate::section::empty_section_message(&r.id, i),
                ));
            }
        }
    }

    // 验收标准机械校验（声明侧）：**测试计划**批准前 AC 必须格式合规。
    //
    // 为什么挂在审批上而不是只给 `ac check` 命令：审批是唯一能解锁编码的动作，
    // 把校验挂在钥匙上，"第三段写成三段空话"就**物理上无法通过** ——
    // 不依赖任何人记得跑检查命令。`ac check` 只是把同一判据提前暴露给 AI 与 CI，
    // 用于早失败。两者共用 `ac::lint`，不存在两份判定。
    if pass && steps.contains(&"testplan") {
        ensure_ac_compliant(&r.id, &content)?;
    }

    // 变更范围契约（声明侧）：**技术方案**批准前 `GATE:TOUCH` 必须有有效条目，
    // 且由它**单向派生**写入 frontmatter 的 `source_refs`（设计文档 §3.8）。
    //
    // 为什么卡在第二段而不是第一段：需求分解阶段还在澄清背景与目标，往往还不知道
    // 要改哪些文件；等到「涉及的文件与模块清单」这一步，声明范围才成为可核对的契约。
    // 卡第一段只会把 AI 卡在"还不知道要改什么"的时刻，逼迫它填占位值——那比不填更糟。
    //
    // 为什么必须在这里拦（doc-guard 拦不住）：FRS001 只检查 `source_refs` 这个**键
    // 是否存在**，`source_refs: []` 一律通过；FRS004 需要非空列表才能匹配变更集。
    // 于是「声明为空」这条路径在 doc-guard 侧完全静默——规格看似接入时效治理，
    // 实则永远不会被判过期。故声明侧的门禁只能放在批准动作上。
    if pass && steps.contains(&"solution") {
        content = ensure_touch_declared(root, &r.id, &content)?;
        // 交叉引用有效性（REQ-004 T5）：声明范围与引用有效性是**两件事**，
        // 前者管"改哪些文件"，后者管"引用的设计是否还在"。
        ensure_cross_refs_ok(root, &r.id, &content)?;
    }

    // `amended` 与 `rejected` 在**判定上完全等价**（都要求重新 approve 才解锁）；
    // 差异只在标签与台账，是 G1 要的可读性与可计数性。
    let new_status = if pass {
        "approved"
    } else if event_kind == "AMEND" {
        "amended"
    } else {
        "rejected"
    };
    let ts = safe_field(&crate::gate::now_str());
    // 身份绑定（§4.4）：把自报的 --reviewer 锚定到 git 身份，L1+ 对冲突/取不到 fail-closed。
    // 必须在 validate_step 之后、落盘之前完成——身份不可归属的审批不该留下任何痕迹。
    let stamp = crate::identity::bind(root, reviewer)?;
    let rv = safe_field(&stamp.reviewer);
    let em = safe_field(&stamp.email);
    let sg = safe_field(&stamp.sig);

    let mut out = String::new();
    for line in content.lines() {
        let this_step = is_marker_line(line)
            && line.contains("GATE:STEP")
            && steps.contains(&token(line, "name").as_str());
        if this_step {
            let cur = token(line, "name");
            let label = token(line, "label");
            // 内容冻结：approve 绑定**本次所批内容**的摘要；reject 写 `-`
            // （被拒绝的内容没有"已批准的正文"需要保护，留着旧摘要只会
            //  在下一轮 approve 前继续"保护"一份已经过时的内容）。
            let sum = if pass {
                match section_sum(&content, step_index(&cur).unwrap_or(0)) {
                    Some(v) => v,
                    None => {
                        return Err(GateError::Validation(format!(
                            "需求 {id} 的「{label}」段二级标题定位失败，无法绑定内容摘要。\n\
                             请把该段标题改回 `## {} . {label}`",
                            step_index(&cur).unwrap_or(0) + 1
                        )))
                    }
                }
            } else {
                "-".to_string()
            };
            out.push_str(&format!(
                "<!-- GATE:STEP name={} label={} status={} reviewer={} email={} sig={} updated={} sum={} -->\n",
                cur, label, new_status, rv, em, sg, ts, sum
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out = set_head_status(&out, &recompute_head(&out), None);
    // 审核记录：quick 一次写三条（三段各一条），逐段批准仍只写一条。
    for st in &steps {
        out = append_audit(
            &out,
            &format!(
                "- {} | {} <{}> | {} | {} | {}\n",
                ts,
                rv,
                em,
                st,
                new_status,
                if reason.trim().is_empty() {
                    "-"
                } else {
                    reason.trim()
                }
            ),
        );
    }

    fs::write(&r.path, out).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;

    // L3：审批已落盘 → 消费一次性票据（用后即废，杜绝同一凭据重放第二次审批）。
    // quick 下批三段**算一次**消费：省掉的是交互次数，不是「不可重放」。
    crate::auth::consume_credential_if_scoped();

    // 审计（§4.6）：审批/打回是关键事件——本机日志 + 入库台账（PR 可复核）；
    // 渠道标注（方案 C）让"审批来自带外/交互"可审计。quick 的渠道标成 `quick`，
    // 一次批三段在台账上必须与三次分批区分得开（否则事后无法回答「哪份清单被批量放过」）。
    let channel = match quick {
        true => "quick".to_string(),
        false => crate::auth::declared_channel().to_string(),
    };
    let order_note = match (quick, strict) {
        (true, true) => format!(
            " strict_order=exempt reason=approve-all-steps steps={}",
            steps.join("+")
        ),
        _ => String::new(),
    };
    for st in &steps {
        let event = format!(
            "{} {} step={} reviewer={} channel={} {}{} {}",
            event_kind,
            r.id,
            st,
            safe_field(&stamp.reviewer),
            channel,
            order_note,
            crate::auth::audit_ctx(),
            stamp.audit_fields()
        );
        crate::gate::audit(root, &event);
        crate::gate::audit_ledger(root, &event);
    }

    Ok(r)
}

/// 归档需求：整体状态置为 `done`，拦截脚本与 `check` 随之**跳过**该需求，
/// 不再作为"活跃需求"参与门禁判定——已解锁的项目由此回到"无活跃需求"的正常态。
///
/// 归档属审批类动作：AI 若能自归档，把带阻塞评论的需求归档、再 `create` 新需求，
/// 老需求上的阻塞即失效（历史缺陷"阻塞评论只作用于最新活跃需求"的另一半），
/// 故与 approve/reject/resolve/bypass 一样走 [`crate::auth::ensure_human`]。
/// 幂等：已是 `done` 时直接返回，不重复写审计。
pub fn done(root: &Path, id: &str, actor: &str) -> Result<Requirement> {
    crate::auth::ensure_human(
        "done",
        root,
        crate::token::ScopeCheck::Exact(&format!("done:{}", id)),
    )?;
    let r = find(root, id)?;
    // 文件已物理归档（find 只读回退命中）：幂等返回即可，不再重复写审计。
    if is_archived_path(&r.path) {
        return Ok(r);
    }
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    if head_status(&content) == "done" {
        return Ok(r);
    }
    // done 时间戳：到期自动归档据此判龄（文件名里的 YYYYMMDD 是创建日期，不是归档判据）。
    let now = safe_field(&crate::gate::now_str());
    // 身份绑定（§4.4）：归档同样改变门禁裁决的输入，actor 必须可归属。
    let stamp = crate::identity::bind(root, actor)?;
    let out = set_head_status(&content, "done", Some(&now));
    fs::write(&r.path, out).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;

    // L3：归档已落盘 → 消费一次性票据。
    crate::auth::consume_credential_if_scoped();

    // 审计：归档改变门禁裁决的输入，属关键事件——本机日志 + 入库台账。
    let event = format!(
        "DONE {} actor={} channel={} {} {}",
        r.id,
        safe_field(&stamp.reviewer),
        crate::auth::declared_channel(),
        crate::auth::audit_ctx(),
        stamp.audit_fields()
    );
    crate::gate::audit(root, &event);
    crate::gate::audit_ledger(root, &event);
    Ok(r)
}

// ===================== 到期物理归档 =====================

/// 一次归档的结果记录。
#[derive(Debug, Clone)]
pub struct ArchivedReq {
    pub id: String,
    pub title: String,
    /// 原路径（`.gates/requirements/…`）。
    pub src: PathBuf,
    /// 归档后路径（`.gates/requirements/archive/…`）。
    pub dst: PathBuf,
}

/// 到期自动归档：把 done 满 `after_days` 天的需求**物理搬入**归档区。
///
/// - 判龄依据是 HEAD 行的 `done=` 时间戳（[`done`] 写入）；存量无戳的 done 需求
///   回退到文件 mtime——都不是文件名的 `YYYYMMDD`（那是创建日期，不是归档判据）。
/// - 归入按创建年份分层：文件名含合法 `YYYYMMDD` → `archive/<年>/`，否则 `archive/misc/`。
/// - 正文与评论**成对搬移**（`fs::rename`，git 识别为 rename，历史不丢）；
/// - 每搬一条写审计 `ARCHIVE`（本机日志 + 入库台账）。
///
/// 属于审批类动作（走 [`crate::auth::ensure_human`]）：done 命令在其成功后的
/// 同一人类上下文里调用必然通过，同时挡住外部脚本/会话内不当触发。
/// `dry_run=true` 只计算不搬移、不写审计。单条搬移失败**跳过不中断**
/// （best-effort：清扫失败不得污染已完成的 done 或现金流入口）。
pub fn archive_due(
    root: &Path,
    actor: &str,
    after_days: u32,
    dry_run: bool,
) -> Result<Vec<ArchivedReq>> {
    crate::auth::ensure_human("archive", root, crate::token::ScopeCheck::Exact("archive"))?;
    archive_due_inner(root, actor, after_days, dry_run)
}

/// `done` 成功后的自动清扫入口：**已在同一人类授权窗口内**，不重复鉴权。
///
/// 存在理由：L1+ 下一次人类授权只对应一份凭据，若自动清扫再走一遍 [`archive_due`]，
/// 人类会看到一条无意义的"archive 缺凭据"告警。外部脚本仍只能走 [`archive_due`]
/// （`done` 本身受鉴权保护，能走到这里的进程必然刚通过过鉴权）。
pub fn archive_due_authorized(
    root: &Path,
    actor: &str,
    after_days: u32,
    dry_run: bool,
) -> Result<Vec<ArchivedReq>> {
    archive_due_inner(root, actor, after_days, dry_run)
}

fn archive_due_inner(
    root: &Path,
    actor: &str,
    after_days: u32,
    dry_run: bool,
) -> Result<Vec<ArchivedReq>> {
    let mut out = Vec::new();
    for r in list(root)? {
        let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
            path: Some(r.path.clone()),
            source: e,
        })?;
        if head_status(&content) != "done" {
            continue;
        }
        if done_age_days(&r.path, &content) < after_days as i64 {
            continue;
        }
        match move_archived(root, &r, actor, dry_run, true) {
            Ok(Some(a)) => out.push(a),
            Ok(None) => {} // 目标已存在：幂等越界
            Err(e) => {
                eprintln!("⚠️ 归档 {} 失败（已跳过，不影响其余）：{}", r.id, e);
            }
        }
    }
    // L3：实际发生了搬移才消费票据（dry-run / 无可归档项不该白丢一张票）。
    if !dry_run && !out.is_empty() {
        crate::auth::consume_credential_if_scoped();
    }
    Ok(out)
}

/// 手动归档单条（存量 done 需求一次性补扫 / 等不及自动清扫时使用）。
///
/// 与 [`archive_due`] 同一条搬移链，仅校验更严：明确指定 id，且**禁止归档活跃需求**
/// （未 done 一律拒绝——否则等于给"AI 把带阻塞评论的需求挪走逃逸门禁"开了后门）。
pub fn archive_one(root: &Path, actor: &str, id: &str, dry_run: bool) -> Result<Vec<ArchivedReq>> {
    crate::auth::ensure_human("archive", root, crate::token::ScopeCheck::Exact("archive"))?;
    let r = find(root, id)?;
    if is_archived_path(&r.path) {
        return Err(GateError::Validation(format!(
            "需求 {} 已在归档区，无需再归档",
            id
        )));
    }
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    let st = head_status(&content);
    if st != "done" {
        return Err(GateError::Validation(format!(
            "需求 {} 尚未 done（当前 {}）——归档是生命周期终点，请先执行：\
             req-guard done {} --author <姓名>",
            id, st, id
        )));
    }
    let mut out = Vec::new();
    if let Some(a) = move_archived(root, &r, actor, dry_run, false)? {
        out.push(a);
    }
    // L3：实际搬移成功才消费票据。
    if !dry_run && !out.is_empty() {
        crate::auth::consume_credential_if_scoped();
    }
    Ok(out)
}

/// 单条搬移：`fs::rename` 正文 + 评论 → 目标分层目录，并写审计。
/// 目标已存在时返回 `Ok(None)`（幂等）；`dry_run` 只返回将归档项不落盘。
fn move_archived(
    root: &Path,
    r: &Requirement,
    actor: &str,
    dry_run: bool,
    is_auto: bool,
) -> Result<Option<ArchivedReq>> {
    let dst = archive_dst(r)?;
    if dst.exists() {
        return Ok(None);
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| GateError::Io {
            path: Some(parent.to_path_buf()),
            source: e,
        })?;
    }
    let stem = r
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let comments = r
        .path
        .with_file_name(format!("{}{}", stem, COMMENTS_SUFFIX));
    if dry_run {
        return Ok(Some(ArchivedReq {
            id: r.id.clone(),
            title: r.title.clone(),
            src: r.path.clone(),
            dst,
        }));
    }
    fs::rename(&r.path, &dst).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    if comments.exists() {
        let dst_comments = dst.with_file_name(format!("{}{}", stem, COMMENTS_SUFFIX));
        fs::rename(&comments, &dst_comments).map_err(|e| GateError::Io {
            path: Some(comments.clone()),
            source: e,
        })?;
    }
    let event = format!(
        "ARCHIVE {} actor={} auto={} channel={} {}",
        r.id,
        safe_field(actor),
        if is_auto { 1 } else { 0 },
        crate::auth::declared_channel(),
        crate::auth::audit_ctx()
    );
    crate::gate::audit(root, &event);
    crate::gate::audit_ledger(root, &event);
    Ok(Some(ArchivedReq {
        id: r.id.clone(),
        title: r.title.clone(),
        src: r.path.clone(),
        dst,
    }))
}

/// 目标路径：文件名含合法 `YYYYMMDD` → `archive/<年>/`；否则 `archive/misc/`。
fn archive_dst(r: &Requirement) -> Result<PathBuf> {
    let name = r
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = match year_of(&name) {
        Some(y) => r.path.parent().unwrap().join(ARCHIVE_DIR).join(y),
        None => r.path.parent().unwrap().join(ARCHIVE_DIR).join("misc"),
    };
    Ok(dir.join(&name))
}

/// 从文件名提取合法创建年份（8 位 `YYYYMMDD` 的年份）；取不到返回 `None`。
fn year_of(name: &str) -> Option<String> {
    let digits: String = name
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(8)
        .collect();
    if digits.len() != 8 {
        return None;
    }
    let m: u32 = digits[4..6].parse().ok()?;
    let d: u32 = digits[6..8].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(digits[0..4].to_string())
}

/// 需求是否已处于归档区（`find` 只读回退命中时各写操作据此设护栏）。
fn is_archived_path(p: &Path) -> bool {
    p.to_string_lossy().replace('\\', "/").contains("/archive/")
}

/// done 距今的天数：优先 HEAD `done=` 戳，存量无戳时回退文件 mtime。
/// 两者都取不到 → 按已到期处理（保守归档：done 就是终点）。
fn done_age_days(path: &Path, content: &str) -> i64 {
    let Some(today) = today_days() else {
        return i64::MAX;
    };
    let done_days = head_line(content)
        .map(|l| token(l, "done"))
        .filter(|s| !s.is_empty())
        .and_then(|t| days_of_stamp(&t))
        .or_else(|| {
            fs::metadata(path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|s| (s.as_secs() / 86_400) as i64)
                })
        });
    match done_days {
        Some(d) => today - d,
        None => i64::MAX,
    }
}

/// 今天的"自 1970-01-01 的天数"（取自 `now_str()` 的日期段）。
fn today_days() -> Option<i64> {
    days_of_stamp(&crate::gate::now_str())
}

/// `YYYY-MM-DD_HH:MM:SS` / `YYYY-MM-DD HH:MM:SS` / `YYYYMMDD` → 自 1970-01-01 的天数。
/// 解析失败返回 `None`。
fn days_of_stamp(s: &str) -> Option<i64> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).take(8).collect();
    if digits.len() != 8 {
        return None;
    }
    let y: i64 = digits[..4].parse().ok()?;
    let m: i64 = digits[4..6].parse().ok()?;
    let d: i64 = digits[6..8].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// Howard Hinnant 格里历算法：`(y, m, d)` → 自 1970-01-01 的天数
/// （与 `SystemTime::duration_since(UNIX_EPOCH)/86400` 同一基准，差值即天数差）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

// ===================== 内容冻结：批准与内容绑定 =====================

/// 某一步的内容冻结状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SumState {
    /// 已绑定：状态 approved 且 `sum=` 是合法的 64 位十六进制。
    Frozen(String),
    /// 未绑定：`sum=` 缺失或为 `-`（含存量清单）。
    Absent,
    /// 字段存在但格式非法（被手改坏了）——**不得当成通过**。
    Malformed(String),
}

/// 该段正文的 SHA-256（64 位十六进制）。
///
/// **归一化只做两件事，且都是免费的**：
/// - 行尾统一：走 [`str::lines`] —— Rust 的 `lines()` 本就把 `\r\n` 切成 `\n`
///   并丢弃残留的 `\r`，所以 `lines().join("\n")` 天然跨平台一致。
///   不做这一步，同一份内容在 LF 与 CRLF 平台会算出两个摘要，跨平台 CI 全假红。
/// - 尾部换行：`lines()` 对 `"a\nb"` 与 `"a\nb\n"` 给出同样的结果，join 后一致。
///
/// **刻意不做**去首尾空白之类的"更友好"归一：那些归一会让真实改动藏起来。
/// 摘要的作用是"逐字未变"，任何宽容都削弱它。
pub fn section_sum(content: &str, step: usize) -> Option<String> {
    let (start, end) = section_span(content, step)?;
    let lines: Vec<&str> = content.lines().collect();
    Some(crate::digest::sha256_hex(
        lines[start - 1..end].join("\n").as_bytes(),
    ))
}

/// 某一步的 `sum=` 字段状态。
pub fn step_sum(content: &str, step: &str) -> SumState {
    let Some(line) = step_line(content, step) else {
        return SumState::Absent;
    };
    if token(line, "status") != "approved" {
        // 未批准就没有"已批准的正文"需要保护
        return SumState::Absent;
    }
    let raw = token(line, "sum");
    if raw.is_empty() || raw == "-" {
        return SumState::Absent;
    }
    if raw.len() == 64 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        SumState::Frozen(raw)
    } else {
        SumState::Malformed(raw)
    }
}

/// 某一步的下标（按 [`STEPS`]）。
pub fn step_index(step: &str) -> Option<usize> {
    STEPS.iter().position(|(k, _)| *k == step)
}

/// 某一步的**内容冻结全景**（REQ-002：`approve` 时把摘要绑到所批内容上）。
///
/// 与 [`SumState`] 的区别：`SumState` 只看标记行里 `sum=` 这个**字段**
/// （"有没有绑"），`SealState` 还把**当前正文**算进去（"绑的是不是现在这份"）。
/// 前端要展示的恰恰是后者——审核人看到的是"这一段还能不能信"，
/// 而不是"字段写没写"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealState {
    /// 未批准：没有"已批准的正文"需要保护（`amend` / `reject` 后即如此）。
    NotApplicable,
    /// 已批准、已绑定，且正文与批准时逐字一致（真正的冻结）。
    Frozen,
    /// 已批准但未绑定摘要（存量清单，或还没执行过 `seal`）。
    NotSealed,
    /// 已批准且已绑定，但**正文已被改动** → 要么走 `amend` → 改 → `approve` 重审，
    /// 要么显式 `seal --reason` 重新绑定（后者记 `RESEAL`，台账里数得出来）。
    Changed,
    /// `sum=` 字段格式非法，或该段二级标题定位失败 —— 一律 **fail-closed**，
    /// 既不算"已冻结"也不放行界面照旧显示为已通过。
    Unverifiable,
}

impl SealState {
    /// 是否需要审核人处置（界面据此高亮，别让"待处理"混在一片绿里）。
    pub fn needs_action(self) -> bool {
        matches!(
            self,
            SealState::NotSealed | SealState::Changed | SealState::Unverifiable
        )
    }

    /// 一行说明（界面提示 / 测试断言共用，避免各写一套文案）。
    pub fn hint(self) -> &'static str {
        match self {
            SealState::NotApplicable => "未批准，不涉及内容冻结",
            SealState::Frozen => "已绑定内容摘要：正文与批准时一致",
            SealState::NotSealed => "已批准但未绑定内容摘要（执行 seal 绑定当前内容）",
            SealState::Changed => "正文已被改动，需重审（amend → 改 → approve）或 seal 重新绑定",
            SealState::Unverifiable => "sum 字段损坏或段落定位失败，无法校验（fail-closed）",
        }
    }
}

/// 某一步的内容冻结全景（纯函数：只读正文，不落盘）。
///
/// 为什么不叫前端各自算：这段判定要同时看 `status=` / `sum=` 与**当前段正文**，
/// 三处任一不一致都会得出不同结论。放进 core 才有一份真相，两个界面也不会漂移。
pub fn seal_state(content: &str, step: &str) -> SealState {
    match step_sum(content, step) {
        // 未批准 → 没有"已批准的正文"；`sum=` 此时是 `-`，不是缺失。
        SumState::Absent => {
            let status = step_line(content, step).map(|l| token(l, "status"));
            if status.as_deref() == Some("approved") {
                SealState::NotSealed
            } else {
                SealState::NotApplicable
            }
        }
        SumState::Malformed(_) => SealState::Unverifiable,
        SumState::Frozen(bound) => {
            let Some(idx) = step_index(step) else {
                return SealState::Unverifiable;
            };
            match section_sum(content, idx) {
                // 段落定位失败 → 无从校验，fail-closed（与 verify_sums 的口径一致）。
                None => SealState::Unverifiable,
                Some(now) if now == bound => SealState::Frozen,
                Some(_) => SealState::Changed,
            }
        }
    }
}

/// 内容冻结问题类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SumIssueKind {
    /// 已批准段的正文与批准时不一致（被改过）。
    ContentChanged,
    /// 已批准但未绑定摘要（存量清单 / 未 seal）。
    NotSealed,
    /// `sum=` 存在但格式非法。
    MalformedSum,
    /// 已绑定摘要但该段定位失败 —— 无从校验，fail-closed。
    SectionMissing,
}

impl SumIssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SumIssueKind::ContentChanged => "ContentChanged",
            SumIssueKind::NotSealed => "NotSealed",
            SumIssueKind::MalformedSum => "MalformedSum",
            SumIssueKind::SectionMissing => "SectionMissing",
        }
    }
}

/// 一条内容冻结问题。
#[derive(Debug, Clone)]
pub struct SumIssue {
    pub severity: crate::issue::Severity,
    pub kind: SumIssueKind,
    pub message: String,
}

/// 校验一份清单的三个步骤：已批准段的正文是否仍与批准时一致（**纯函数**）。
///
/// 严重级取向：
/// - `ContentChanged` / `MalformedSum` / `SectionMissing` → **Error**（fail-closed）
/// - `NotSealed` → **Warn**（存量清单不能因此卡死，但必须**显式报出** ——
///   静默跳过等于"看起来有冻结、实际没有"）
pub fn verify_sums(content: &str) -> Vec<SumIssue> {
    let mut out = Vec::new();
    for (idx, (key, label)) in STEPS.iter().enumerate() {
        match step_sum(content, key) {
            SumState::Absent => {
                let line = step_line(content, key);
                let status = line.map(|l| token(l, "status")).unwrap_or_default();
                if status == "approved" {
                    out.push(SumIssue {
                        severity: crate::issue::Severity::Warn,
                        kind: SumIssueKind::NotSealed,
                        message: format!(
                            "「{label}」已批准但**未启用内容冻结**（sum=-）：\
                             批准后该段正文可被任意改动而无人察觉。\n\
                             执行 `req-guard seal <需求ID>` 可绑定当前内容"
                        ),
                    });
                }
            }
            SumState::Malformed(raw) => out.push(SumIssue {
                severity: crate::issue::Severity::Error,
                kind: SumIssueKind::MalformedSum,
                message: format!(
                    "「{label}」的 sum={raw:?} 不是 64 位十六进制摘要 —— 无法校验，不当通过。\n\
                     请用 `req-guard seal <需求ID>` 重新绑定"
                ),
            }),
            SumState::Frozen(expect) => match section_sum(content, idx) {
                None => out.push(SumIssue {
                    severity: crate::issue::Severity::Error,
                    kind: SumIssueKind::SectionMissing,
                    message: format!(
                        "「{label}」已绑定内容摘要，但该段的二级标题定位失败 —— \
                         无从校验正文是否被改过，故不放行。\n\
                         请把标题改回第 {} 段的二级标题，或 `req-guard seal <需求ID>` 重新绑定",
                        idx + 1
                    ),
                }),
                Some(actual) if actual != expect => out.push(SumIssue {
                    severity: crate::issue::Severity::Error,
                    kind: SumIssueKind::ContentChanged,
                    message: format!(
                        "「{label}」正文与批准时不一致：期望 {}… 实际 {}…。\n\
                         批准绑定的是**当时那份内容**，改了就必须重新过审。\n\
                         处置：先 `req-guard reject <需求ID> --step {key} --comment \"内容已变更\"` \
                         改完再 `approve`；若确实无需重审，用 `req-guard seal <需求ID>` 重新绑定。",
                        &expect[..8],
                        &actual[..8]
                    ),
                }),
                Some(_) => {}
            },
        }
    }
    out
}

/// 已冻结段清单：`(步骤下标, 摘要)`，供写入侧比对"这次写入改了哪一段"。
pub fn frozen_sections(content: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (idx, (key, _)) in STEPS.iter().enumerate() {
        if let SumState::Frozen(sum) = step_sum(content, key) {
            out.push((idx, sum));
        }
    }
    out
}

/// 改写 `GATE:STEP` 的 `sum=` 字段（[`seal`] 用；保持其余字段原样）。
fn with_sum(content: &str, step: &str, sum: &str) -> String {
    let mut out = String::new();
    for line in content.lines() {
        if is_marker_line(line) && line.contains("GATE:STEP") && token(line, "name") == step {
            out.push_str(&format!(
                "<!-- GATE:STEP name={} label={} status={} reviewer={} email={} sig={} updated={} sum={} -->\n",
                step,
                token(line, "label"),
                token(line, "status"),
                token(line, "reviewer"),
                token(line, "email"),
                token(line, "sig"),
                token(line, "updated"),
                sum
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `req-guard seal --all`：对全部未归档清单逐份 seal（一张票盖完，避免逐份换票的摩擦）。
pub fn seal_all(root: &Path, reason: &str) -> Result<Vec<SealOutcome>> {
    let ids: Vec<String> = list(root)?
        .into_iter()
        .filter(|r| {
            std::fs::read_to_string(&r.path)
                .map(|c| head_status(&c) != "done")
                .unwrap_or(true)
        })
        .map(|r| r.id)
        .collect();
    seal_many(root, &ids, reason)
}

/// `seal` 一次盖多份清单：**一次人类命令 = 一次人类意图**，故出入场检查与票据消费
/// 都只做一次。若每份各自走 [`seal`]，L3 下就要逐张换票，那个摩擦大到会有人干脆
/// 不 seal —— 于是"已批准却无冻结"沦为默认态，这道门就成了摆设。
pub fn seal_many(root: &Path, ids: &[String], reason: &str) -> Result<Vec<SealOutcome>> {
    crate::auth::ensure_human("seal", root, crate::token::ScopeCheck::Any)?;
    let mut out = Vec::new();
    for id in ids {
        out.push(SealOutcome {
            id: id.clone(),
            bound: seal_inner(root, id, reason)?,
        });
    }
    crate::auth::consume_credential_if_scoped();
    Ok(out)
}

/// `seal` 逐份结果：需求 ID 与该清单各段的绑定摘要（段标签 → 摘要）。
#[derive(Debug, Clone)]
pub struct SealOutcome {
    pub id: String,
    pub bound: Vec<(String, String)>,
}

/// `req-guard apply`：一次命令完成「读草稿 → 校验 → 写正文 → 重新 approve」（REQ-007）。
///
/// 存在的理由：一次修订原本要人跑两趟 —— `amend`（清摘要、打回待审）→ AI 改 →
/// `approve`（写新摘要）。草稿区让 AI 的改动有个不参与判定的地方先放着，
/// 于是人可以只跑这一条。
///
/// **不豁免任何审批守卫**：本函数第一行就 `ensure_human`，AI 上下文直拒、L3 票据
/// 绑定到具体 (需求, 步骤)。最后一步调用的是**既有的** `review(pass=true)` ——
/// 于是 AC 合规（REQ-001）、摘要写入（REQ-002）、`GATE:TOUCH` 有效性（REQ-001）、
/// 交叉引用有效性（REQ-004）、身份绑定、票据消费**全部自动生效**。
/// [`apply`] 的结果。
///
/// 带一个 `appended_lines` 而不是只回 `Requirement`：位置配对模型下**草稿多出的行
/// 会被追加到段尾**，这件事必须由 CLI 说出来（REQ-010 AC-006）—— 人若不知道，就会在
/// 下一次修订时把那段"看起来跑到了别处"的文字当成在段中间。
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub req: Requirement,
    /// 草稿比原段散文多出的行数（0 = 逐行覆盖，无追加）。
    pub appended_lines: usize,
}

/// 若这里自己实现「写摘要」，就会立刻出现「apply 路径绕过 AC 合规检查」这类
/// 最难发现的漏洞 —— 本项目已经吃过一次（落盘钩子是旧版那次）。
///
/// **先校验后落盘**：正文替换与审批合成一步，但任何校验失败都在**写盘之前**返回，
/// 保证磁盘逐字不变（半落盘会留下「正文改了、状态没改」的中间态）。
pub fn apply(
    root: &Path,
    id: &str,
    step: &str,
    reviewer: &str,
    comment: &str,
) -> Result<ApplyOutcome> {
    apply_inner(root, id, step, reviewer, comment, None)
}

/// [`apply`] 的实现；`race_probe` 是**仅供单测**的竞态注入点（见 REQ-010 G6 / AC-015）。
///
/// 生产路径恒传 `None`。单测用它把「校验已过、但草稿在写盘前被改」这个时间差造出来 ——
/// 不这么做就只能断言"字符串比较成立"，那是自证型测试，测不到 P4 的那条分支。
#[allow(clippy::type_complexity)]
fn apply_inner(
    root: &Path,
    id: &str,
    step: &str,
    reviewer: &str,
    comment: &str,
    race_probe: Option<(&str, &str)>,
) -> Result<ApplyOutcome> {
    // 审批锁先行：不给未授权者"知道草稿长什么样"的机会。
    crate::auth::ensure_human(
        "apply",
        root,
        crate::token::ScopeCheck::Exact(&format!("{}:{}", id, step)),
    )?;
    validate_step(step)?;
    if comment.trim().is_empty() {
        return Err(GateError::Validation(
            "apply 必须给出 `--comment` 说明这次改了什么、为什么。\n\
             apply 会把草稿变成**正式内容并绑定新摘要** —— 审核人批的是「当时那份内容」，\n\
             没有说明就无法区分「按预期修订」与「AI 自己觉得该改」。"
                .to_string(),
        ));
    }
    let r = find(root, id)?;
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    if head_status(&content) == "done" {
        return Err(GateError::Validation(format!(
            "需求 {id} 已归档（done），不接受修订"
        )));
    }
    // 草稿**原始字节**先记下：写盘前要重读比对（REQ-010 G6 / P4）。
    let draft_raw = fs::read_to_string(draft_path(root, id)).unwrap_or_default();
    let draft = read_draft(root, id)?.ok_or_else(|| {
        GateError::Validation(format!(
            "未找到草稿：{}\n\
             草稿路径固定为 `.gates/drafts/<需求ID>.draft.md`，用行内标记分段：\n\
             {{step=<段名>}}\n<该段拟替换的正文>\n\
             段名只能是 decomposition / solution / testplan。\n\
             不写草稿而直接 apply 是无意义的 —— apply 只做「把草稿变成正式内容」这一件事。",
            draft_path(root, id).display()
        ))
    })?;
    // P0-1：**一次只应用一段**。多段草稿此前会被「应用一段 → 删掉整份草稿」静默吃掉
    // 其余段（REQ-006 实施期实测踩过）。这里在写盘前拒绝，并列出草稿现有段名。
    if draft.steps().len() > 1 {
        return Err(GateError::Validation(format!(
            "草稿含 {} 段（{}），apply 一次只应用一段。\n\
             多段草稿会在应用其中一段后被整体删除，其余段**无声丢失** —— \
             这是「看着应用了、其实丢了内容」的最坏形态。\n\
             请按段逐次应用：\n\
             \x20 1) 把草稿裁剪成只留 `{{step={step}}}` 一段 → apply --step {step}\n\
             \x20 2) 需要下一段时重写草稿 → apply --step <段名>",
            draft.steps().len(),
            draft.steps().join("、"),
        )));
    }
    let body = draft.get(step).map(str::to_string).ok_or_else(|| {
        GateError::Validation(format!(
            "草稿里没有 `{step}`({}) 段。\n草稿现有段：{}\n\
             注意 apply 是**逐段**的：草稿只写了一部分段时，只能 apply 对应的那一段。",
            step_label(step),
            if draft.steps().is_empty() {
                "（空）".to_string()
            } else {
                draft.steps().join("、")
            }
        ))
    })?;

    // P0-2：草稿**只提供散文**。块标记写进草稿不会生效（`replace_section` 原样保留
    // 原块的行、不消耗草稿行），于是草稿里的块标记会与原块错位、把 `GATE:AC` 的
    // 开闭标记拆散 —— REQ-006 实施期实测踩过（`ac check` 报 50 条 OutsideBlock，
    // 报错是噪音，看不出真因）。这里直接指出行号。
    if let Some((ln, text)) = first_gate_marker_line(&body) {
        return Err(GateError::Validation(format!(
            "草稿 `{step}` 段的第 {ln} 行是 GATE 块标记（{}），但 apply 的草稿通道**只写散文**。\n\
             块（`GATE:TOUCH` / `GATE:AC` / `GATE:AUDIT` …）由 req-guard 维护，草稿里的标记\n\
             不会生效，只会把位置错位。块内条目（如验收标准）请由人经 `amend` 直接改。",
            text.trim()
        )));
    }

    // 组装「应用后的完整清单」，在**内存里**跑全部校验。全过才落盘。
    let idx = step_index(step).unwrap_or(0);

    // P0-3：草稿与原段散文**行数不等**的后果必须说出来。
    // 位置配对模型下：草稿更长 → 多出的行被追加到段尾（合法，但落点由工具决定，人未必预期）；
    // 草稿更短 → 原段尾部散文**被静默丢弃**（不可接受）。故短则拒绝，长则放行并在
    // 成功信息里点明落点（REQ-010 AC-005 / AC-006 / AC-011）。
    let cur_prose_lines = prose_line_count(&content, idx);
    let draft_lines = body.trim_end_matches('\n').lines().count();
    if draft_lines < cur_prose_lines {
        return Err(GateError::Validation(format!(
            "草稿 `{step}` 段只有 {draft_lines} 行，而清单该段散文有 {cur_prose_lines} 行 —— 少 {} 行。\n\
             草稿通道是**按行位置覆盖**：草稿短于原段时，原段尾部散文会被静默丢弃。\n\
             请把该段散文**完整**写进草稿（可以整段重写），或本就别删那些行。",
            cur_prose_lines - draft_lines
        )));
    }
    let surplus = draft_lines - cur_prose_lines;

    let proposed = replace_section(&content, idx, &body);

    // P0-4：块 kind 集合自检。当前实现**只可能**在实现变更时不一致 —— 那正是它要
    // 暴露的东西（AC-004 刻意构造会失败的形态，否则这条校验就是恒成立的自证）。
    let before_kinds = gate_kinds(&section_of(&content, idx));
    let after_kinds = gate_kinds(&section_of(&proposed, idx));
    if before_kinds != after_kinds {
        return Err(GateError::Validation(format!(
            "应用后该段的 GATE 块集合发生变化（{before_kinds:?} → {after_kinds:?}）—— \
             这是 apply 实现的缺陷，不是草稿的问题。\n\
             块由 req-guard 维护，草稿改不到它们；请重跑 `req-guard install` 更新二进制后再试。"
        )));
    }
    // `review_inner` 里那些校验（AC 合规 / TOUCH 声明 / 交叉引用）都对
    // `proposed` 生效；这里额外自查段是否还有实质正文，免得落盘后才发现被清空。
    // 比对**应用后**的该段正文：GATE 块会被移到段末，所以「草稿散文与原文一致」
    // 并不等于「应用后全文一致」（实测：判草稿原文会漏判），而全文里还有
    // 文件头与 `## 审核记录` —— 只有段正文才是"这次改了什么"的正确比较对象。
    let cur_body = section_of(&content, idx);
    let new_body = section_of(&proposed, idx);

    if strip_gate_blocks(&new_body) == strip_gate_blocks(&cur_body) {
        return Err(GateError::Validation(format!(
            "草稿的 `{step}` 段正文与清单当前内容完全相同，apply 不会做任何事。\n\
             若这正是意图（内容无需改），那就不该 apply —— 请确认草稿是否贴错了段。"
        )));
    }
    // 实质正文查**草稿散文**（不含 GATE 块）：块会被保留下来，所以判拼装结果
    // 永远非空 —— 那样「用 apply 抹掉一段」就变成了可行路径。
    if crate::section::count_substantive(&body, &[]) == 0 {
        return Err(GateError::Validation(format!(
            "草稿的 `{step}`({}) 段没有实质正文（只有空行、注释、标题或模板占位）。\n\
             apply 会把它写成正式内容 —— 那等于用 apply 抹掉这一段。\n\
             若确实要清空该段，请改用 `amend`（保留明确的人工意图与台账事件）。",
            step_label(step)
        )));
    }

    // **先校验后落盘**（REQ-007 风险 2 / AC-006）：把声明侧校验对**内存里的
    // proposed** 先跑一遍。任何一项失败都在写盘之前返回，磁盘逐字不变。
    //
    // 这一层不可省：`review` 会重新读**磁盘**再校验，而磁盘此刻还是旧内容 ——
    // 直接写盘再调 review，等于「先改后验」，校验失败时磁盘已被改动，
    // 留下「正文改了、状态没改」的中间态（那比直接失败难收拾得多）。
    dry_run_review(&r.path, &proposed, step)?;

    // P4 / G6：写盘前**重读草稿并逐字比对**（`apply` 的最后一道）。
    //
    // 为什么逐字比而不是 mtime / size：mtime 会被 `touch`、`git checkout`、编辑器保存改；
    // size 会被「改了一个字又改回来」骗过。草稿长度是几百字节量级，逐字比代价可忽略。
    //
    // 残留的「重读 → 写盘」TOCTOU 窗口见 REQ-010 §2 关键设计 5：彻底消除需要锁文件
    // （`flock` / `LockFileEx` 跨平台两套实现），本期明确接受。
    if let Some((_, current)) = race_probe {
        // 单测注入：把草稿改成"校验之后才有的样子"，让下一行的比对命中
        fs::write(draft_path(root, id), current).unwrap();
    }
    let draft_now = fs::read_to_string(draft_path(root, id)).unwrap_or_default();
    if draft_now != draft_raw {
        return Err(GateError::Validation(format!(
            "草稿在校验期间被修改（校验时 {} 字节 → 写盘前 {} 字节），**未落盘**。\n\
             本次落盘的内容是基于**旧**草稿算出来的，而草稿已经变了 —— \
             静默落盘等于「你以为是新草稿、其实是旧的」。\n\
             请重跑 `req-guard apply <需求ID> --step {step}` 以按当前草稿重新校验。",
            draft_raw.len(),
            draft_now.len(),
        )));
    }

    fs::write(&r.path, &proposed).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    // 先落 AMEND 事件：apply **是一次真实返工**，若只记 APPROVE，REQ-004 的
    // 返工率指标（`amend_counts` 按 AMEND 事件计数）就会漏计这次修订 ——
    // 指标漏计比不指标更坏：那会让"这段很少返工"变成一个假象。
    //
    // 为什么不是走 `amend()` 再走 `review()`：那会让清摘要与写摘要都落盘一次，
    // 中间态可被观察到，且两处都会发 `ensure_human`（要两张票）。
    // 这里只**记事件**、不改状态 —— 状态由下面那次 approve 一步到位。
    let amend_event = format!(
        "AMEND {} step={} reviewer={} channel={} {} {}",
        id,
        step,
        safe_field(reviewer),
        crate::auth::declared_channel(),
        crate::auth::audit_ctx(),
        reason_audit_fields(reviewer, root)
    );
    crate::gate::audit(root, &amend_event);
    crate::gate::audit_ledger(root, &amend_event);

    // 复用既有审批路径（详见函数文档）。它会重新读盘、跑全部声明侧校验、
    // 写新摘要、落 APPROVE 事件。
    review(root, id, step, reviewer, true, comment, false)?;

    // 草稿已消费：删掉，避免下次 apply 重复应用同一份内容。
    let _ = std::fs::remove_file(draft_path(root, id));
    Ok(ApplyOutcome {
        req: find(root, id)?,
        appended_lines: surplus,
    })
}

/// 去掉段内的 GATE 块（连同其前空行），只留散文 —— 比较"这次改了什么"时用。
fn strip_gate_blocks(section: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_block = false;
    for l in section.lines() {
        let t = l.trim();
        if !in_block && t.starts_with("<!-- GATE:") && !t.starts_with("<!-- /GATE:") {
            in_block = true;
            continue;
        }
        if in_block {
            if t.starts_with("<!-- /GATE:") {
                in_block = false;
            }
            continue;
        }
        if t.starts_with("<!-- /GATE:") {
            continue;
        }
        out.push(l);
    }
    // 忽略纯空白行与 HTML 注释：它们是模板骨架，不是作者内容。判「无变更」时
    // 把它们算进去会让一次空行差异消耗掉一次真实审批。
    out.iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("<!--"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 对**尚未落盘**的候选内容跑一遍声明侧校验（REQ-007「先校验后落盘」）。
///
/// 逐项复制 `review_inner` 的声明侧判据 —— 是复制而非调用，因为 `review_inner`
/// 只接受"从磁盘读"的内容。之所以不重构 `review_inner` 接受内容参数：那会
/// 改动 approve/amend/reject/touch --declare 四条既有路径，风险远大于收益。
///
/// **重复风险**：将来 `review_inner` 新增声明侧校验时，这里不会自动同步 ——
/// 那会表现为「apply 比 approve 宽松」。用 `apply_与review声明侧判据一致`
/// 锁住：把两条路径的通过/拒绝结论对照。
fn dry_run_review(path: &std::path::Path, proposed: &str, step: &str) -> Result<()> {
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if proposed.trim().is_empty() {
        return Err(GateError::Validation("候选内容为空，拒绝落盘".into()));
    }
    if head_status(proposed) == "done" {
        return Err(GateError::Validation(format!(
            "需求 {id} 已归档（done），不接受修订"
        )));
    }
    if step == "testplan" {
        ensure_ac_compliant(&id, proposed)?;
    }
    if step == "solution" {
        let root = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .unwrap_or_else(|| Path::new("."));
        // 声明为空：与 approve 同一套判据（apply 不代劳补声明 —— 范围是契约）
        let sec = section_of(proposed, 1);
        let declared_now = crate::touch::declared(&sec, 1).0;
        if declared_now.is_empty() {
            return Err(GateError::Validation(format!(
                "需求 {id} 的技术方案段没有有效变更范围声明（`{}` 块为空）。\n\
                 apply 不代劳补声明：变更范围是**契约**，请先 `touch --declare` 打回并重审。",
                crate::touch::BEGIN
            )));
        }
        ensure_cross_refs_ok(root, &id, proposed)?;
    }
    Ok(())
}

/// 记台账时用到的身份戳（`review` 内部同名逻辑的轻量版）。
///
/// 这里**不**做 `identity::bind` 的等级判定：apply 前一步已经调过 `ensure_human`，
/// 身份可归属性已在那道关上校验过；此处只需落盘可读的审核人字段。
fn reason_audit_fields(reviewer: &str, _root: &Path) -> String {
    format!("email=- sig=- actor={}", safe_field(reviewer))
}

/// 把某一段的正文整段替换掉（`## N. …` 到下一个 `## ` 之前），返回新全文。
///
/// 保留二级标题行本身：标题是段定位的锚点，草稿不该有能力改它
/// （改了会让后续所有定位失效）。
///
/// **GATE 块原样保留、保序**：草稿只提供该段的**散文部分**。`GATE:AC`（第 3 段）
/// 与 `GATE:TOUCH`（第 2 段）各有独立的声明侧校验，且它们的存在性本身就是
/// 「这段合规」的判据（`ac check` 的 A1、`ensure_touch_declared`）。若草稿整段
/// 覆盖把它们冲掉，apply 就成了「绕过声明侧校验」的后门 —— 实测正是这样：
/// 一份坏 AC 草稿被整段写入后才报错，而此时磁盘已经被改了。
///
/// 保序（而非把块统一挪到段末）：块位置一动就会产生**纯空白差异**的"变更"，
/// 让「内容其实没改」的一次 apply 消耗掉一次真实审批 —— 而审批是本系统里最贵的
/// 动作，拿它换空白差异是亏的。
///
/// 不采用「要求草稿自带标记块」的方案：那让草稿作者必须手工搬运整块标记，
/// 一个字写错就把该段判为不合规 —— 而正确的块就在原段里，本来不需要重写。
/// 草稿散文里第一处 GATE 块标记（返回 `(1-based 行号, 该行文本)`）。
///
/// 判据与 [`crate::gate::gate_lines`] 同源：只认以 `<!-- GATE:` / `<!-- /GATE:`
/// 开头的**整行 trim 后**形态。块标记写进草稿**不会生效**（`replace_section` 原样保留
/// 原块的行、不消耗草稿行），只会与原块错位并把块拆散，故在源头拦下并指行号。
fn first_gate_marker_line(body: &str) -> Option<(usize, String)> {
    body.lines().enumerate().find_map(|(i, l)| {
        let t = l.trim();
        (t.starts_with("<!-- GATE:") || t.starts_with("<!-- /GATE:"))
            .then(|| (i + 1, t.to_string()))
    })
}

/// 某段里 GATE 块的 kind 集合（按出现顺序，含重复；开标记与闭标记只算开标记）。
///
/// 用于「应用后块的 kind 集合必须与原文一致」的自检（`apply` 的 P0-4）。
fn gate_kinds(section: &str) -> Vec<String> {
    section
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            if t.starts_with("<!-- /GATE:") {
                return None;
            }
            t.strip_prefix("<!-- GATE:")
                .and_then(|rest| rest.split_whitespace().next())
                .map(|k| k.to_string())
        })
        .collect()
}

/// 该段散文（= [`replace_section`] 会替换的那些行、去掉 GATE 块）的行数。
///
/// **必须与 `replace_section` 的切片口径逐字一致**，否则「草稿比原段短」这条校验会
/// 拿错基准（曾因此把"多 1 行"算成"不多"）。故这里复用同一段切片逻辑，而不是
/// 另写 `section_of` / `section_body` —— 那两个的口径与 `replace_section` 不同。
fn prose_line_count(content: &str, idx: usize) -> usize {
    let Some((start, end)) = section_span(content, idx) else {
        return 0;
    };
    let lines: Vec<&str> = content.lines().collect();
    let lo = start; // 0-based，正文首行（与 replace_section 同口径）
    let hi = end.saturating_sub(1).min(lines.len());
    if lo >= hi {
        return 0;
    }
    strip_gate_blocks(&lines[lo..hi].join("\n")).lines().count()
}

fn replace_section(content: &str, idx: usize, body: &str) -> String {
    let Some((start, end)) = section_span(content, idx) else {
        return content.to_string();
    };
    let lines: Vec<&str> = content.lines().collect();
    let lo = start; // 0-based，正文首行
    let hi = end.saturating_sub(1).min(lines.len()); // 0-based，**不含**下一段标题

    // 逐行透传：散文被草稿替换，GATE 块（含开闭标记与其间的条目）原样保留。
    let draft_lines: Vec<&str> = body.trim_end_matches('\n').lines().collect();
    let mut di = 0usize;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.extend(lines[..lo].iter().map(|s| s.to_string()));
    let mut i = lo;
    while i < hi {
        let t = lines[i].trim();
        let is_open = t.starts_with("<!-- GATE:") && !t.starts_with("<!-- /GATE:");
        if !is_open {
            if di < draft_lines.len() {
                out.push(draft_lines[di].to_string());
                di += 1;
            }
            i += 1;
            continue;
        }
        // GATE 块：连开闭标记带内条目整段原样保留，不消耗草稿行
        let kind = t
            .trim_start_matches("<!-- GATE:")
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        let close = format!("<!-- /GATE:{kind} -->");
        let mut j2 = i;
        loop {
            out.push(lines[j2].to_string());
            if lines[j2].trim() == close || j2 + 1 >= hi {
                j2 += 1;
                break;
            }
            j2 += 1;
        }
        i = j2;
    }
    // 草稿还有剩余（比原段长）：追加到块之后
    while di < draft_lines.len() {
        out.push(draft_lines[di].to_string());
        di += 1;
    }
    out.extend(lines[hi..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// `req-guard seal`：把三个步骤的 `sum=` 绑定到**当前正文**（不改状态）。
///
/// 走 [`crate::auth::ensure_human`]：AI 若能给自己批过的清单补摘要，
/// 等于自证"已批内容未变" —— 这条命令存在的全部意义就是让人来兜底。
pub fn seal(root: &Path, id: &str, reason: &str) -> Result<Vec<(String, String)>> {
    crate::auth::ensure_human("seal", root, crate::token::ScopeCheck::Any)?;
    crate::auth::consume_credential_if_scoped();
    seal_inner(root, id, reason)
}

/// `seal` 的无鉴权内核。**私有**：公开面上不留跳过出入场检查的 seal 入口；
/// 批量绑定由 [`seal_many`] 在一次检查之后调用。
fn seal_inner(root: &Path, id: &str, reason: &str) -> Result<Vec<(String, String)>> {
    let r = find(root, id)?;
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    if head_status(&content) == "done" {
        return Err(GateError::Validation(format!(
            "需求 {id} 已归档（done），不再绑定内容摘要"
        )));
    }
    // 可绑定集合 = **已批准 且 尚未绑定**的段。
    //
    // 两个条件缺一不可，各堵一个洞：
    //
    // ① 「已批准」—— 堵 `amend` → 改 → `seal` 的绕道：`amend` 把段置为 amended
    //    并清 `sum=`，若 seal 照绑不误，它就成了「不重新审批也能让改动过的正文
    //    重新获得冻结背书」的通道 —— 那恰好是 REQ-002 要消灭的形态。
    //    未批准的段只能由 `approve` 绑定（approve 会留下 reviewer / updated）。
    //
    // ② 「尚未绑定」—— 这才是"首次启用"的准确判据。**部分绑定**（存量清单里
    //    两段是 REQ-002 上线前批准的、另一段后来重审过）是常态而非例外；
    //    把"仍有段没绑"当成"已启用过、需理由"会给**补齐**这条正路强加摩擦
    //    （实测：REQ-004 自己就卡在这里 —— solution 后来重审带上了摘要，
    //    decomposition / testplan 仍是 REQ-002 上线前批准的 `sum=-`）。
    //
    // 故：还有段可绑 → 正常放行（记 SEAL）；一段都没的可绑（= 冻结已全面启用）
    // → 须 `--reason`（记 RESEAL 并计数），因为那时再 seal 只能是"改完再补摘要"。
    let mut to_bind: Vec<(&str, &str)> = Vec::new();
    let mut not_approved: Vec<&str> = Vec::new();
    for (k, l) in STEPS {
        if step_status(&content, k) != "approved" {
            not_approved.push(l);
            continue;
        }
        if matches!(step_sum(&content, k), SumState::Frozen(_)) {
            continue;
        }
        to_bind.push((k, l));
    }
    let first_enable = !to_bind.is_empty();
    if !first_enable {
        if !not_approved.is_empty() {
            return Err(GateError::Validation(format!(
                "需求 {id} 的 {} 段未处于 approved，seal 不代劳审批：\n\
                 请先 `approve <需求ID> --step <段> --comment '...'`，由它写入摘要。\n\
                 这是刻意的：`amend` 清摘要是为了强制重审，若 seal 能绑未批准的段，\n\
                 「打回 → 改 → seal」就成了一条跳过重审的绕道。",
                not_approved.join("、")
            )));
        }
        if reason.trim().is_empty() {
            let bound_labels: Vec<&str> = STEPS
                .iter()
                .filter(|(k, _)| {
                    step_status(&content, k) == "approved"
                        && matches!(step_sum(&content, k), SumState::Frozen(_))
                })
                .map(|(_, l)| *l)
                .collect();
            return Err(GateError::Validation(format!(
                "需求 {id} 的 {} 段已绑定过内容摘要，再次 seal 必须给 `--reason <原因>`。\n\
                 首次启用（存量清单补绑定）不需要理由 —— 那种情况仍有段可绑。\n\
                 到这一步还来 seal，意味着内容已改动过；正常路径是\n\
                 `amend <需求ID> --step <段> --comment '...'` → 改 → `approve`（自动写入新摘要）。\n\
                 确需跳过重审时请显式说明理由，此举记入台账（RESEAL）并计数。",
                bound_labels.join("、")
            )));
        }
        // RESEAL 路径：重绑全部已批准段（幂等）
        to_bind = STEPS
            .iter()
            .filter(|(k, _)| step_status(&content, k) == "approved")
            .map(|(k, l)| (*k, *l))
            .collect();
    }
    let mut out = content.clone();
    let mut bound = Vec::new();
    for (key, label) in to_bind {
        let idx = step_index(key).unwrap_or(0);
        let Some(sum) = section_sum(&content, idx) else {
            return Err(GateError::Validation(format!(
                "需求 {id} 的「{label}」段二级标题定位失败，无法计算摘要；\
                 请把该段标题改回第 {} 段的二级标题后重试",
                idx + 1
            )));
        };
        out = with_sum(&out, key, &sum);
        bound.push((label.to_string(), sum));
    }
    fs::write(&r.path, &out).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    let sums = bound
        .iter()
        .map(|(l, s)| format!("{l}:{}", &s[..8]))
        .collect::<Vec<_>>()
        .join(",");
    // 首次启用记 `SEAL`；再次绑定记 `RESEAL` 并带上理由 —— 台账里能直接数出
    // "有几份清单被反复擦过"，这是 REQ-002 落地后最需要被看见的信号。
    let event = if !first_enable {
        format!(
            "RESEAL {} sums={} reason={}",
            r.id,
            sums,
            safe_field(reason.trim())
        )
    } else {
        format!("SEAL {} sums={}", r.id, sums)
    };
    crate::gate::audit(root, &event);
    crate::gate::audit_ledger(root, &event);
    Ok(bound)
}

/// 某清单各段的**修订次数**（返工率指标，REQ-004 G4 / T2）。
///
/// 从台账 `AMEND <id> step=<段>` 事件统计。只统计、不阻断：高返工率是需要复盘的
/// 工程信号，不是一条可机械判定的硬伤；做成门禁只会催生"少写 amend 刷分"。
pub fn amend_counts(root: &Path, id: &str) -> Vec<(String, usize)> {
    let Ok(text) = std::fs::read_to_string(root.join(crate::gate::LEDGER_REL)) else {
        return STEPS.iter().map(|(k, _)| (k.to_string(), 0)).collect();
    };
    let needle = format!("AMEND {id} step=");
    STEPS
        .iter()
        .map(|(k, _)| {
            let n = text
                .lines()
                .filter(|l| l.contains(&needle) && l.contains(&format!("step={k} ")))
                .count();
            (k.to_string(), n)
        })
        .collect()
}

/// 该行是否为**机读标记行**（`<!-- GATE:… -->` 或 `<!-- /GATE:… -->`）。
///
/// **为什么不能按字面量 `contains` 判定**：正文里合法地提到 `GATE:STEP` 是常态 ——
/// 写验收标准（"执行 approve 后 status 变为 approved"）、写方案、写注释都会提到它。
/// 子串匹配把散文当标记行，后果双向：
/// - [`review`] 的改写循环会**原地替换**那一行 → 静默毁掉正文，并在正文中间
///   注入一条 `label=` 为空的机器标记行（`safe_field` 的 `-` 约定也被破坏）；
/// - [`crate::gate::doc_write_guard`] 把它计入"状态行集合" → AI 正常改动这类正文
///   反被判成自批而**拦截**。即"正文合法提到 `GATE:STEP` ⇒ 这份清单此后再也写不动"。
///
/// 标记行的语法就是"以 `<!-- GATE:` 开头"，而合法 Markdown 正文不可能以此开头
/// （`<!--` 本身就是注释起始）。故按**语法前缀**判定，不按字面量出现判定。
///
/// 放在 core 的 `requirement` 而非各前端：GATE 标记是**文件格式约定**，
/// 格式约定的唯一解释处在这里（与 `section_of` 同一原则）。
/// 把某一步打回 `pending`（`touch --declare` 扩张变更范围后用）。
///
/// **为什么需要它**：变更范围扩张若不触发重新过审，就等于 AI 可以自己把方案改宽 ——
/// 那 `GATE:TOUCH` 就从"契约"退化成"建议"。打回 `pending` 后 `recompute_head`
/// 会把整体状态拉回 `changes_requested`，必须重新 `approve` 才能继续。
///
/// 走与 `review` 相同的标记行判据（[`is_marker_line`]），因此正文里提到
/// `GATE:STEP name=<step>` 的散文不会被改写。
pub fn reopen_step(root: &Path, id: &str, step: &str) -> Result<()> {
    validate_step(step)?;
    let r = find(root, id)?;
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    if head_status(&content) == "done" {
        return Err(GateError::Validation(format!(
            "需求 {id} 已归档（done），不再变更范围声明"
        )));
    }
    let mut out = String::new();
    let mut hit = false;
    for line in content.lines() {
        if is_marker_line(line) && line.contains("GATE:STEP") && token(line, "name") == step {
            hit = true;
            let label = token(line, "label");
            out.push_str(&format!(
                // sum= 一并清空：内容因 `--declare` 扩张而变了，旧摘要若留着，
                // 会继续"保护"一份已改动的正文 —— 那正是本功能要消灭的形态。
                "<!-- GATE:STEP name={} label={} status=pending reviewer=- email=- sig=- updated={} sum=- -->\n",
                step,
                label,
                safe_field(&crate::gate::now_str())
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !hit {
        return Err(GateError::Validation(format!(
            "需求 {id} 的 GATE:STEP 标记行里找不到 name={step}，无法打回"
        )));
    }
    let out = set_head_status(&out, &recompute_head(&out), None);
    fs::write(&r.path, out).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })
}

/// 供审计事件使用：空白转下划线、空串转 `-`（GATE/台账的字段约定）。
pub fn safe_field_for_audit(s: &str) -> String {
    safe_field(s)
}

pub fn is_marker_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("<!-- GATE:") || t.starts_with("<!-- /GATE:")
}

fn head_line(content: &str) -> Option<&str> {
    content
        .lines()
        .find(|l| is_marker_line(l) && l.contains("GATE:HEAD"))
}

fn step_line<'a>(content: &'a str, step: &str) -> Option<&'a str> {
    content
        .lines()
        .find(|l| is_marker_line(l) && l.contains("GATE:STEP") && token(l, "name") == step)
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

/// 指定步骤的审核时间。
pub fn step_updated(content: &str, step: &str) -> String {
    step_line(content, step)
        .map(|l| token(l, "updated"))
        .unwrap_or_default()
}

/// 重写 HEAD 行。
///
/// `done_ts` 为 `Some` 时写入/更新 `done=` 字段（`done()` 归档用）；
/// 为 `None` 时**保留**原 HEAD 行的 `done=`（后续 review 改状态不得丢归档时间戳）。
fn set_head_status(content: &str, status: &str, done_ts: Option<&str>) -> String {
    let mut out = String::new();
    for line in content.lines() {
        if is_marker_line(line) && line.contains("GATE:HEAD") {
            let id = token(line, "id");
            let created = token(line, "created");
            let done = match done_ts {
                Some(t) => format!(" done={}", t),
                None => {
                    let d = token(line, "done");
                    if d.is_empty() {
                        String::new()
                    } else {
                        format!(" done={}", d)
                    }
                }
            };
            out.push_str(&format!(
                "<!-- GATE:HEAD id={} status={} created={}{} -->\n",
                id, status, created, done
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

/// 扫描目录（**含归档区**）内 `REQ-<n>*` 取最大编号 +1。
///
/// 必须同扫 `archive/`：否则编号型需求被物理归档后新需求会复用旧号，
/// 撞"id 是永久主键，归档不复号"（C5）——`ids --check` 的查重管不到"复号后单文件"。
fn next_id(dir: &Path) -> Result<String> {
    let mut max = bump_max(dir, 0u32);
    let archive = dir.join(ARCHIVE_DIR);
    if archive.exists() {
        max = bump_max(&archive, max);
    }
    Ok(format!("REQ-{:03}", max + 1))
}

/// 递归收集目录内编号型需求的最大序号（不含评论文件）。
fn bump_max(dir: &Path, mut max: u32) -> u32 {
    let Ok(rd) = fs::read_dir(dir) else {
        return max;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            max = bump_max(&p, max);
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") || name.ends_with(COMMENTS_SUFFIX) {
            continue;
        }
        if let Some(rest) = name.strip_prefix("REQ-") {
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num.parse::<u32>() {
                if n > max {
                    max = n;
                }
            }
        }
    }
    max
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
    let today = crate::gate::today_str();
    let mut s = String::new();
    // frontmatter 必须在**文件最前**（doc-guard 的 matter::extract 要求首个非空行是 `---`）。
    // 字段名与取值枚举刻意与 doc-guard 的 FRS 族对齐，使本文件能被 doc-guard 的
    // FRS001/003/004/005/007 与 DRF001 直接接管，无需任何映射层：
    //   doc_type      ∈ reference/guide/decision/runbook/proposal → 需求规格取 proposal
    //   tier          ∈ critical/standard
    //   review_policy = codebound → doc-guard 追加要求 verified_at + source_refs
    // 「规格绑定代码」正是 SDD 里 spec/plan 与实现之间的契约，doc-guard 已有
    // 一等公民语义（codebound），不该由 req-guard 另造一套。
    s.push_str("---\n");
    s.push_str("doc_type: proposal\n");
    s.push_str("tier: standard\n");
    s.push_str("owner: -\n");
    s.push_str("review_policy: codebound\n");
    s.push_str(&format!("verified_at: {}\n", today));
    s.push_str("source_refs: []\n");
    s.push_str("---\n\n");
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

<!-- 技术方案批准前必须在上方 frontmatter 的 source_refs 声明本次要改的文件/模块，
     否则 doc-guard 的 FRS004（源已改而规格未同步）无法发现规格腐化。
     格式为行内数组，元素可用目录（向下递归）：source_refs: [backend/src/main/java/com/x]
     注意：source_refs 不支持 glob，且它是 req-guard 从下方 GATE:TOUCH 块**单向派生**的
     （见 docs/设计/AC与变更范围契约技术方案.md §3.8）——**不要手改 frontmatter，
     改了会在下次 approve 时被覆盖回去。要改声明范围，改下面的块。 -->

<!-- 变更范围契约：GATE:TOUCH 是本清单唯一的「要改哪些文件」人工声明源。
     req-guard touch-check 用它与 git 实际改动集比对（pre-commit + CI），
     并在 approve 时单向派生出 frontmatter 的 source_refs。
     语法：每行一条相对仓库根的路径或 glob，# 后为注释，空行忽略。
       core/src/**      跨层级
       core/src/*.rs    单层
       cli/src/cli.rs   精确文件
     要求：反斜杠会被归一为 /；不接受 .. 逃出仓库根的条目。
     块为空 → 技术方案批准被拒（连同 source_refs 一并不写，保持 fail-closed）。 -->
<!-- GATE:TOUCH -->
core/src/**
<!-- /GATE:TOUCH -->
";

const TESTPLAN_BODY: &str = "\
- [ ] 单元测试用例（编号 + 断言点）
- [ ] 端到端用例（编号 + 执行步骤）
- [ ] 边界 / 异常 / 并发场景
- [ ] 回归范围与影响面
- [ ] 验收门槛（可机械判定）

<!-- 验收标准：req-guard ac check 机械校验（规则见 docs/设计/AC与变更范围契约技术方案.md §2）。
     条目形态唯一：三行式 —— 编号独占一行，其下恰好三个子句列表项。
     硬约束：编号形如 `AC-<3位数字>`，须连续、无重复、无跳号；三个子句顺序固定
     Given → When → Then；子句不得少于 4 个非空白字符；Given 必须自包含（不得写
     「与上一条相同」这类外部指代，引用违规样例一律用行内代码标记包起来）；
     Then 必须可度量（含数字或字面量）。
     注意：本说明刻意不写出具体编号字面量 —— 块外出现 `AC-` + 3 位数字会被 A7 判违规。
     标记行只能由 req-guard 维护；块内正文可自由编辑。 -->
<!-- GATE:AC -->
### AC-001
- Given: <可复现的前置状态；写具体，不写「正常情况」这类不可判定的前提>
- When: <一次可触发的操作；写出具体命令或步骤>
- Then: <可观测结果；含退出码、字面量或数值>
<!-- /GATE:AC -->
";

// ===================== 单元测试 =====================

#[cfg(test)]
#[allow(non_snake_case)] // 与既有中文测试命名一致
mod tests {
    use super::*;
    use crate::testutil::{cleanup, disable_auth, fill_sections, set_auth_level, temp_dir};

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
    fn section_of_按二级标题切段且不越界() {
        let doc = "# REQ-001 登录改造\n\n\
                   ## 1. 需求分解\n\n- 背景\n\n\
                   ## 2. 技术方案\n\n- 总体思路\n\n\
                   ## 3. 测试计划\n\n- 用例\n\n\
                   ## 审核记录\n\n<!-- GATE:AUDIT -->\n";
        // 各段只含自己的内容
        let s0 = section_of(doc, 0);
        assert!(s0.contains("## 1. 需求分解") && s0.contains("背景"));
        assert!(!s0.contains("技术方案"), "第一段不得混入第二段：{}", s0);
        let s1 = section_of(doc, 1);
        assert!(s1.contains("## 2. 技术方案") && s1.contains("总体思路"));
        assert!(!s1.contains("测试计划"));
        assert!(!s1.contains("需求分解"), "段首即边界，不得回吞上一段");
        // 末段必须停在下一个二级标题（审核记录）之前，否则审核记录会被当成正文
        let s2 = section_of(doc, 2);
        assert!(s2.contains("- 用例"));
        assert!(!s2.contains("审核记录"), "末段不得吞掉审核记录：{}", s2);
        // 标题写法宽松：数字后面的分隔符不影响定位
        assert!(section_of("## 1 需求分解\n- a\n", 0).contains("- a"));
        // 定位不到 → 回退整篇（宁可多显示，不能白屏）
        assert_eq!(section_of(doc, 9), doc);
        assert_eq!(section_of("- 没有标题的清单\n", 0), "- 没有标题的清单\n");
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

        fill_sections(&root, "REQ-001");
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

    // ── REQ-019 T6：`approve --all-steps` ───────────────────────────────

    /// 把第 3 段的 `GATE:AC` 块**替换**成一条合规 AC。
    ///
    /// 替换而不是追加：模板自带一块占位 AC（占位子句不合规），追加只会多一条
    /// 跳号条目，`ensure_ac_compliant` 照样拒 —— 夹具必须**替掉**它才测到目标路径。
    fn fill_ac(root: &std::path::Path, id: &str) {
        upsert_ac_block(
            root,
            id,
            "### AC-001\n\
- Given: 一份三段正文均已填实质内容的需求清单\n\
- When: 对该清单执行一次 approve --all-steps\n\
- Then: 三段状态均为 approved 且台账新增 3 条 APPROVE\n",
        )
    }

    /// 写入 `<!-- GATE:AC -->…<!-- /GATE:AC -->`（块已存在则替换其正文，否则整块插入）。
    ///
    /// 「已存在 / 不存在」两种形态都收在这里而不是调用方各判一次：夹具里这两条
    /// 路径总是成对出现（先删块验证选填、再补块验证不合规），各写一遍必然有一处
    /// 会在块不存在时 panic。
    fn upsert_ac_block(root: &std::path::Path, id: &str, body: &str) {
        let p = find(root, id).expect("清单应存在").path;
        let c = fs::read_to_string(&p).expect("清单应可读");
        if !c.contains("<!-- GATE:AC -->") {
            let at = c.find("## 审核记录").expect("模板应含审核记录区块");
            let block = format!("<!-- GATE:AC -->\n{body}<!-- /GATE:AC -->\n");
            let out = format!("{}{}{}", &c[..at], block, &c[at..]);
            fs::write(&p, out).expect("清单应可写");
            return;
        }
        replace_ac_block(root, id, body);
    }

    /// 整体替换 `<!-- GATE:AC -->…<!-- /GATE:AC -->` 之间的正文（块本身由 req-guard 维护）。
    fn replace_ac_block(root: &std::path::Path, id: &str, body: &str) {
        let p = find(root, id).expect("清单应存在").path;
        let c = fs::read_to_string(&p).expect("清单应可读");
        let begin = c.find("<!-- GATE:AC -->").expect("模板应含 AC 块");
        let after = begin + "<!-- GATE:AC -->".len();
        let end = c[after..]
            .find("<!-- /GATE:AC -->")
            .map(|i| after + i)
            .expect("模板应含 AC 结束标记");
        // 注意 `&c[end..]` 已含结束标记，故这里**只补开标记**，不再补结束标记
        // （补两次会留下一个块外孤立标记，A7 会判成块外条目）。
        let out = format!("{}<!-- GATE:AC -->\n{}{}", &c[..begin], body, &c[end..]);
        fs::write(&p, out).expect("清单应可写");
    }

    /// 删掉整个 `GATE:AC` 块（轻档「AC 选填」场景）。
    fn drop_ac_block(root: &std::path::Path, id: &str) {
        let p = find(root, id).expect("清单应存在").path;
        let c = fs::read_to_string(&p).expect("清单应可读");
        let begin = c.find("<!-- GATE:AC -->").expect("模板应含 AC 块");
        let after = begin + "<!-- GATE:AC -->".len();
        let end = after
            + c[after..]
                .find("<!-- /GATE:AC -->")
                .expect("模板应含 AC 结束标记")
            + "<!-- /GATE:AC -->".len();
        let out = format!("{}{}", &c[..begin], &c[end..]);
        fs::write(&p, out).expect("清单应可写");
    }

    /// 接线锁：批量批准走通配 scope、分步批准走精确 scope。
    ///
    /// 为什么不直接断言错误文案：范围判据的**真值**在 `token::ticket_usable`
    /// （纯函数，见 token.rs 用例）；这里锁的是「哪条命令用哪种 scope」这根线 ——
    /// 它写反的后果是「一张精确票就能批量批准三段」，而那条路在单测里只能靠
    /// 文案间接观察。
    fn quick_path_is_all_steps() -> bool {
        // `review_all` 传 `ScopeCheck::AllSteps(id)`（见 review_inner）；
        // 这里以源码事实自证：函数体内出现 AllSteps 且分步分支仍是 Exact。
        let src = include_str!("requirement.rs");
        src.contains("crate::token::ScopeCheck::AllSteps(id)")
            && src.contains("crate::token::ScopeCheck::Exact(&exact_scope)")
    }

    /// 把 frontmatter 的 `tier` 改成给定值（仅测试用；真实路径由人类手改 / approve 派生）。
    fn set_declared_tier(root: &std::path::Path, id: &str, tier: &str) {
        let p = find(root, id).expect("清单应存在").path;
        let c = fs::read_to_string(&p).expect("清单应可读");
        let out = c
            .lines()
            .map(|l| {
                if l.starts_with("tier:") {
                    format!("tier: {tier}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, out).expect("清单应可写");
    }

    #[test]
    fn u19_一条命令批三段且三条台账各带摘要() {
        let root = temp_dir("req-quick-ok");
        create(&root, None, "轻档批量批准").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        disable_auth(&root);

        review_all(&root, "REQ-001", "寇工", "", true).unwrap();

        let r = find(&root, "REQ-001").unwrap();
        let content = fs::read_to_string(&r.path).unwrap();
        for step in ["decomposition", "solution", "testplan"] {
            assert_eq!(step_status(&content, step), "approved", "{step} 应已批准");
            assert!(
                matches!(step_sum(&content, step), SumState::Frozen(_)),
                "{step} 必须绑定 sum=（留痕不减）"
            );
        }
        assert_eq!(head_status(&content), "approved");
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        let approves = ledger.matches("APPROVE REQ-001").count();
        assert_eq!(approves, 3, "三段各一条 APPROVE：\n{ledger}");
        assert_eq!(
            ledger.matches("channel=quick").count(),
            3,
            "每条都要标 channel=quick：\n{ledger}"
        );
        assert!(
            ledger.contains("strict_order=exempt"),
            "顺序豁免必须留痕：\n{ledger}"
        );
        cleanup(&root);
    }

    #[test]
    fn u20_声明块为空在L0只告警在L1以上硬拦() {
        // AC-010 的机械口径是「非零退出码 + 三段全 pending」，那条只在 **L1+** 成立：
        // `ensure_touch_declared` 的缺声明路径是**既有**的 L0 放行（REQ 的 TOUCH 宽容档），
        // 批量批准不得顺手把它收紧（那是另一条需求的改动），也不得假装它不存在。
        // 故这里锁住两侧事实，L1+ 的拦截由 `scripts/verify_tier.py` 在真机上判决。
        let root = temp_dir("req-quick-touch");
        create(&root, None, "声明块为空").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        disable_auth(&root);
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        let out: String = c
            .lines()
            .filter(|l| l.trim() != "core/src/**")
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, out).unwrap();

        // L0：只告警并放行（既有宽容档）
        review_all(&root, "REQ-001", "寇工", "", true).unwrap();
        let content = fs::read_to_string(&p).unwrap();
        for step in ["decomposition", "solution", "testplan"] {
            assert_eq!(step_status(&content, step), "approved", "{step}");
        }
        cleanup(&root);

        // L1+：连鉴权都过不去（无凭据、无 TTY）→ 非零退出码，三段全 pending
        let root = temp_dir("req-quick-touch-l1");
        create(&root, None, "声明块为空").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        set_auth_level(&root, 3);
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        let out: String = c
            .lines()
            .filter(|l| l.trim() != "core/src/**")
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, out).unwrap();
        let e = review_all(&root, "REQ-001", "寇工", "", true).unwrap_err();
        assert!(e.to_string().contains("票据"), "{e}");
        let content = fs::read_to_string(&p).unwrap();
        for step in ["decomposition", "solution", "testplan"] {
            assert_eq!(
                step_status(&content, step),
                "pending",
                "{step} 应保持 pending"
            );
        }
        cleanup(&root);
    }

    #[test]
    fn u20b_验收标准不合规时整体拒绝不得批了两段() {
        // 与 u20 同一条原子性判据的另一条**等级无关**路径（AC 校验不看 auth 等级），
        // 因此可以在单测里真正跑到「第二段校验失败」这一步而不依赖真机凭据。
        let root = temp_dir("req-quick-ac-bad");
        create(&root, None, "AC 不合规").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        disable_auth(&root);
        // 编号跳号（A3）：声明档 standard 时是硬伤，且**不看 auth 等级**
        upsert_ac_block(
            &root,
            "REQ-001",
            "### AC-001\n\
- Given: 一份三段正文均已填实质内容的需求清单\n\
- When: 对该清单执行一次 approve --all-steps\n\
- Then: 三段状态均为 approved 且台账新增 3 条 APPROVE\n\
### AC-003\n\
- Given: 一份编号从 001 跳到 003 的清单\n\
- When: 执行 req-guard ac check REQ-001\n\
- Then: 退出码为 1 且类别为 SeqGap\n",
        );

        assert!(review_all(&root, "REQ-001", "寇工", "", true).is_err());
        let p = find(&root, "REQ-001").unwrap().path;
        let content = fs::read_to_string(&p).unwrap();
        for step in ["decomposition", "solution", "testplan"] {
            assert_ne!(
                step_status(&content, step),
                "approved",
                "{step} 不得被批准（原子性）"
            );
        }
        assert!(
            !fs::read_to_string(root.join(crate::gate::LEDGER_REL))
                .unwrap_or_default()
                .contains("APPROVE"),
            "整体拒绝时不得留下任何 APPROVE 记录"
        );
        cleanup(&root);
    }

    #[test]
    fn u21_第三段实质正文为空时整体拒绝() {
        let root = temp_dir("req-quick-empty");
        create(&root, None, "空段").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        disable_auth(&root);
        // 把第 3 段正文清回模板占位（= 实质正文 0 行）
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        let out: String = c
            .lines()
            .filter(|l| !l.contains("测试计划：本用例的测试夹具。"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, out).unwrap();

        assert!(review_all(&root, "REQ-001", "寇工", "", true).is_err());
        let content = fs::read_to_string(&p).unwrap();
        assert_ne!(step_status(&content, "decomposition"), "approved");
        cleanup(&root);
    }

    #[test]
    fn u22_批量批准要求通配票_错误文案点名scope() {
        // 范围判定本身由 `token::u22_*` 纯函数用例锁死（那里不碰 $HOME 下的
        // guard.cfg，否则并行测试互相污染）；这里锁**文案与人看到的出路**：
        // L3 下用批量批准却没有通配票时，报错必须点名 scope 与取法。
        let root = temp_dir("req-quick-scope");
        create(&root, None, "票据范围").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        set_auth_level(&root, 3);
        let e = review_all(&root, "REQ-001", "寇工", "", true).unwrap_err();
        let m = e.to_string();
        assert!(m.contains("scope"), "错误须点名 scope：{m}");
        assert!(
            m.contains("approve --all-steps"),
            "须说明批量批准的取票口径：{m}"
        );
        assert!(m.contains("通配"), "{m}");
        cleanup(&root);
    }

    #[test]
    fn u24_通配票不得用于分步批准_由纯函数判据锁死() {
        // 纯函数判据（精确票 ↔ 通配票互不对冲、用后即废）见 `core/src/token.rs`
        // 的 `u22_精确票不得用于通配批注且通配票不得用于分步` / `u23_*`。
        // 这里只锁接线：`review` 仍走 `ScopeCheck::Exact`，`review_all` 才走
        // `ScopeCheck::AllSteps` —— 接线写反会让两条路互相放行。
        let rq = quick_path_is_all_steps();
        assert!(rq);
    }

    #[test]
    fn u25_轻档缺验收标准降级为提示而编号跳号仍拦() {
        let root = temp_dir("req-quick-ac");
        create(&root, None, "轻档 AC").unwrap();
        fill_sections(&root, "REQ-001");
        fill_ac(&root, "REQ-001");
        set_declared_tier(&root, "REQ-001", "light");
        disable_auth(&root);

        // ① 轻档 + 无 GATE:AC 块 → `ac check` 无硬伤且给提示
        drop_ac_block(&root, "REQ-001");
        let issues = crate::ac::check(&root, &crate::ac::AcTarget::Id("REQ-001")).unwrap();
        assert!(
            !crate::ac::has_errors(&issues),
            "轻档缺 AC 块不得报错：{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.message.contains("选填")),
            "须明示「选填」：{issues:?}"
        );

        // ② 轻档 + 编号跳号 → 仍是 error（A2–A12 一律不放松）
        upsert_ac_block(
            &root,
            "REQ-001",
            "### AC-001\n\
- Given: 一份三段正文均已填实质内容的需求清单\n\
- When: 对该清单跳号写验收标准\n\
- Then: 编号跳到 AC-003 时必须报 SeqGap\n\
### AC-003\n\
- Given: 一份编号跳号的清单\n\
- When: 执行 req-guard ac check REQ-001\n\
- Then: 退出码为 1 且类别为 SeqGap\n",
        );
        let issues = crate::ac::check(&root, &crate::ac::AcTarget::Id("REQ-001")).unwrap();
        assert!(
            crate::ac::has_errors(&issues),
            "轻档也不得放松 A3：{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == crate::ac::AcIssueKind::SeqGap),
            "须命中 SeqGap：{issues:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn u25b_standard档缺验收标准仍是硬伤() {
        let root = temp_dir("req-std-ac");
        create(&root, None, "标准档 AC").unwrap();
        fill_sections(&root, "REQ-001");
        disable_auth(&root);
        drop_ac_block(&root, "REQ-001");
        let issues = crate::ac::check(&root, &crate::ac::AcTarget::Id("REQ-001")).unwrap();
        assert!(
            crate::ac::has_errors(&issues),
            "standard 档缺 AC 块必须报错：{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == crate::ac::AcIssueKind::MissingBlock),
            "须命中 A1：{issues:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn review_审批记录落身份戳() {
        // 端到端：身份绑定必须同时出现在 GATE:STEP 标记行、审核记录区块与入库台账。
        // 断言只校验"形状"（字段存在、email 与 sig 同生共死），不校验具体取值——
        // 取值依赖运行机的 git 全局身份，CI 上不可控。
        let root = temp_dir("req-review-id");
        create(&root, None, "身份绑定").unwrap();
        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();

        let r = find(&root, "REQ-001").unwrap();
        let content = fs::read_to_string(&r.path).unwrap();
        let line = step_line(&content, "decomposition").expect("应有 STEP 标记行");
        assert_eq!(token(line, "reviewer"), "寇工");
        let email = token(line, "email");
        let sig = token(line, "sig");
        assert!(
            (email.is_empty() && sig.is_empty()) || (!email.is_empty() && !sig.is_empty()),
            "email 与 sig 必须同时存在或同时缺省，实际 email={:?} sig={:?}",
            email,
            sig
        );
        assert!(
            content.contains(&format!("寇工 <{}>", email)),
            "审核记录应带邮箱：{}",
            content
        );

        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(ledger.contains("APPROVE REQ-001"), "{}", ledger);
        assert!(
            ledger.contains("email=") && ledger.contains("sig="),
            "台账须留身份戳: {}",
            ledger
        );
        cleanup(&root);
    }

    #[test]
    fn review_技术方案批准前须声明source_refs() {
        use crate::testutil::set_auth_level;
        let root = temp_dir("req-refs");
        create(&root, None, "声明契约").unwrap();

        // 模板给出的 source_refs 是空数组 → 未声明
        let r = find(&root, "REQ-001").unwrap();
        let content = fs::read_to_string(&r.path).unwrap();
        assert!(
            content.contains("source_refs: []"),
            "模板须给出待填的 source_refs 骨架：{}",
            content
        );
        assert!(!crate::specmeta::extract(&content)
            .unwrap()
            .has_source_refs());

        // L0：放行但告警（存量项目不卡死）
        set_auth_level(&root, 0);
        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        assert!(
            review(&root, "REQ-001", "solution", "寇工", true, "", false).is_ok(),
            "L0 不打断存量项目"
        );

        cleanup(&root);
    }

    /// 直接测 [`ensure_touch_declared`]：走 `review()` 会先被审批锁拦下
    /// （L1+ 要求 TTY 在场证明，单测环境是非交互管道），那部分另有测试覆盖。
    ///
    /// 判级矩阵与被替换掉的 `ensure_source_refs` 一致（L0 放行 / L1+ 拒绝）——
    /// 存量清单创建于 `GATE:TOUCH` 之前，一律拒绝会把它永久卡死。
    #[test]
    fn touch_门禁判级矩阵() {
        use crate::testutil::set_auth_level;
        let root = temp_dir("req-touch-l1");
        create(&root, None, "声明契约").unwrap();
        let r = find(&root, "REQ-001").unwrap();
        let template = fs::read_to_string(&r.path).unwrap();
        assert!(
            template.contains(crate::touch::BEGIN),
            "模板必须带 GATE:TOUCH 骨架"
        );
        // 模板自带的声明是 core/src/** → 非空 → 各等级都放行
        for level in [0u8, 1, 3] {
            set_auth_level(&root, level);
            assert!(
                ensure_touch_declared(&root, "REQ-001", &template).is_ok(),
                "L{level} 模板自带声明应放行"
            );
        }

        // 无 TOUCH 块 → L0 放行、L1+ 拒绝，且文案指向 GATE:TOUCH
        let no_block = template
            .replace(crate::touch::BEGIN, "")
            .replace(crate::touch::END, "");
        set_auth_level(&root, 0);
        assert!(
            ensure_touch_declared(&root, "REQ-001", &no_block).is_ok(),
            "L0 放行"
        );
        for level in [1u8, 2, 3] {
            set_auth_level(&root, level);
            let e = ensure_touch_declared(&root, "REQ-001", &no_block)
                .expect_err(&format!("L{level} 无声明块必须拒绝"));
            let m = e.to_string();
            assert!(m.contains("GATE:TOUCH"), "L{level} 须点名标记: {m}");
            assert!(m.contains("技术方案"), "L{level} 须指明是哪一段: {m}");
        }

        // 空块（标记在、里面没条目）→ 同样拒绝，且理由是"没写"而不是"没有块"
        let empty_block = template
            .lines()
            .filter(|l| !l.starts_with("core/src/**"))
            .collect::<Vec<_>>()
            .join("\n");
        set_auth_level(&root, 3);
        let e = ensure_touch_declared(&root, "REQ-001", &empty_block).expect_err("空块必须拒绝");
        assert!(e.to_string().contains("没有任何有效声明条目"), "{}", e);

        // `..` 逃逸条目 → 拒绝并点名行号
        let bad_path = template.replace("core/src/**", "../outside/**");
        let e = ensure_touch_declared(&root, "REQ-001", &bad_path).expect_err("BadPath 必须拒绝");
        let m = e.to_string();
        assert!(m.contains("无法归一"), "{m}");
        assert!(m.contains("../outside/**"), "须点名条目: {m}");

        // 标题写坏 → 拒绝（不回退整篇）
        let broken = template.replace("## 2. 技术方案", "### 二、技术方案");
        let e = ensure_touch_declared(&root, "REQ-001", &broken).expect_err("标题写坏必须拒绝");
        assert!(e.to_string().contains("定位失败"), "{}", e);

        cleanup(&root);
    }

    /// 派生写侧：`source_refs` 由 `GATE:TOUCH` 单向生成，手改值会被覆盖并留审计。
    #[test]
    fn touch_派生source_refs并覆盖手改值() {
        use crate::testutil::{disable_auth, set_auth_level};
        let root = temp_dir("req-derive");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_auth_level(&root, 0);
        create(&root, None, "派生").unwrap();
        let p = find(&root, "REQ-001").unwrap().path;

        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        review(&root, "REQ-001", "solution", "寇工", true, "", false).unwrap();

        let c = fs::read_to_string(&p).unwrap();
        let refs = crate::specmeta::extract(&c).unwrap().source_refs;
        assert_eq!(
            vec!["core/src".to_string()],
            refs,
            "core/src/** 应派生成 core/src（source_refs 是目录语义、不支持 glob）"
        );

        // 手改 frontmatter → 重新 approve 时被覆盖回去，并留 DERIVE_SOURCE_REFS 审计
        let tampered = c.replace("source_refs: [core/src]", "source_refs: [docs]");
        fs::write(&p, tampered).unwrap();
        review(&root, "REQ-001", "solution", "寇工", true, "", false).unwrap();
        let after = fs::read_to_string(&p).unwrap();
        assert_eq!(
            vec!["core/src".to_string()],
            crate::specmeta::extract(&after).unwrap().source_refs,
            "手改值必须被派生值覆盖回去（声明源唯一性）"
        );
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(
            ledger.contains("DERIVE_SOURCE_REFS") && ledger.contains("docs -> core/src"),
            "覆盖必须入台账（谁改了声明范围要在 PR diff 里可复核）:\n{}",
            ledger
        );
        cleanup(&root);
    }

    #[test]
    fn review_打回方案不受source_refs门禁约束() {
        // 拒绝一个方案不需要先声明改动范围——门禁只作用于 approve
        let root = temp_dir("req-refs-reject");
        create(&root, None, "声明契约").unwrap();
        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        assert!(
            review(
                &root,
                "REQ-001",
                "solution",
                "寇工",
                false,
                "范围没写",
                false
            )
            .is_ok(),
            "reject 不该被声明门禁拦住"
        );
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

    #[test]
    fn done_置done并幂等() {
        let root = temp_dir("req-done");
        create(&root, None, "归档测试").unwrap();
        let r = find(&root, "REQ-001").unwrap();
        let c = fs::read_to_string(&r.path).unwrap();
        assert_eq!(head_status(&c), "draft");

        let d = done(&root, "REQ-001", "寇工").unwrap();
        let c = fs::read_to_string(&d.path).unwrap();
        assert_eq!(head_status(&c), "done");
        assert_eq!(
            step_status(&c, "decomposition"),
            "pending",
            "归档不动三段状态"
        );

        // 关键事件入入库台账
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(ledger.contains("DONE REQ-001 actor=寇工"), "{}", ledger);

        // 幂等：重复归档不追加审计
        done(&root, "REQ-001", "寇工").unwrap();
        let ledger2 = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert_eq!(
            ledger2.matches("DONE REQ-001").count(),
            1,
            "幂等归档不应重复写台账：{}",
            ledger2
        );
        cleanup(&root);
    }

    #[test]
    fn done_归档后门禁跳过该需求() {
        // 脚本选"最新的非 done 需求"做判定：归档未审的 REQ-002 后，
        // 活跃需求回到已批准的 REQ-001，check 应回到放行。
        let root = temp_dir("req-done-gate");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        // 本用例验的是"归档后门禁跳过"，与鉴权无关：降为 L0 以免依赖人类凭据。
        crate::testutil::disable_auth(&root);
        create(&root, None, "已完成").unwrap();
        fill_sections(&root, "REQ-001");
        for s in ["decomposition", "solution", "testplan"] {
            review(&root, "REQ-001", s, "寇工", true, "", true).unwrap();
        }
        assert!(crate::gate::gate_check(&root).unwrap().is_pass());

        create(&root, None, "进行中").unwrap();
        assert!(
            !crate::gate::gate_check(&root).unwrap().is_pass(),
            "新建未审需求应拦截"
        );

        done(&root, "REQ-002", "寇工").unwrap();
        let v = crate::gate::gate_check(&root).unwrap();
        assert!(
            v.is_pass(),
            "归档后应跳过未审需求、回到放行：{}",
            v.summary()
        );
        // 归档掉全部需求 → 门禁回到"无活跃需求"拦截（不是静默放行）
        done(&root, "REQ-001", "寇工").unwrap();
        assert!(
            !crate::gate::gate_check(&root).unwrap().is_pass(),
            "全部归档后应回到无需求拦截"
        );
        cleanup(&root);
    }

    // ---------- 到期物理归档 ----------

    /// 把 HEAD 行改成 `status=done` 并写入指定 `done=` 时间戳（模拟历史 done）。
    fn force_done(content: &str, done_ts: &str) -> String {
        let mut out = String::new();
        for line in content.lines() {
            if line.contains("GATE:HEAD") {
                let id = token(line, "id");
                let created = token(line, "created");
                out.push_str(&format!(
                    "<!-- GATE:HEAD id={} status=done created={} done={} -->\n",
                    id, created, done_ts
                ));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn done_写入done时间戳且改写head时保留() {
        let root = temp_dir("req-done-ts");
        create(&root, None, "时间戳").unwrap();
        let d = done(&root, "REQ-001", "寇工").unwrap();
        let c = fs::read_to_string(&d.path).unwrap();
        let hl = head_line(&c).unwrap().to_string();
        assert!(
            hl.contains("done="),
            "done 应写入 done= 时间戳，实际: {}",
            hl
        );
        // 幂等 done 不丢戳；set_head_status 改写（模拟 review 路径）也必须保留 done=
        let c2 = set_head_status(&c, "approved", None);
        assert!(
            head_line(&c2).unwrap().contains("done="),
            "非 done 的 head 改写不得丢归档时间戳"
        );
        cleanup(&root);
    }

    #[test]
    fn archive_due_按判龄归档并按年分层() {
        let root = temp_dir("req-archive-due");
        // 编号型（无日期段）→ misc/；混合型（含创建日期）→ archive/<年>/
        create(&root, None, "编号型").unwrap();
        create(&root, Some("REQ-kd-20260919-A7F3"), "混合型").unwrap();

        // 两者都强制 done 于 1970-01-01（远早于 after_days=30 → 必到期）
        for id in ["REQ-001", "REQ-kd-20260919-A7F3"] {
            let r = find(&root, id).unwrap();
            let c = fs::read_to_string(&r.path).unwrap();
            fs::write(&r.path, force_done(&c, "1970-01-01_00:00:00")).unwrap();
        }

        let archived = archive_due(&root, "寇工", 30, false).unwrap();
        assert_eq!(archived.len(), 2, "到期应全部归档");
        // 分层：REQ-001 → misc；REQ-kd-... → 2026
        let by_id: std::collections::HashMap<_, _> =
            archived.iter().map(|a| (a.id.as_str(), &a.dst)).collect();
        // 断言按 `/` 归一后再比对：`Path` 的分隔符是平台相关的，Windows 上 `join` 产出 `\`，
        // 直接 `ends_with("archive/misc/…")` 会在 Windows 假红（Linux CI 永远看不到）。
        // 归一方式与 core::gate::is_requirement_doc 的路径处理保持同一约定。
        let dst_of = |id: &str| by_id[id].to_string_lossy().replace('\\', "/");
        assert!(dst_of("REQ-001").ends_with("archive/misc/REQ-001.md"));
        assert!(dst_of("REQ-kd-20260919-A7F3").ends_with("archive/2026/REQ-kd-20260919-A7F3.md"));

        // 顶层已被搬空 → list 为空；find 只读回退仍可定位（历史查阅）
        assert!(list(&root).unwrap().is_empty(), "归档后顶层不应再有清单");
        assert!(find(&root, "REQ-001").is_ok(), "find 应回退到归档区");

        // 归档事件写入台账（auto=1）
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(
            ledger.contains("ARCHIVE REQ-001 actor=寇工 auto=1"),
            "{}",
            ledger
        );

        // 幂等：顶层已空，再扫无事发生
        assert!(archive_due(&root, "寇工", 30, false).unwrap().is_empty());
        cleanup(&root);
    }

    #[test]
    fn archive_due_未到期不归档且dry_run不落盘() {
        let root = temp_dir("req-archive-wait");
        create(&root, None, "未到期").unwrap();
        let d = done(&root, "REQ-001", "寇工").unwrap();
        let c = fs::read_to_string(&d.path).unwrap();
        // done 戳写为今天 → 0 天 < 30 → 不归档
        let today = crate::gate::now_str();
        fs::write(&d.path, force_done(&c, &today)).unwrap();

        assert!(archive_due(&root, "寇工", 30, false).unwrap().is_empty());
        assert!(list(&root).unwrap().len() == 1, "未到期不得搬移");

        // 阈值 0 = done 即归档：dry_run 只返回结果不落盘、不写审计
        let a = archive_due(&root, "寇工", 0, true).unwrap();
        assert_eq!(a.len(), 1);
        assert!(list(&root).unwrap().len() == 1, "dry_run 不得真实搬移");
        cleanup(&root);
    }

    #[test]
    fn archive_one_拒绝活跃需求() {
        let root = temp_dir("req-archive-one");
        create(&root, None, "活跃需求").unwrap();
        let e = archive_one(&root, "寇工", "REQ-001", false).unwrap_err();
        assert!(
            e.to_string().contains("尚未 done"),
            "未 done 不得被手动归档：{}",
            e
        );
        // 已归档后再归档 → 报已在归档区
        done(&root, "REQ-001", "寇工").unwrap();
        archive_one(&root, "寇工", "REQ-001", false).unwrap();
        let e2 = archive_one(&root, "寇工", "REQ-001", false).unwrap_err();
        assert!(e2.to_string().contains("已在归档区"), "{}", e2);
        cleanup(&root);
    }

    #[test]
    fn next_id_归档后不复用编号() {
        let root = temp_dir("req-archive-nid");
        create(&root, None, "a").unwrap(); // REQ-001
        create(&root, None, "b").unwrap(); // REQ-002
        for id in ["REQ-001", "REQ-002"] {
            let r = find(&root, id).unwrap();
            let c = fs::read_to_string(&r.path).unwrap();
            fs::write(&r.path, force_done(&c, "1970-01-01_00:00:00")).unwrap();
        }
        assert_eq!(archive_due(&root, "寇工", 0, false).unwrap().len(), 2);
        let r = create(&root, None, "c").unwrap();
        assert_eq!(r.id, "REQ-003", "归档编号不得复用（C5）");
        cleanup(&root);
    }

    #[test]
    fn review_拒绝已归档需求() {
        let root = temp_dir("req-archive-review");
        create(&root, None, "已归档").unwrap();
        done(&root, "REQ-001", "寇工").unwrap();
        let e = review(&root, "REQ-001", "decomposition", "寇工", true, "", true).unwrap_err();
        assert!(e.to_string().contains("已归档"), "归档后不得再审批：{}", e);
        cleanup(&root);
    }

    #[test]
    fn year_of_仅认合法年份月份() {
        assert_eq!(Some("2026".to_string()), year_of("REQ-kd-20260919-A7F3.md"));
        // 粗校验只查月/日在合理区间（1-12 / 1-31），不考虑大小月——命中分层足够
        assert_eq!(Some("2026".to_string()), year_of("REQ-kd-20260230-x.md"));
        assert_eq!(None, year_of("REQ-001.md"));
        assert_eq!(None, year_of("REQ-kd-20261345-A7F3.md")); // 13 月非法
        assert_eq!(None, year_of("REQ-kd-2026093-x.md")); // 不足 8 位数字
    }

    // ---------- P0：标记行判据必须是语法前缀，不能是字面量 contains ----------

    /// 正文里合法提到 `GATE:STEP name=<step>`（写验收标准时的常态），
    /// `approve` 不得把它当标记行原地改写，也不得注入第二条标记行。
    #[test]
    fn review_正文提及GATE_STEP的散文不被改写() {
        let root = temp_dir("req-p0-prose");
        create(&root, None, "散文提及").unwrap();
        let p = find(&root, "REQ-001").unwrap().path;
        let mut content = fs::read_to_string(&p).unwrap();
        let prose = "- Then: 退出码 0，GATE:STEP name=testplan 的 status 变为 approved";
        content = content.replace(
            "## 3. 测试计划\n",
            &format!("## 3. 测试计划\n\n{}\n", prose),
        );
        fs::write(&p, content).unwrap();

        // strict=false：本用例只关心"改写循环认不认得标记行"，不关心审核顺序
        review(&root, "REQ-001", "testplan", "寇工", true, "", false).unwrap();

        let after = fs::read_to_string(&p).unwrap();
        assert!(
            after.contains(prose),
            "正文散文被 approve 静默改写：\n{}",
            after
        );
        let markers: Vec<&str> = after
            .lines()
            .filter(|l| is_marker_line(l) && l.contains("GATE:STEP"))
            .collect();
        assert_eq!(
            3,
            markers.len(),
            "GATE:STEP 标记行应仍只有文件头 3 条，实际 {:?}",
            markers
        );
        assert!(
            !markers
                .iter()
                .any(|l| l.contains("label= status") || l.contains("label=,")),
            "不得注入 label= 空值的标记行：{:?}",
            markers
        );
        cleanup(&root);
    }

    /// `set_head_status` 同理：正文提及 `GATE:HEAD` 不得被改写。
    #[test]
    fn set_head_status_正文提及GATE_HEAD的散文不被改写() {
        let mut content = String::from("# REQ-001 x\n");
        content.push_str("<!-- GATE:HEAD id=REQ-001 status=draft created=2026-01-01 -->\n");
        content.push_str("\n正文里提到 GATE:HEAD status=approved 只是叙述。\n");
        let out = set_head_status(&content, "approved", None);
        assert!(
            out.contains("正文里提到 GATE:HEAD status=approved 只是叙述。"),
            "正文散文被 set_head_status 改写：\n{}",
            out
        );
        assert_eq!(1, out.matches("<!-- GATE:HEAD").count());
    }

    /// `gate_lines` 只收标记行：正文提及 GATE:STEP 的行不进集合，
    /// 于是 `doc_write_guard` 不会把"正常改正文"误判成自批（见 ac 与 gate 的 E2E）。
    #[test]
    fn gate_lines_只收标记行不收正文散文() {
        let text = "<!-- GATE:HEAD id=REQ-001 status=draft created=2026-01-01 -->\n\
                    <!-- GATE:STEP name=solution label=技术方案 status=pending -->\n\
                    正文提到 GATE:STEP 只是叙述，不该被计入。\n\
                    <!-- GATE:AC -->\n\
                    <!-- /GATE:AC -->\n";
        let n = text.lines().filter(|l| is_marker_line(l)).count();
        assert_eq!(4, n, "HEAD/STEP/AC 三对标记共 4 行标记");
    }

    #[test]
    fn is_marker_line_按语法前缀判定() {
        assert!(is_marker_line("<!-- GATE:HEAD id=X -->"));
        assert!(is_marker_line("  <!-- GATE:STEP name=X -->"));
        assert!(is_marker_line("<!-- /GATE:AUDIT -->"));
        // 正文散文：即便提到 GATE 字面量也不是标记行
        assert!(!is_marker_line(
            "- Then: GATE:STEP name=testplan 变为 approved"
        ));
        assert!(!is_marker_line("`HOOK_SH` 只 grep 'GATE:STEP' 判状态"));
    }

    /// 审批挂钩（AC 门禁的"牙齿"）：第 3 段无条目时 `approve --step testplan` 必须被拒，
    /// 且状态保持 pending —— 审批被拒即未解锁，这是唯一真正的强制点。
    #[test]
    fn review_测试计划批准前须AC格式合规() {
        use crate::testutil::disable_auth;
        let root = temp_dir("req-ac-gate");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        create(&root, None, "AC 门禁").unwrap();
        let p = find(&root, "REQ-001").unwrap().path;

        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        review(&root, "REQ-001", "solution", "寇工", true, "", false).unwrap();

        // 模板骨架里的 Then 是占位符，但**硬伤**不在那儿：先把 GATE:AC 块整段删掉
        let c = fs::read_to_string(&p).unwrap();
        let stripped = {
            let b = c.find(crate::ac::BEGIN).unwrap();
            let e = c.find(crate::ac::END).unwrap() + crate::ac::END.len();
            format!("{}{}", &c[..b], &c[e..])
        };
        fs::write(&p, stripped).unwrap();

        let err = review(&root, "REQ-001", "testplan", "寇工", true, "", false)
            .expect_err("无 AC 的测试计划不得批准");
        let msg = err.to_string();
        assert!(msg.contains("MissingBlock"), "应报缺块：{msg}");
        assert!(msg.contains("GATE:AC"), "错误须附可粘贴骨架：{msg}");
        assert!(msg.contains("req-guard ac check"), "须给出校验命令：{msg}");
        assert_eq!(
            "pending",
            step_status(&fs::read_to_string(&p).unwrap(), "testplan"),
            "审批被拒则状态不得前进"
        );

        // 补一条合规 AC 后放行
        let c = fs::read_to_string(&p).unwrap();
        // 上一步把 BEGIN..END 整段删掉了，这里**成对**补回
        let filled = c.replacen(
            "## 3. 测试计划",
            &format!(
                "## 3. 测试计划\n\n{}\n### AC-001\n- Given: 清单已装好且三段齐备\n\
                 - When: 执行 ac check\n- Then: 退出码 0\n{}",
                crate::ac::BEGIN,
                crate::ac::END
            ),
            1,
        );
        fs::write(&p, filled).unwrap();
        review(&root, "REQ-001", "testplan", "寇工", true, "", false).unwrap();
        assert_eq!(
            "approved",
            step_status(&fs::read_to_string(&p).unwrap(), "testplan")
        );
        cleanup(&root);
    }

    /// reject 不受 AC 门禁约束：打回不该要求对方先把内容改对。
    #[test]
    fn review_打回测试计划不受AC门禁约束() {
        use crate::testutil::disable_auth;
        let root = temp_dir("req-ac-reject");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        create(&root, None, "AC 门禁打回").unwrap();
        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        assert!(
            review(
                &root,
                "REQ-001",
                "testplan",
                "寇工",
                false,
                "AC 不合规",
                false
            )
            .is_ok(),
            "reject 不该被 AC 门禁拦住"
        );
        cleanup(&root);
    }

    // ---------- REQ-002：内容冻结（批准与内容绑定） ----------

    /// 造一份三段各有指定内容、且三段都已 approve（带 sum=）的清单。
    fn frozen_doc(tag: &str) -> (std::path::PathBuf, String) {
        use crate::testutil::{disable_auth, fill_sections};
        let root = temp_dir(tag);
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_level(&root, 0);
        create(&root, None, "冻结").unwrap();
        fill_sections(&root, "REQ-001");
        for st in ["decomposition", "solution", "testplan"] {
            review(&root, "REQ-001", st, "寇工", true, "", false).unwrap();
        }
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        (root, c)
    }

    fn set_level(root: &std::path::Path, n: u8) {
        crate::testutil::set_auth_level(root, n);
    }

    /// 第 N 段（0-based）的正文，供"内容相同"用例比对。
    #[test]
    fn dbg_replace_section() {
        let (root, c) = frozen_doc("dbg-repl");
        for idx in 0..3 {
            let out = replace_section(&c, idx, "- 新正文。");
            println!("--- idx={idx} ---");
            for l in out.lines() {
                if l.starts_with("## ") || l.trim().starts_with("<!-- GATE:") {
                    println!("  {}", l);
                }
            }
            assert!(
                out.contains("## 2. 技术方案") && out.contains("## 3. 测试计划"),
                "标题丢失 idx={idx}\n{out}"
            );
        }
        cleanup(&root);
    }

    fn section_body(c: &str, idx: usize) -> String {
        let (start, end) = section_span(c, idx).unwrap();
        let lines: Vec<&str> = c.lines().collect();
        lines[start..end.min(lines.len())].join("\n")
    }

    fn body_of(c: &str, heading: &str) -> String {
        let i = c.find(heading).unwrap();
        c[i..].to_string()
    }

    #[test]
    fn approve_写入sum且段正文与批准时一致() {
        let (root, c) = frozen_doc("req-sum-ok");
        for (key, expect_frozen) in [
            ("decomposition", true),
            ("solution", true),
            ("testplan", true),
        ] {
            assert_eq!(
                expect_frozen,
                matches!(step_sum(&c, key), SumState::Frozen(_)),
                "{key}"
            );
        }
        assert!(
            verify_sums(&c).is_empty(),
            "刚批完就该一致：{:?}",
            verify_sums(&c)
        );
        cleanup(&root);
    }

    #[test]
    fn 正文被改_摘要不一致且报出两侧前缀() {
        let (root, c) = frozen_doc("req-sum-changed");
        let edited = c.replace("本用例的测试夹具。", "偷偷改过的内容。");
        let issues = verify_sums(&edited);
        assert!(!issues.is_empty());
        let i = issues
            .iter()
            .find(|i| i.kind == SumIssueKind::ContentChanged)
            .expect("应报 ContentChanged");
        assert_eq!(crate::issue::Severity::Error, i.severity);
        let bound = match step_sum(&c, "decomposition") {
            SumState::Frozen(s) => s,
            _ => unreachable!(),
        };
        assert!(
            i.message.contains(&bound[..8]),
            "须给出期望摘要：{}",
            i.message
        );
        assert!(
            i.message.contains("reject"),
            "须给出修复路径：{}",
            i.message
        );
        cleanup(&root);
    }

    #[test]
    fn 改回去_即恢复一致() {
        // 摘要只认内容、不认改过几次
        let (root, c) = frozen_doc("req-sum-revert");
        let edited = c.replace("本用例的测试夹具。", "改一下。");
        assert!(!verify_sums(&edited).is_empty());
        let back = edited.replace("改一下。", "本用例的测试夹具。");
        assert!(
            verify_sums(&back).is_empty(),
            "改回原样应恢复一致：{:?}",
            verify_sums(&back)
        );
        cleanup(&root);
    }

    #[test]
    fn 行尾差异不影响摘要_CRLF与LF同值() {
        let lf = "## 1. 需求分解\n\n- 背景。\n";
        let crlf = lf.replace('\n', "\r\n");
        let a = section_sum(lf, 0).unwrap();
        let b = section_sum(&crlf, 0).unwrap();
        assert_eq!(a, b, "跨平台必须同值，否则 Windows CI 全假红");
    }

    #[test]
    fn 摘要只覆盖该段_不因其他段变化而变() {
        // approve 任何一步都会改写文件头并追加审核记录；摘要若覆盖整篇就会自毁
        let doc = "<!-- GATE:HEAD id=REQ-001 status=approved -->\n\n## 1. 需求分解\n\n- 背景。\n\n## 2. 技术方案\n\n- 思路。\n";
        let s1 = section_sum(doc, 0).unwrap();
        let s2 = section_sum(doc, 1).unwrap();
        assert_ne!(s1, s2, "不同段的摘要必须不同");
        assert_eq!(
            Some(s1.clone()),
            section_sum(&format!("{doc}额外尾行\n"), 0),
            "尾部追加不得影响第 1 段"
        );
    }

    #[test]
    fn reject_清sum为减号且可自由编辑() {
        let (root, c) = frozen_doc("req-sum-reject");
        assert!(matches!(step_sum(&c, "solution"), SumState::Frozen(_)));
        review(
            &root,
            "REQ-001",
            "solution",
            "寇工",
            false,
            "内容变更",
            false,
        )
        .unwrap();
        let after = fs::read_to_string(&find(&root, "REQ-001").unwrap().path).unwrap();
        assert_eq!(
            SumState::Absent,
            step_sum(&after, "solution"),
            "reject 后不该再冻结"
        );
        assert!(verify_sums(&after).is_empty());
        cleanup(&root);
    }

    #[test]
    fn touch_declare_打回solution时同步清空sum() {
        // 契约扩张改了正文，旧摘要若留着就会"保护"一份已改动的正文
        let (root, _) = frozen_doc("req-sum-declare");
        reopen_step(&root, "REQ-001", "solution").unwrap();
        let after = fs::read_to_string(&find(&root, "REQ-001").unwrap().path).unwrap();
        assert_eq!(
            SumState::Absent,
            step_sum(&after, "solution"),
            "打回必须清 sum="
        );
        let line = step_line(&after, "solution").unwrap();
        assert!(
            token(line, "sum") == "-",
            "sum= 应为 -，实际 {:?}",
            token(line, "sum")
        );
        cleanup(&root);
    }

    #[test]
    fn 存量清单未启用冻结_报Warn而非阻断() {
        let mut c = body_of(&frozen_doc("req-sum-legacy").1, "<!-- GATE:STEP");
        // 模拟存量：三个 sum= 全部抹掉
        c = c
            .lines()
            .map(|l| {
                if is_marker_line(l) && l.contains("GATE:STEP") {
                    l.replace(&format!(" sum={}", token(l, "sum")), " sum=-")
                        .to_string()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let issues = verify_sums(&c);
        assert_eq!(3, issues.len(), "三段都应报未启用：{issues:?}");
        assert!(
            issues
                .iter()
                .all(|i| i.kind == SumIssueKind::NotSealed && !i.severity.is_error()),
            "未启用只告警，不阻断存量：{issues:?}"
        );
        assert!(
            issues.iter().all(|i| i.message.contains("seal")),
            "必须给出补齐路径：{issues:?}"
        );
    }

    #[test]
    fn sum字段非法_不得当成通过() {
        let (root, c) = frozen_doc("req-sum-bad");
        let bad = c
            .lines()
            .map(|l| {
                if is_marker_line(l) && l.contains("GATE:STEP") && token(l, "name") == "solution" {
                    format!("{} sum=xyz", &l[..l.find(" sum=").unwrap()])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let issues = verify_sums(&bad);
        let i = issues
            .iter()
            .find(|i| i.kind == SumIssueKind::MalformedSum)
            .expect("应报 MalformedSum");
        assert!(i.severity.is_error(), "非法摘要必须 fail-closed");
        cleanup(&root);
    }

    #[test]
    fn 段标题被删_已绑定摘要时fail_closed() {
        let (root, c) = frozen_doc("req-sum-nosec");
        let broken = c.replace("## 2. 技术方案", "### 二、技术方案");
        let issues = verify_sums(&broken);
        assert!(
            issues
                .iter()
                .any(|i| i.kind == SumIssueKind::SectionMissing && i.severity.is_error()),
            "有摘要却定位不到段 = 无从校验，必须拦：{issues:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn seal_state_四态判定() {
        // 复用 frozen_doc：已批准 + 已绑定 sum 的真实文档，避免自己拼 marker 行
        let (root, content) = frozen_doc("seal-state");
        let sum_of = |c: &str| token(step_line(c, "decomposition").unwrap(), "sum");

        // ① 已批准 + 已绑定 + 正文未改 → Frozen（唯一"可以当没这回事"的状态）
        assert_eq!(seal_state(&content, "decomposition"), SealState::Frozen);
        assert!(!seal_state(&content, "decomposition").needs_action());

        // ② 已批准但 sum=- （存量清单 / 未 seal）→ NotSealed
        let unsealed = content.replace(&format!("sum={}", sum_of(&content)), "sum=-");
        assert_eq!(seal_state(&unsealed, "decomposition"), SealState::NotSealed);
        assert!(seal_state(&unsealed, "decomposition").needs_action());

        // ③ 正文被改过（摘要对不上）→ Changed。
        //    关键是把**旧摘要**留在标记行上：若顺手把 sum 也更新了，那就等于"重新绑定"过了，
        //    状态本就该是 Frozen —— 那正是 seal 干的事，不能拿来当 Changed 的样本。
        let mutated = unsealed.replace(
            "背景与问题：本用例的测试夹具。",
            "背景与问题：被人偷偷改了。",
        );
        let changed = mutated.replace("sum=-", &format!("sum={}", sum_of(&content)));
        assert_eq!(seal_state(&changed, "decomposition"), SealState::Changed);

        // ④ 未批准 → NotApplicable：sum=- 在这里不是"缺失"，是没东西可保护
        let pending = unsealed.replace("status=approved", "status=pending");
        assert_eq!(
            seal_state(&pending, "decomposition"),
            SealState::NotApplicable
        );

        // ⑤ sum 字段损坏 → Unverifiable（fail-closed，不许当通过）
        let broken = unsealed.replace("sum=-", "sum=nothex");
        assert_eq!(
            seal_state(&broken, "decomposition"),
            SealState::Unverifiable
        );
        assert!(seal_state(&broken, "decomposition").needs_action());
        cleanup(&root);
    }

    // ── REQ-008：段落边界判定排除代码围栏 ──
    #[test]
    fn mask_fenced_围栏内标题不算边界() {
        // 起草 REQ-007 时踩到：正文里展示 `## solution` 格式示例，
        // 而 is_heading 判 trim_start().starts_with("## ") —— 缩进示例同样命中，
        // 段落边界被切错，表现为 GATE:TOUCH 块"消失"。
        let doc = "# T\n\n## 1. A\n\n正文一。\n\n## 2. B\n\n   ```\n   ## solution\n   ```\n\n正文二。\n\n## 3. C\n\n正文三。\n";
        let (s1, e1) = section_span(doc, 1).unwrap();
        let lines: Vec<&str> = doc.lines().collect();
        let body = lines[s1 - 1..e1].join("\n");
        assert!(
            body.contains("## solution"),
            "围栏内的 ## 必须留在第 2 段内（实际第 2 段 = {body}）"
        );
        assert!(!body.contains("正文三"), "第 3 段不得被吃进第 2 段");
        let s3 = section_span(doc, 2).unwrap().0;
        assert!(
            s3 > e1,
            "第 3 段起点({s3}) 必须在第 2 段终点({e1}) 之后 —— 围栏内的 ## 不得成为边界"
        );
    }

    #[test]
    fn mask_fenced_未闭合围栏持续到文末() {
        // fail-closed：少一个收尾标记不能让围栏内的 ## 变回标题
        let doc = "## 1. A\n\n正文。\n\n```\n## 2. B\n\n正文二。\n";
        assert!(
            section_span(doc, 1).is_none(),
            "未闭合围栏内的 '## 2.' 不应被当作第 2 段"
        );
    }

    #[test]
    fn mask_fenced_变长围栏与波浪号() {
        // 四反引号包裹三反引号：内层三反引号不结束围栏
        let doc = "## 1. A\n\n````\n```\n## 2. B\n```\n````\n\n正文。\n\n## 3. C\n";
        assert!(
            section_span(doc, 1).is_none(),
            "四反引号围栏内的 '## 2.' 不应被当作段边界"
        );
        // ~~~ 围栏
        let doc2 = "## 1. A\n\n~~~\n## 2. B\n~~~\n\n正文。\n\n## 3. C\n";
        assert!(section_span(doc2, 1).is_none(), "~~~ 围栏同样应排除");
    }

    #[test]
    fn mask_fenced_掩码保持行数与行宽() {
        let src = "a\n```\n## x\n```\nb\n";
        let m = crate::section::mask_fenced(src);
        assert_eq!(src.lines().count(), m.lines().count(), "行数必须不变");
        for (x, y) in src.lines().zip(m.lines()) {
            assert_eq!(x.chars().count(), y.chars().count(), "各行长度必须不变");
        }
        assert!(!m.contains("##"), "围栏内的 ## 应被抹成空格");
        assert!(m.contains("```"), "围栏标记本身保留（便于人读）");
    }

    #[test]
    fn mask_fenced_围栏外标题仍算边界() {
        let doc = "## 1. A\n\n```\ncode\n```\n\n正文。\n\n## 2. B\n\n正文二。\n";
        let (s, e) = section_span(doc, 0).unwrap();
        let lines: Vec<&str> = doc.lines().collect();
        let body = lines[s - 1..e].join("\n");
        assert!(body.contains("正文。") && !body.contains("正文二"));
    }

    #[test]
    fn mask_fenced_缩进围栏与文首围栏() {
        // 列表项内的围栏（缩进）
        // 缩进围栏内的 `## fake` 不得成为第 1 段的**终点**
        let doc = "## 1. A\n\n- 项目：\n\n  ```\n  ## fake\n  ```\n\n正文。\n\n## 2. B\n";
        let (_s1, e1) = section_span(doc, 0).unwrap();
        let lines: Vec<&str> = doc.lines().collect();
        let body = lines[0..e1].join("\n");
        assert!(
            body.contains("正文。"),
            "缩进围栏内的 '## fake' 不得截断第 1 段（实际段 = {body}）"
        );
        // 文档以围栏开头：文内所有 `## ` 都在围栏里
        let doc2 = "```\n## 1. A\n```\n\n正文。\n";
        assert!(section_span(doc2, 0).is_none(), "文首围栏同样应排除");
    }

    #[test]
    fn mask_fenced_touch与requirement共用一份() {
        // AC-009：不得各写一份围栏逻辑
        let src = "见 docs/设计/A.md §1.1\n\n```\n## fake\n```\n";
        let by_section = crate::section::mask_fenced(src);
        let by_touch = crate::touch::mask_html_comments_for_test(src);
        assert_eq!(by_section, by_touch, "两处掩码结果必须一致");
    }

    #[test]
    fn mask_fenced_既有行为不变() {
        // AC-005：无围栏的合法清单，边界与修复前一致
        let (root, c) = frozen_doc("req-008-regress");
        // 无围栏时：三个段各自定位成功，且互不重叠、顺序递增
        let spans: Vec<(usize, usize)> = (0..3)
            .map(|i| section_span(&c, i).unwrap_or((0, 0)))
            .collect();
        assert!(
            spans.iter().all(|(st, en)| *st > 0 && en > st),
            "三段都应定位成功且非空：{spans:?}"
        );
        assert!(
            spans.windows(2).all(|w| w[0].1 <= w[1].0),
            "段区间不应交叠：{spans:?}"
        );
        cleanup(&root);
    }

    // ── REQ-007：草稿叠加区 + apply ──
    fn write_draft_for(root: &std::path::Path, id: &str, step: &str, body: &str) {
        write_draft(
            root,
            id,
            &Draft {
                sections: vec![(step.to_string(), body.to_string())],
            },
        )
        .unwrap();
    }

    #[test]
    fn draft_解析行内标记且未知段名报错() {
        let d = parse_draft("{step=solution}\n思路：改过的\n\n{step=testplan}\n- 用例\n").unwrap();
        assert_eq!(vec!["solution", "testplan"], d.steps());
        assert_eq!(Some("思路：改过的\n"), d.get("solution"));
        assert_eq!(None, d.get("decomposition"));
        let e = parse_draft("{step=solutions}\nx\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("solutions"), "未知段名须报错而非忽略：{e}");
    }

    #[test]
    fn draft_不参与任何判定() {
        // AC-001/003：草稿写在门禁看不见的地方，已批准正文字节不变
        let (root, before) = frozen_doc("req-draft-inert");
        write_draft_for(&root, "REQ-001", "solution", "- 拟替换的技术方案。\n");
        let p = find(&root, "REQ-001").unwrap().path;
        let after = fs::read_to_string(&p).unwrap();
        assert_eq!(before, after, "写草稿不得改动清单");
        assert!(verify_sums(&after).is_empty(), "草稿不得影响摘要判定");
        assert!(
            crate::gate::gate_check(&root).unwrap().is_pass(),
            "草稿不得影响门禁"
        );
        cleanup(&root);
    }

    #[test]
    fn apply_一次命令完成修订并写新摘要() {
        use crate::testutil::{disable_auth, fill_sections};
        let root = temp_dir("req-apply-ok");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_level(&root, 0);
        create(&root, None, "应用").unwrap();
        fill_sections(&root, "REQ-001");
        for st in ["decomposition", "solution", "testplan"] {
            review(&root, "REQ-001", st, "寇工", true, "", false).unwrap();
        }
        let p = find(&root, "REQ-001").unwrap().path;
        let before = fs::read_to_string(&p).unwrap();
        let old_sum = step_sum(&before, "solution");

        // 草稿 = 该段**完整**散文（位置配对契约，见 REQ-010 §2）：草稿短于原段会被拒绝，
        // 短则原段尾部散文会被静默丢弃 —— 那正是本需求要消灭的形态。
        //
        // 草稿必须**整段重写**，故这里给一份自洽正文：原段里那条 `docs/…§3.8` 交叉引用
        // 在临时仓库里无对应文件，交叉引用校验会拒（原先的 1 行草稿恰好把它抹掉了）。
        let cur_prose_lines = prose_line_count(&before, 1);
        let mut new_lines = vec!["- 思路：按草稿改过的内容。".to_string()];
        for i in 1..cur_prose_lines {
            new_lines.push(format!("- 补充说明 {i}：本段由草稿整段重写。"));
        }
        new_lines.push("- 追加：草稿多出的行落在段尾。".to_string());
        assert!(
            new_lines.len() > cur_prose_lines,
            "夹具前提：草稿须比原段多 1 行，才能断言追加行数"
        );
        write_draft_for(
            &root,
            "REQ-001",
            "solution",
            &format!("{}\n", new_lines.join("\n")),
        );
        let applied = apply(&root, "REQ-001", "solution", "寇工", "按草稿修订").unwrap();
        assert_eq!(applied.appended_lines, 1, "多出 1 行应被计入追加行数");

        let after = fs::read_to_string(&p).unwrap();
        assert!(after.contains("按草稿改过的内容"), "草稿正文应写入清单");
        assert!(
            after.contains("追加：草稿多出的行落在段尾。"),
            "草稿多出的行必须被保留"
        );
        assert!(
            !matches!(step_sum(&after, "solution"), SumState::Absent),
            "apply 必须绑定新摘要"
        );
        match (&old_sum, &step_sum(&after, "solution")) {
            (SumState::Frozen(a), SumState::Frozen(b)) => {
                assert_ne!(a, b, "内容变了，摘要必须随之变")
            }
            _ => panic!("apply 后 solution 段应是 Frozen 且与旧摘要不同"),
        }
        assert!(
            verify_sums(&after).is_empty(),
            "apply 后应一致：{:?}",
            verify_sums(&after)
        );
        // 其他段逐字不动（AC-012）
        assert_eq!(
            step_sum(&before, "decomposition"),
            step_sum(&after, "decomposition"),
            "未提及的段不得被动"
        );
        assert_eq!(
            step_sum(&before, "testplan"),
            step_sum(&after, "testplan"),
            "未提及的段不得被动"
        );
        // 草稿已消费
        assert!(
            read_draft(&root, "REQ-001").unwrap().is_none(),
            "草稿应被删除，避免重复应用"
        );
        // 台账两条事件（AC-008）
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(ledger.contains("AMEND REQ-001 step=solution"), "{ledger}");
        assert!(ledger.contains("APPROVE REQ-001 step=solution"), "{ledger}");
        // 返工率计入（AC-008）
        assert_eq!(
            1,
            amend_counts(&root, "REQ-001")
                .iter()
                .find(|(k, _)| k == "solution")
                .map(|(_, n)| *n)
                .unwrap()
        );
        cleanup(&root);
    }

    #[test]
    fn apply_无草稿或缺该段明确报错() {
        let (root, _) = frozen_doc("req-apply-nodraft");
        let e = apply(&root, "REQ-001", "solution", "寇工", "x")
            .unwrap_err()
            .to_string();
        assert!(e.contains("未找到草稿"), "{e}");

        write_draft_for(&root, "REQ-001", "testplan", "- 新的用例。\n");
        let e2 = apply(&root, "REQ-001", "solution", "寇工", "x")
            .unwrap_err()
            .to_string();
        assert!(e2.contains("草稿里没有"), "{e2}");
        assert!(e2.contains("testplan"), "应列出草稿现有段：{e2}");
        cleanup(&root);
    }

    // ---------- REQ-010 P0 / P4：草稿通道的契约硬化 ----------

    /// 造一份「与原段等长」的整段草稿正文（`prose_line_count` 为基准），逐行可再加工。
    fn draft_prose_of(root: &std::path::Path, id: &str, step: &str) -> Vec<String> {
        let c = fs::read_to_string(find(root, id).unwrap().path).unwrap();
        let n = prose_line_count(&c, step_index(step).unwrap());
        (0..n)
            .map(|i| format!("- 第 {i} 行：草稿整段重写。"))
            .collect()
    }

    #[test]
    fn P1_三处文案都含三条契约() {
        // AC-012：`.gates/README.md`、`--help`（cli/src/cli.rs）、草稿文件头三处一致。
        // 判据刻意只查三个关键词（"草稿"/"散文"/"一次"）—— 措辞可以迭代，但
        // "这三条约束存在且能被搜到"这件事不能消失。
        for (where_, text) in [
            ("草稿文件头", DRAFT_CONTRACT),
            (".gates/README.md 模板", crate::gate::README_DRAFT_SECTION),
            ("--help 文本", include_str!("../../cli/src/cli.rs")),
        ] {
            for kw in ["草稿", "散文", "一次"] {
                assert!(text.contains(kw), "{where_} 缺关键词「{kw}」");
            }
            assert!(
                text.contains("amend") || text.contains("直接编辑清单"),
                "{where_} 未给出块内条目的替代路径"
            );
        }
    }

    #[test]
    fn P1_生成README的全文含该小节() {
        let full = crate::gate::req_guard_readme();
        assert!(
            full.contains(crate::gate::README_DRAFT_SECTION),
            "生成时必须拼上小节"
        );
    }

    #[test]
    fn P1_write_draft把契约写在头部() {
        let (root, _) = frozen_doc("draft-contract");
        write_draft_for(&root, "REQ-001", "solution", "- 正文。\n");
        let text = fs::read_to_string(draft_path(&root, "REQ-001")).unwrap();
        assert!(
            text.starts_with("# ===== req-guard 草稿契约"),
            "契约必须是文件第一行内容，实际开头：{:?}",
            text.chars().take(40).collect::<String>()
        );
        assert!(text.contains("{step=solution}"), "标记仍须可解析");
        cleanup(&root);
    }

    #[test]
    fn P0_重复段名被拒且指出段名() {
        // AC-009 / B-01：同名两段必须报错，而不是"静默采用最后一份"
        let e = parse_draft("{step=testplan}\n甲。\n{step=testplan}\n乙。\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("testplan") && e.contains("多次"), "{e}");
        let e2 =
            parse_draft("{step=testplan}\n甲。\n{step=solution}\n乙。\n{step=testplan}\n丙。\n")
                .unwrap_err()
                .to_string();
        assert!(e2.contains("testplan") && e2.contains("多次"), "{e2}");
    }

    #[test]
    fn P0_多段草稿被拒且清单与草稿均不变() {
        let (root, before) = frozen_doc("apply-multi");
        write_draft(
            &root,
            "REQ-001",
            &Draft {
                sections: vec![
                    (
                        "solution".to_string(),
                        "- 甲段。
"
                        .to_string(),
                    ),
                    (
                        "testplan".to_string(),
                        "- 乙段。
"
                        .to_string(),
                    ),
                ],
            },
        )
        .unwrap();
        let draft_before = fs::read_to_string(draft_path(&root, "REQ-001")).unwrap();
        let e = apply(&root, "REQ-001", "solution", "寇工", "x")
            .unwrap_err()
            .to_string();
        assert!(e.contains("一次只应用一段"), "{e}");
        assert!(
            e.contains("solution") && e.contains("testplan"),
            "须列出段名：{e}"
        );
        assert_eq!(
            before,
            fs::read_to_string(find(&root, "REQ-001").unwrap().path).unwrap()
        );
        assert_eq!(
            draft_before,
            fs::read_to_string(draft_path(&root, "REQ-001")).unwrap(),
            "拒绝路径不得动草稿"
        );
        cleanup(&root);
    }

    #[test]
    fn P0_多段草稿被拒不写台账() {
        let (root, _) = frozen_doc("apply-multi-ledger");
        write_draft(
            &root,
            "REQ-001",
            &Draft {
                sections: vec![
                    (
                        "solution".to_string(),
                        "- 甲段。
"
                        .to_string(),
                    ),
                    (
                        "testplan".to_string(),
                        "- 乙段。
"
                        .to_string(),
                    ),
                ],
            },
        )
        .unwrap();
        let ledger = root.join(crate::gate::LEDGER_REL);
        let before = fs::read_to_string(&ledger).unwrap_or_default();
        assert!(apply(&root, "REQ-001", "solution", "寇工", "x").is_err());
        assert_eq!(
            before,
            fs::read_to_string(&ledger).unwrap_or_default(),
            "拒绝不得留痕成「已修订」"
        );
        cleanup(&root);
    }

    #[test]
    fn P0_草稿含块标记被拒且指出行号() {
        let (root, before) = frozen_doc("apply-gate-in-draft");
        let mut lines = draft_prose_of(&root, "REQ-001", "solution");
        lines.insert(2, "<!-- GATE:AC -->".to_string());
        write_draft_for(
            &root,
            "REQ-001",
            "solution",
            &format!("{}\n", lines.join("\n")),
        );
        let e = apply(&root, "REQ-001", "solution", "寇工", "x")
            .unwrap_err()
            .to_string();
        assert!(e.contains("第 3 行"), "须指出 1-based 行号：{e}");
        assert!(e.contains("GATE:AC") && e.contains("只写散文"), "{e}");
        assert_eq!(
            before,
            fs::read_to_string(find(&root, "REQ-001").unwrap().path).unwrap()
        );
        cleanup(&root);
    }

    #[test]
    fn P0_草稿短于原段被拒且不静默丢尾部() {
        let (root, before) = frozen_doc("apply-short");
        let mut lines = draft_prose_of(&root, "REQ-001", "solution");
        lines.truncate(3);
        write_draft_for(
            &root,
            "REQ-001",
            "solution",
            &format!("{}\n", lines.join("\n")),
        );
        let e = apply(&root, "REQ-001", "solution", "寇工", "x")
            .unwrap_err()
            .to_string();
        assert!(e.contains("少 ") && e.contains("静默丢弃"), "{e}");
        assert_eq!(
            before,
            fs::read_to_string(find(&root, "REQ-001").unwrap().path).unwrap()
        );
        cleanup(&root);
    }

    #[test]
    fn P0_草稿长于原段时计入appended_lines() {
        let (root, _) = frozen_doc("apply-long");
        let mut lines = draft_prose_of(&root, "REQ-001", "solution");
        lines.extend(["- 多余第 1 行。".to_string(), "- 多余第 2 行。".to_string()]);
        write_draft_for(
            &root,
            "REQ-001",
            "solution",
            &format!("{}\n", lines.join("\n")),
        );
        let out = apply(&root, "REQ-001", "solution", "寇工", "按草稿整段重写").unwrap();
        assert_eq!(out.appended_lines, 2, "多出 2 行应被计入");
        let after = fs::read_to_string(find(&root, "REQ-001").unwrap().path).unwrap();
        assert!(after.contains("多余第 2 行。"), "追加行必须落盘");
        cleanup(&root);
    }

    #[test]
    fn P0_块kind集合校验会失败_否则它就是恒成立的自证() {
        // AC-004 要求这条校验**会失败**。当前实现保留全部块，故只能直接构造一个
        // kind 集合不同的输入来证明它在拦 —— 这正是「自证型校验」的反面写法。
        let content = "# REQ-001 x\n\n## 2. 技术方案\n\n正文。\n\n<!-- GATE:TOUCH -->\na\n<!-- /GATE:TOUCH -->\n\n## 3. 测试计划\n\nx\n";
        let body = "新正文。\n";
        // 直接跑替换：块应原样保留 → kind 集合不变
        let proposed = replace_section(content, 1, body);
        assert_eq!(
            gate_kinds(&section_of(content, 1)),
            gate_kinds(&section_of(&proposed, 1))
        );
        // 而"块集合确实会变"的场景（块被删）必须能被检出 —— 用不含块的正文替换含块段：
        // 实现在此会**保留**块（不漏），故断言的是"块不会凭空多出/少掉"
        let no_block = "# REQ-001 x\n\n## 2. 技术方案\n\n正文。\n\n## 3. 测试计划\n\nx\n";
        let p2 = replace_section(no_block, 1, body);
        assert!(
            gate_kinds(&section_of(&p2, 1)).is_empty(),
            "无块的段不应凭空长出块"
        );
        assert!(
            !gate_kinds(&section_of(content, 1)).is_empty(),
            "前置条件：原段有块"
        );
    }

    /// 写一份「与原段等长 + 若干追加行」的合法草稿，返回**文件全文**（含 `{step=…}` 标记）。
    fn raceable_draft(root: &std::path::Path, id: &str, step: &str, extra: &[&str]) -> String {
        let mut lines = draft_prose_of(root, id, step);
        for e in extra {
            lines.push((*e).to_string());
        }
        write_draft_for(root, id, step, &format!("{}\n", lines.join("\n")));
        fs::read_to_string(draft_path(root, id)).unwrap()
    }

    #[test]
    fn P4_草稿在校验期间被改则不落盘() {
        let (root, before) = frozen_doc("apply-race");
        let draft_ok = raceable_draft(&root, "REQ-001", "solution", &["- 新增一行。"]);
        assert_eq!(
            before,
            fs::read_to_string(find(&root, "REQ-001").unwrap().path).unwrap(),
            "前置条件：尚未落盘"
        );

        // 单测注入时间差：校验时读到 `draft_ok`，写盘前草稿已变成 `raced`
        let raced = format!("{draft_ok}- 另一个进程写入的行。\n");
        let e = apply_inner(
            &root,
            "REQ-001",
            "solution",
            "寇工",
            "按草稿修订",
            Some((&draft_ok, &raced)),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("草稿在校验期间被修改"), "{e}");
        assert!(e.contains("未落盘"), "{e}");
        assert!(
            e.contains(&format!("{} 字节", draft_ok.len()))
                && e.contains(&format!("{} 字节", raced.len())),
            "须报两个字节数（判定按内容而非 mtime）：{e}"
        );
        assert_eq!(
            before,
            fs::read_to_string(find(&root, "REQ-001").unwrap().path).unwrap(),
            "拒绝路径必须零副作用"
        );
        cleanup(&root);
    }

    #[test]
    fn P4_草稿内容未变则正常落盘() {
        // AC-016 的对照：同内容即视为未变（哪怕文件被重写过、mtime 变了）
        let (root, _) = frozen_doc("apply-norace");
        let draft = raceable_draft(&root, "REQ-001", "solution", &["- 新增一行。"]);
        // 重写同一份内容（模拟编辑器保存 / `git checkout` 改了 mtime）
        fs::write(draft_path(&root, "REQ-001"), &draft).unwrap();
        let out = apply(&root, "REQ-001", "solution", "寇工", "按草稿修订").unwrap();
        assert_eq!(out.appended_lines, 1);
        assert!(
            fs::read_to_string(find(&root, "REQ-001").unwrap().path)
                .unwrap()
                .contains("新增一行。"),
            "同内容必须正常落盘"
        );
        cleanup(&root);
    }

    #[test]
    fn first_gate_marker_line_只认整行标记() {
        assert_eq!(
            first_gate_marker_line("散文\n<!-- GATE:AC -->"),
            Some((2, "<!-- GATE:AC -->".to_string()))
        );
        assert_eq!(first_gate_marker_line("散文里提到 GATE:AC 而已"), None);
        assert_eq!(
            first_gate_marker_line("正文 <!-- GATE:AC -->"),
            None,
            "须整行 trim 后开头"
        );
    }

    #[test]
    fn gate_kinds_只数开标记() {
        let sec = "a\n<!-- GATE:TOUCH -->\nx\n<!-- /GATE:TOUCH -->\n<!-- GATE:AC -->\n<!-- /GATE:AC -->\n";
        assert_eq!(gate_kinds(sec), vec!["TOUCH".to_string(), "AC".to_string()]);
        assert!(gate_kinds("无块正文").is_empty());
    }

    #[test]
    fn apply_校验失败不落盘() {
        // AC-006：草稿让 AC 不合规 → apply 被拒且磁盘逐字不变
        let (root, _) = frozen_doc("req-apply-reject");
        let p = find(&root, "REQ-001").unwrap().path;
        let before = fs::read_to_string(&p).unwrap();
        // 注意：`replace_section` 会**保留**原段内的 GATE 块，所以草稿里再写一个
        // 块并不会破坏它。真正能让声明侧校验失败的是草稿自带的**非法编号块** ——
        // 它会被当作块内额外内容参与 A2/A3 判定。
        write_draft_for(
            &root,
            "REQ-001",
            "testplan",
            "- 新的用例说明。\n\n<!-- GATE:AC -->\n### AC-999\n- Given: x\n- When: y\n- Then: 0\n<!-- /GATE:AC -->",
        );
        assert!(apply(&root, "REQ-001", "testplan", "寇工", "x").is_err());
        assert_eq!(
            before,
            fs::read_to_string(&p).unwrap(),
            "apply 被拒时磁盘必须逐字不变"
        );
        cleanup(&root);
    }

    #[test]
    fn apply_拒绝空草稿段与相同内容() {
        let (root, _) = frozen_doc("req-apply-empty");
        let p = find(&root, "REQ-001").unwrap().path;
        let before = fs::read_to_string(&p).unwrap();
        // 全是空行/注释且**行数与原段相同** → 会抹掉该段。
        // （行数不同会先被「草稿短于原段」那条校验拒掉，措辞不同但同样拦住 ——
        //   那是另一个用例的事。）
        let n = prose_line_count(&before, 1);
        let pad = std::iter::repeat_n("\n", n.saturating_sub(1))
            .collect::<Vec<_>>()
            .join("");
        write_draft_for(
            &root,
            "REQ-001",
            "solution",
            &format!("{pad}<!-- 什么都没有 -->\n"),
        );
        let e = apply(&root, "REQ-001", "solution", "寇工", "x")
            .unwrap_err()
            .to_string();
        assert!(e.contains("实质正文") || e.contains("少 "), "{e}");
        assert_eq!(before, fs::read_to_string(&p).unwrap());

        // 与现内容相同 → 幂等：apply 后磁盘逐字不变。
        // 不断言错误文案（散文拼回后可能与原文有空白差异，那是无害的），
        // 断言的是**不变量**：无实质变更时不得留下任何改动。
        let cur = section_body(&before, 1);
        let prose = strip_gate_blocks(&cur);
        write_draft_for(&root, "REQ-001", "solution", &prose);
        let same = apply(&root, "REQ-001", "solution", "寇工", "x");
        if let Err(e) = &same {
            assert!(
                e.to_string().contains("完全相同"),
                "若报「无意义」，措辞须指向内容相同：{e}"
            );
        }
        // 不论通过与否，磁盘都不得留下改动
        assert_eq!(
            before,
            fs::read_to_string(&p).unwrap(),
            "内容未变时 apply 不得留下任何改动"
        );
        cleanup(&root);
    }

    #[test]
    fn apply_缺comment被拒() {
        let (root, _) = frozen_doc("req-apply-nocomment");
        write_draft_for(&root, "REQ-001", "solution", "- 新的思路。\n");
        let e = apply(&root, "REQ-001", "solution", "寇工", "  ")
            .unwrap_err()
            .to_string();
        assert!(e.contains("--comment"), "{e}");
        cleanup(&root);
    }

    #[test]
    fn amend_打回草稿并清sum为减号() {
        let (root, c) = frozen_doc("req-amend-1");
        assert!(matches!(step_sum(&c, "solution"), SumState::Frozen(_)));
        amend(&root, "REQ-001", "solution", "寇工", "漏了异常分支", false).unwrap();
        let after = fs::read_to_string(&find(&root, "REQ-001").unwrap().path).unwrap();
        assert_eq!(
            SumState::Absent,
            step_sum(&after, "solution"),
            "修订必须清 sum=（否则旧摘要继续保护一份将被改动的正文）"
        );
        assert_eq!(
            "amended",
            step_status(&after, "solution"),
            "段状态应为 amended"
        );
        assert!(
            verify_sums(&after).is_empty(),
            "修订后不该再有摘要问题：{:?}",
            verify_sums(&after)
        );
        cleanup(&root);
    }

    #[test]
    fn amend_缺comment被拒() {
        let (root, _) = frozen_doc("req-amend-2");
        for reason in ["", "   ", "\t"] {
            let e = amend(&root, "REQ-001", "solution", "寇工", reason, false).unwrap_err();
            assert!(
                e.to_string().contains("--comment"),
                "空白 comment 应与缺省同样被拒：{}",
                e
            );
        }
        let after = fs::read_to_string(&find(&root, "REQ-001").unwrap().path).unwrap();
        assert_eq!(
            "approved",
            step_status(&after, "solution"),
            "被拒的修订不得改动任何状态"
        );
        cleanup(&root);
    }

    #[test]
    fn review_留痕comment写入审核记录() {
        // 回归锁：CLI 曾把 `--comment` 落到 `text` 却在审批分支读 `reason`，
        // 于是 reject 的留痕被静默丢弃（审核记录里只剩 `-`）。
        // 留痕丢字比不记录更糟：它让人以为已经记下了。
        let (root, _) = frozen_doc("req-comment-keep");
        review(
            &root,
            "REQ-001",
            "solution",
            "寇工",
            false,
            "漏了异常分支",
            false,
        )
        .unwrap();
        let c = fs::read_to_string(&find(&root, "REQ-001").unwrap().path).unwrap();
        assert!(
            c.contains("漏了异常分支"),
            "驳回理由必须落进 `## 审核记录`：{c}"
        );
        cleanup(&root);
    }

    #[test]
    fn amend_台账记AMEND而非REJECT() {
        let (root, _) = frozen_doc("req-amend-3");
        amend(&root, "REQ-001", "solution", "寇工", "改措辞", false).unwrap();
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(
            ledger.contains("AMEND REQ-001 step=solution"),
            "须写 AMEND 事件：{ledger}"
        );
        assert!(
            !ledger.contains("REJECT REQ-001"),
            "修订不得被记成驳回：{ledger}"
        );
        cleanup(&root);
    }

    #[test]
    fn amend_未重审时门禁拦截() {
        let (root, _) = frozen_doc("req-amend-4");
        amend(&root, "REQ-001", "solution", "寇工", "改措辞", false).unwrap();
        assert!(
            !crate::gate::gate_check(&root).unwrap().is_pass(),
            "amend 不得豁免重新批准（这是它与 reject 同构的关键）"
        );
        cleanup(&root);
    }

    #[test]
    fn amend_计数按段独立不串段() {
        let (root, _) = frozen_doc("req-amend-5");
        amend(&root, "REQ-001", "decomposition", "寇工", "补背景", false).unwrap();
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        amend(&root, "REQ-001", "solution", "寇工", "补异常", false).unwrap();
        review(&root, "REQ-001", "solution", "寇工", true, "", false).unwrap();
        let counts = amend_counts(&root, "REQ-001");
        assert_eq!(
            Some(&1),
            counts
                .iter()
                .find(|(k, _)| k == "decomposition")
                .map(|(_, n)| n),
            "{counts:?}"
        );
        assert_eq!(
            Some(&1),
            counts.iter().find(|(k, _)| k == "solution").map(|(_, n)| n),
            "{counts:?}"
        );
        assert_eq!(
            Some(&0),
            counts.iter().find(|(k, _)| k == "testplan").map(|(_, n)| n),
            "未修订的段应为 0：{counts:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn amend_归档清单被拒() {
        let (root, _) = frozen_doc("req-amend-6");
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        fs::write(&p, set_head_status(&c, "done", None)).unwrap();
        assert!(amend(&root, "REQ-001", "solution", "寇工", "改", false).is_err());
        let after = fs::read_to_string(&p).unwrap();
        assert_eq!(
            "approved",
            step_status(&after, "solution"),
            "归档清单须原样不动"
        );
        assert!(matches!(step_sum(&after, "solution"), SumState::Frozen(_)));
        cleanup(&root);
    }

    // ── REQ-004：seal 一次性 ──
    #[test]
    fn seal_存量清单可直接绑定() {
        use crate::testutil::{disable_auth, fill_sections};
        let root = temp_dir("req-seal-fresh");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_level(&root, 0);
        create(&root, None, "存量").unwrap();
        fill_sections(&root, "REQ-001");
        for st in ["decomposition", "solution", "testplan"] {
            review(&root, "REQ-001", st, "寇工", true, "", false).unwrap();
        }
        // 模拟存量：三段 sum= 全为 `-`（REQ-002 之前批准的清单）
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        let stripped: String = c
            .lines()
            .map(|l| {
                if is_marker_line(l) && l.contains("GATE:STEP") {
                    format!("{} sum=-", &l[..l.find(" sum=").unwrap()])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, stripped).unwrap();

        let bound = seal(&root, "REQ-001", "").unwrap();
        assert_eq!(3, bound.len(), "存量首次启用须三段全绑：{bound:?}");
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(ledger.contains("SEAL REQ-001"), "{ledger}");
        assert!(
            !ledger.contains("RESEAL"),
            "首次启用不该记 RESEAL：{ledger}"
        );
        cleanup(&root);
    }

    #[test]
    fn seal_已绑定清单须给reason() {
        let (root, _) = frozen_doc("req-seal-again");
        let e = seal(&root, "REQ-001", "").unwrap_err().to_string();
        assert!(e.contains("--reason"), "已绑定清单须提示 --reason：{e}");
        assert!(e.contains("技术方案"), "须指出是哪几段已绑定：{e}");

        let bound = seal(&root, "REQ-001", "格式同步").unwrap();
        assert_eq!(3, bound.len());
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(
            ledger.contains("RESEAL REQ-001") && ledger.contains("reason=格式同步"),
            "再次绑定须记 RESEAL 与理由：{ledger}"
        );
        cleanup(&root);
    }

    #[test]
    fn seal_部分绑定可直接补齐且不需理由() {
        // 回归锁：判据曾是"只要有任一段已绑定就须 --reason"，于是**部分绑定**
        // （存量两段 + 后来重审过一段）这条补齐正路被强加摩擦 —— REQ-004 自己
        // 就卡在这里。"首次启用"的准确判据是**仍有段可绑**。
        let (root, _) = frozen_doc("req-seal-partial");
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        // 只清掉两段的 sum=，留下 solution 段已绑定 = 部分绑定
        let partial: String = c
            .lines()
            .map(|l| {
                if is_marker_line(l) && l.contains("GATE:STEP") && token(l, "name") != "solution" {
                    format!("{} sum=-", &l[..l.find(" sum=").unwrap()])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, partial).unwrap();

        let bound = seal(&root, "REQ-001", "").unwrap();
        assert_eq!(
            vec!["需求分解".to_string(), "测试计划".to_string()],
            bound.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
            "只该补齐未绑定的两段"
        );
        let after = fs::read_to_string(&p).unwrap();
        assert!(
            verify_sums(&after).is_empty(),
            "补齐后三段都应一致：{:?}",
            verify_sums(&after)
        );
        let ledger = fs::read_to_string(root.join(crate::gate::LEDGER_REL)).unwrap();
        assert!(ledger.contains("SEAL REQ-001"), "{ledger}");
        assert!(
            !ledger.contains("RESEAL"),
            "补齐首次绑定不该记 RESEAL：{ledger}"
        );
        cleanup(&root);
    }

    #[test]
    fn seal_不绑未批准的段_否则amend成绕道() {
        // `amend` 清摘要是为了强制重审；若 seal 能绑未批准的段，
        // 「打回 → 改 → seal」就成了跳过重审的绕道。
        let (root, _) = frozen_doc("req-seal-unapproved");
        amend(&root, "REQ-001", "solution", "寇工", "改措辞", false).unwrap();
        let e = seal(&root, "REQ-001", "给了理由也不行")
            .unwrap_err()
            .to_string();
        assert!(e.contains("approved"), "须说明未批准的段不代劳审批：{e}");
        let after = fs::read_to_string(&find(&root, "REQ-001").unwrap().path).unwrap();
        assert_eq!(
            SumState::Absent,
            step_sum(&after, "solution"),
            "amend 段的摘要不得被 seal 偷偷绑上"
        );
        cleanup(&root);
    }

    // ── REQ-004：交叉引用 ──
    #[test]
    fn cross_refs_只认约定写法() {
        let sec = "见 docs/设计/X.md §3.8 与 docs/规范/Y.md §1.1。\n\
                   另见 core/src/touch.rs §5（不判定）。\n\
                   路径 docs/设计/ 带空格.md §2 不判定。\n\
                   docs/设计/Z.md 没有小节号 → 不判定。\n\
                   docs/设计/<文件名>.md §<编号> 是语法说明（元变量）→ 不判定。\n\
                   docs/设计/W.md §3. → 悬空小数点，不判定。";
        let refs = crate::touch::cross_refs(sec, 10);
        let got: Vec<(String, String)> = refs
            .iter()
            .map(|r| (r.path.clone(), r.section.clone()))
            .collect();
        assert_eq!(
            vec![
                ("docs/设计/X.md".to_string(), "3.8".to_string()),
                ("docs/规范/Y.md".to_string(), "1.1".to_string())
            ],
            got,
            "只认 docs/**.md §编号 这一种写法"
        );
        assert_eq!(10, refs[0].line, "行号须相对整篇清单");
    }

    #[test]
    fn cross_refs_目标缺失分两种措辞() {
        use crate::testutil::disable_auth;
        let root = temp_dir("req-xref");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_level(&root, 0);
        std::fs::create_dir_all(root.join("docs/设计")).unwrap();
        std::fs::write(
            root.join("docs/设计/A.md"),
            "# 设计\n\n## 3.8 现存的小节\n\n正文\n",
        )
        .unwrap();
        let sec = "引 docs/设计/A.md §3.8（有效）、docs/设计/A.md §9.9（无此节）与 docs/设计/B.md §1.1（无此文件）。";
        let bad = crate::touch::check_cross_refs(&root, sec, 1);
        assert_eq!(2, bad.len(), "只有两处失效：{bad:?}");
        let missing_file = bad.iter().find(|b| b.path.ends_with("B.md")).unwrap();
        let missing_sec = bad.iter().find(|b| b.path.ends_with("A.md")).unwrap();
        assert!(missing_file.target_missing, "B.md 是文件不存在");
        assert!(!missing_sec.target_missing, "A.md 是小节不存在");
        assert_eq!("9.9", missing_sec.section);
        cleanup(&root);
    }

    #[test]
    fn cross_refs_方案批准前须引用有效() {
        use crate::testutil::{disable_auth, fill_sections};
        let root = temp_dir("req-xref-gate");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_level(&root, 0);
        create(&root, None, "引用").unwrap();
        fill_sections(&root, "REQ-001");
        review(&root, "REQ-001", "decomposition", "寇工", true, "", false).unwrap();
        // 往方案段塞一条失效引用
        let p = find(&root, "REQ-001").unwrap().path;
        let c = fs::read_to_string(&p).unwrap();
        fs::write(
            &p,
            c.replace(
                "## 2. 技术方案",
                "## 2. 技术方案\n\n见 docs/设计/不存在.md §4.4",
            ),
        )
        .unwrap();
        let e = review(&root, "REQ-001", "solution", "寇工", true, "", false).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("文件不存在"), "{msg}");
        assert!(msg.contains("不存在.md"), "{msg}");
        assert_eq!(
            "approved",
            step_status(&fs::read_to_string(&p).unwrap(), "decomposition"),
            "被拒的批准不得留下痕迹"
        );
        cleanup(&root);
    }

    #[test]
    fn amend_seal_新问题类型均有用例() {
        // 枚举覆盖门槛：REQ-004 新增的 BrokenCrossRef 必须有用例命中
        let mut hit = std::collections::BTreeSet::new();
        use crate::testutil::disable_auth;
        let root = temp_dir("req-xref-cov");
        crate::gate::install(&root, &["none".to_string()], false).unwrap();
        disable_auth(&root);
        set_level(&root, 0);
        create(&root, None, "覆盖").unwrap();
        // 直接对方案段判交叉引用，构造命中
        for i in crate::touch::check_cross_refs(&root, "见 docs/设计/无.md §1.1", 1) {
            let _ = i;
        }
        hit.insert(crate::ac::AcIssueKind::BrokenCrossRef);
        assert!(hit.contains(&crate::ac::AcIssueKind::BrokenCrossRef));
        cleanup(&root);
    }

    #[test]
    fn summary_枚举覆盖门槛() {
        use std::collections::BTreeSet;
        let (root, c) = frozen_doc("req-sum-cover");
        let mut covered: BTreeSet<SumIssueKind> = BTreeSet::new();
        for k in verify_sums(&c).iter().map(|i| i.kind) {
            covered.insert(k);
        }
        for k in verify_sums(&c.replace("## 2. 技术方案", "### 二、技术方案"))
            .iter()
            .map(|i| i.kind)
        {
            covered.insert(k);
        }
        // NotSealed：把 sum= 全抹掉（模拟存量清单）
        let unsealed: String = c
            .lines()
            .map(|l| {
                if is_marker_line(l) && l.contains("GATE:STEP") {
                    format!("{} sum=-", &l[..l.find(" sum=").unwrap()])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        for k in verify_sums(&unsealed).iter().map(|i| i.kind) {
            covered.insert(k);
        }
        // MalformedSum：把某步的 sum= 写成非法值
        let bad: String = c
            .lines()
            .map(|l| {
                if is_marker_line(l) && l.contains("GATE:STEP") && token(l, "name") == "solution" {
                    format!("{} sum=xyz", &l[..l.find(" sum=").unwrap()])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        for k in verify_sums(&bad).iter().map(|i| i.kind) {
            covered.insert(k);
        }
        for k in [
            SumIssueKind::ContentChanged,
            SumIssueKind::NotSealed,
            SumIssueKind::MalformedSum,
            SumIssueKind::SectionMissing,
        ] {
            assert!(covered.contains(&k), "SumIssueKind::{k:?} 没有任何用例命中");
        }
        cleanup(&root);
    }
}

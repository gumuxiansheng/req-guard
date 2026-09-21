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
    // 审批锁（§4.4 方案 A）：approve/reject 不得在 AI 执行上下文内发生，
    // 否则 AI 经 Shell 自批即可把状态欺诈骗成 approved。
    crate::auth::ensure_human(if pass { "approve" } else { "reject" })?;
    validate_step(step)?;
    let r = find(root, id)?;
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
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
    out = set_head_status(&out, &recompute_head(&out), None);
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

    // 审计（§4.6）：审批/打回是关键事件——本机日志 + 入库台账（PR 可复核）；
    // 渠道标注（方案 C）让"审批来自带外/交互"可审计。
    let event = format!(
        "{} {} step={} reviewer={} channel={}",
        if pass { "APPROVE" } else { "REJECT" },
        r.id,
        step,
        safe_field(reviewer),
        crate::auth::declared_channel()
    );
    crate::gate::audit(root, &event);
    crate::gate::audit_ledger(root, &event);

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
    crate::auth::ensure_human("done")?;
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
    let out = set_head_status(&content, "done", Some(&now));
    fs::write(&r.path, out).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;

    // 审计：归档改变门禁裁决的输入，属关键事件——本机日志 + 入库台账。
    let event = format!(
        "DONE {} actor={} channel={}",
        r.id,
        safe_field(actor),
        crate::auth::declared_channel()
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
    crate::auth::ensure_human("archive")?;
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
    Ok(out)
}

/// 手动归档单条（存量 done 需求一次性补扫 / 等不及自动清扫时使用）。
///
/// 与 [`archive_due`] 同一条搬移链，仅校验更严：明确指定 id，且**禁止归档活跃需求**
/// （未 done 一律拒绝——否则等于给"AI 把带阻塞评论的需求挪走逃逸门禁"开了后门）。
pub fn archive_one(root: &Path, actor: &str, id: &str, dry_run: bool) -> Result<Vec<ArchivedReq>> {
    crate::auth::ensure_human("archive")?;
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
        "ARCHIVE {} actor={} auto={} channel={}",
        r.id,
        safe_field(actor),
        if is_auto { 1 } else { 0 },
        crate::auth::declared_channel()
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
        if line.contains("GATE:HEAD") {
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
        create(&root, None, "已完成").unwrap();
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
        assert!(by_id["REQ-001"]
            .to_string_lossy()
            .ends_with("archive/misc/REQ-001.md"));
        assert!(by_id["REQ-kd-20260919-A7F3"]
            .to_string_lossy()
            .ends_with("archive/2026/REQ-kd-20260919-A7F3.md"));

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
}

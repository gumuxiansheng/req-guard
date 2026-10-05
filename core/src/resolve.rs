//! 多需求并行的门禁裁决：**裁决对象**是「本次变更集」，不是「某一份清单」。
//!
//! 对应《docs/设计/多需求并行门禁裁决技术方案.md》§4，需求基线
//! `.gates/requirements/REQ-006.md`（二者分歧时以 REQ-006 为准）。
//!
//! ## 为什么要有这个模块
//!
//! 拦截脚本原先断言的是「仓库存在唯一一份当前活跃需求」（`ls | sort -r` 取第一份非
//! `done` 的清单），只对它判三段。结果与「本次改动属于谁」完全无关，于是**同时**产出
//! 两个方向的缺陷（实测见 REQ-006 §1）：
//!
//! - **漏拦**：待审清单 + 已批清单并存时，抽中已批那份 → 放行。AI 在零审批的需求上写代码，
//!   审计台账却记 `PASS <另一份清单>`；
//! - **误锁**：抽中待审那份 → 连已批清单的合法改动也一并拦下。
//!
//! 判据本身是对的，**喂给它的对象是错的** —— 所以调松或调紧判据都修不好。
//!
//! 本模块把对象换掉：拿本次变更集的文件去比各清单的 `GATE:TOUCH` 声明（**表已经存在**，
//! 不必新造），命中谁就问谁；反查不出或有歧义就**报错并给出路**，绝不猜。
//!
//! ## 三条不可让步的设计决策
//!
//! 1. **索引只收三段全批的清单**。起草中的需求人们会先写 `core/src/**` 兜着，若草案声明
//!    进索引，则在已批清单上写代码会因「命中未批清单的 `**`」而被拦 —— **必然误锁**。
//!    推论：未批清单的声明**没有放行权**（但仍可被 `touch --declare` 打回重审时引用）。
//! 2. **显式选择（`--req` / `HOOK_REQ` / 分支名）只收窄、永不覆盖**。若它能覆盖判定，
//!    就成了绕过口（指一份已批清单，为未批清单写代码）。故它必须与本次变更集相交。
//! 3. **命中 >1 份不算歧义**。重构常同时落在两个已批需求范围内，报「无法确定属于哪份」
//!    会制造无解误报。拦截只在**候选里确有未过审项**时发生。
//!
//! ## 分层（可测性前提）
//!
//! ```text
//! judge(live, paths, hint, mode, exempt) -> Verdict   ← 纯函数，不碰 git / 文件系统
//! resolve(root, ctx)                      -> Verdict   ← 薄壳：取参 + 审计 + 渲染
//! ```
//!
//! 判定规则 R1–R15（方案文档 §4.4）全部落在 `judge` 里，才能用 table-driven 单测覆盖，
//! 不必构造 git 索引、不必造临时仓库。
//!
//! **与方案文档的两处签名差异（刻意为之，理由如下）**：
//!
//! - `judge` 多一个 `exempt` 参数：豁免集来自 `.gates/req-guard.yaml`，由薄壳读出后传入，
//!   而不是让 `judge` 自己去碰配置文件 —— 否则「纯函数」名不副实，R5 也测不了。
//! - 应急绕过窗口（R2）**不在** `judge` 里判：它要读 `.gates/.bypass` 与当前时间，
//!   属薄壳的事。`judge` 只负责「谁获准写」。

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::requirement::{self, Requirement, STEPS};

/// 聚合口径（`.gates/req-guard.yaml` 的 `multi.mode`，配置读取随 P3 落地）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiMode {
    /// 按本次变更集反查 + 显式消歧（默认）。
    Resolve,
    /// 保守档：候选恒为全部未归档清单。**不会漏拦，但会把互锁制度化**
    /// （起草中的需求会阻断一切编码）—— 仅在团队明确接受时开启。
    All,
}

/// 变更集来源（三个上下文各一种，**互斥**，由 CLI 显式指定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSource {
    /// 无变更集：裸 `req-guard check` / 人工查状态。
    None,
    /// pre-commit：`HOOK_STAGED_FILES` 或 `git diff --cached`。
    Staged,
    /// CI / L3：`git diff <base>...HEAD`。
    Range(String),
    /// AI PreToolUse：从 stdin payload 取 `file_path`（单路径）。
    Stdin,
}

/// 裁决输入。
#[derive(Debug, Clone)]
pub struct Ctx {
    pub source: PathSource,
    /// 显式选择（消歧用）：`--req <ID>` > `HOOK_REQ` > 分支名。
    pub hint: Option<String>,
}

/// 拦截理由类别。**枚举即契约**（由 `枚举覆盖门槛_每个BlockKind至少被一个用例命中`
/// 强制其被测：新增变体而忘了登记时该用例必红）。
///
/// 派生 `Ord` 是为了枚举覆盖门槛能把 kind 放进 `BTreeMap`（与
/// [`crate::touch::TouchIssueKind`] 同款做法）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlockKind {
    /// 无任何未归档清单。
    NoRequirement,
    /// 显式选择指向的清单不存在 / 已归档 / 不在 live 集内。
    UnknownSelection,
    /// 多份候选，本次改动无法归因于其中任何一份。
    Ambiguous,
    /// 显式选择与本次变更集不相交（选错了需求，或想用它绕过）。
    SelectionMismatch,
    /// 候选清单三段未齐（步骤见 `LiveReq::pending_steps`）。
    StepNotApproved,
    /// 候选清单有未解决的阻塞性评论。
    OpenBlockingComment,
    /// 某份非 `done` 清单的已批准段正文与批准时不一致（REQ-002 内容冻结）。
    SumMismatch,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::NoRequirement => "NoRequirement",
            BlockKind::UnknownSelection => "UnknownSelection",
            BlockKind::Ambiguous => "Ambiguous",
            BlockKind::SelectionMismatch => "SelectionMismatch",
            BlockKind::StepNotApproved => "StepNotApproved",
            BlockKind::OpenBlockingComment => "OpenBlockingComment",
            BlockKind::SumMismatch => "SumMismatch",
        }
    }

    /// 审计日志前缀（与脚本既有 `BLOCK-COMMENT` / `BLOCK-SUM` 同族）。
    fn audit_prefix(self) -> &'static str {
        match self {
            BlockKind::OpenBlockingComment => "BLOCK-COMMENT",
            BlockKind::Ambiguous => "BLOCK-AMBIGUOUS",
            BlockKind::SelectionMismatch => "BLOCK-SELECTION",
            BlockKind::SumMismatch => "BLOCK-SUM",
            BlockKind::UnknownSelection => "BLOCK-UNKNOWN-SELECTION",
            BlockKind::NoRequirement => "BLOCK no-requirement",
            BlockKind::StepNotApproved => "BLOCK",
        }
    }
}

/// 一份未归档清单的裁决快照（判定所需的全部事实，一次读完）。
#[derive(Debug, Clone)]
pub struct LiveReq {
    pub id: String,
    pub path: PathBuf,
    /// 三段 `GATE:STEP` 的 status 是否全为 `approved`。
    ///
    /// **只读 status，不重复实现段落实质性判定**：空段在 `approve` 时就已被
    /// [`crate::section::is_section_empty`] 拒掉，故 `approved` 已隐含三段非空。
    /// 在这里再判一次段空就是第二处真相，会与 REQ-003 的判据漂移。
    pub approved: bool,
    /// 未过审的步骤（文案与审计指名用；已全批时为空）。
    pub pending_steps: Vec<&'static str>,
    /// 是否有未解决的阻塞性评论。
    pub has_open_blocking: bool,
    /// `GATE:TOUCH` 声明。缺块 / 未闭合 / `..` 逃逸 / 段落定位失败一律归一为空 ——
    /// 空声明**没有放行权**（会落进 R7/R8 报错），但不会 panic。
    pub declares: Vec<String>,
    /// 内容冻结**硬伤**（REQ-002 的 `sum=`，Error 级）。
    pub sum_errors: Vec<String>,
    /// 内容冻结**告警**（如「已批准但未 seal」）。**一条都不许静默丢弃** ——
    /// 静默跳过等于「看起来有冻结、实际没有」。
    pub sum_warnings: Vec<String>,
}

impl LiveReq {
    /// 人读的状态标签（文案与审计共用一份，避免两处措辞漂移）。
    pub fn status_label(&self) -> String {
        if self.approved {
            "已批准".to_string()
        } else {
            format!("待审核（未过审: {}）", self.pending_steps.join(", "))
        }
    }
}

/// 裁决结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Pass {
        /// 被裁决的清单。审计必须列出它们 —— 只记「PASS」会让读者以为抽中的那个
        /// 就是被开发的那个，而这正是漏拦被长期掩盖的原因。
        reqs: Vec<String>,
        /// 放行附带说明（内容冻结告警等）。门禁唯一的静默降级口只有 Warn，且必须照打。
        note: Option<String>,
    },
    Bypassed {
        reqs: Vec<String>,
        expires_epoch: u64,
    },
    Block {
        kind: BlockKind,
        /// 被点名的清单（拦截时**只**列真正有问题的那些）。
        reqs: Vec<String>,
        message: String,
    },
}

impl Verdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass { .. } | Verdict::Bypassed { .. })
    }

    pub fn is_bypassed(&self) -> bool {
        matches!(self, Verdict::Bypassed { .. })
    }

    pub fn block_kind(&self) -> Option<BlockKind> {
        match self {
            Verdict::Block { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// 被裁决 / 被点名的清单 ID。
    pub fn reqs(&self) -> &[String] {
        match self {
            Verdict::Pass { reqs, .. } | Verdict::Bypassed { reqs, .. } => reqs,
            Verdict::Block { reqs, .. } => reqs,
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Verdict::Block { message, .. } => Some(message.as_str()),
            _ => None,
        }
    }

    /// 放行附带的说明（`Pass.note`）。
    pub fn note(&self) -> Option<&str> {
        match self {
            Verdict::Pass { note, .. } => note.as_deref(),
            _ => None,
        }
    }
}

// ───────────────────────────── 判定（纯函数） ─────────────────────────────

/// 裁决核心（**纯函数**：不碰 git、不碰文件系统、不读配置）。
///
/// `exempt` 是豁免 glob（来自 `touch.exempt`），在反查前剥除 —— 豁免集**单源**，
/// 不在此处另写一份默认集。
pub fn judge(
    live: &[LiveReq],
    paths: &[String],
    hint: Option<&str>,
    mode: MultiMode,
    exempt: &[String],
) -> Verdict {
    let ids = |rs: &[&LiveReq]| -> Vec<String> { rs.iter().map(|r| r.id.clone()).collect() };
    let exempt_hit = |p: &str| -> bool {
        let p = crate::touch::normalize_path(p);
        exempt.iter().any(|g| crate::touch::glob_match(g, &p))
    };
    // 受管路径 = 剥掉豁免后的路径。空集**不**降级为「全量」（G4：单需求仓库行为不变）。
    let managed: Vec<&str> = paths
        .iter()
        .map(|p| p.as_str())
        .filter(|p| !p.trim().is_empty() && !exempt_hit(p))
        .collect();

    // ── R1 无未归档清单 ────────────────────────────────────────────────
    if live.is_empty() {
        return Verdict::Block {
            kind: BlockKind::NoRequirement,
            reqs: vec![],
            message: "[req-guard] ⛔ 拦截：未找到待开发的需求清单。\n\
                      AI 在编写代码前，必须先创建并走完三段清单审核：\n\
                      \x20             req-guard create -t \"<需求标题>\""
                .to_string(),
        };
    }

    // ── R15 内容冻结：遍历**全部**非 done 清单，与候选集正交（G7）────────
    // 判定先于三段：「证据已被篡改」比「没审批」更严重。
    let sum_bad: Vec<&LiveReq> = live.iter().filter(|r| !r.sum_errors.is_empty()).collect();
    if !sum_bad.is_empty() {
        let mut msg = String::from(
            "[req-guard] ⛔ 拦截：已批准段的正文与批准时不一致（内容冻结校验失败）。\n",
        );
        for r in &sum_bad {
            for e in &r.sum_errors {
                msg.push_str(&format!("  - {} {}\n", r.id, first_line(e)));
            }
        }
        msg.push_str("          被冻结的段是审批证据，改动它等于事后改写审批记录。\n");
        msg.push_str("          正解：req-guard amend <需求ID> --step <步骤> --author <姓名>\n");
        msg.push_str(
            "               （先打回待审 → 改完再 approve；确需本次放行：git commit --no-verify）",
        );
        return Verdict::Block {
            kind: BlockKind::SumMismatch,
            reqs: ids(&sum_bad),
            message: msg,
        };
    }

    // ── R3 显式选择指向不存在 / 已归档的清单 ────────────────────────────
    let hint_req: Option<&LiveReq> = hint.and_then(|id| live.iter().find(|r| r.id == id));
    if hint.is_some() && hint_req.is_none() {
        return Verdict::Block {
            kind: BlockKind::UnknownSelection,
            reqs: vec![],
            message: unknown_selection_message(hint.unwrap_or_default(), live),
        };
    }

    // ── R4 / R6 / R9 / R10 / R7 / R8 / R13：候选推导 ────────────────────
    // 候选推导只有一份实现（`owned_by`），门禁裁决与 `touch --scope strict` 共用 ——
    // 两处各写一套就是第二个真相，且它们在"多份清单都声明了该文件"上必然漂移。
    let candidates: Vec<&LiveReq> = if mode == MultiMode::All {
        // R13 保守档：候选恒为 live 全体（「都在开工前批完」的团队纪律）。
        live.iter().collect()
    } else {
        let owned = owned_by(live, paths, hint, exempt);
        if owned.is_empty() {
            // R3 已在上一步拦掉「hint 指向不存在的清单」，故这里只可能是：
            // R8 无从归因，或 R9 hint 与命中集不相交（hint 是绕过口，必须拦）。
            return match hint_req
                .filter(|hr| !hit_list(live, paths, exempt).iter().any(|r| r.id == hr.id))
            {
                Some(hr) => Verdict::Block {
                    kind: BlockKind::SelectionMismatch,
                    reqs: vec![hr.id.clone()],
                    message: selection_mismatch_message(
                        hr,
                        &hit_list(live, paths, exempt),
                        &managed,
                    ),
                },
                None => Verdict::Block {
                    kind: BlockKind::Ambiguous,
                    reqs: vec![],
                    message: ambiguous_message(live, &managed),
                },
            };
        }
        owned
    };

    // ── R11 逐份判定：只点名真正有问题的那些 ────────────────────────────
    let unapproved: Vec<&LiveReq> = candidates.iter().filter(|r| !r.approved).copied().collect();
    if !unapproved.is_empty() {
        return Verdict::Block {
            kind: BlockKind::StepNotApproved,
            reqs: ids(&unapproved),
            message: step_message(&unapproved),
        };
    }
    let blocked: Vec<&LiveReq> = candidates
        .iter()
        .filter(|r| r.has_open_blocking)
        .copied()
        .collect();
    if !blocked.is_empty() {
        return Verdict::Block {
            kind: BlockKind::OpenBlockingComment,
            reqs: ids(&blocked),
            message: blocking_message(&blocked),
        };
    }

    // ── R12 全部过审 → 放行 ────────────────────────────────────────────
    let mut notes: Vec<String> = Vec::new();
    for r in &candidates {
        for w in &r.sum_warnings {
            notes.push(format!("{} {}", r.id, first_line(w)));
        }
    }
    Verdict::Pass {
        reqs: ids(&candidates),
        note: if notes.is_empty() {
            None
        } else {
            Some(notes.join("\n"))
        },
    }
}

// ───────────────────────────── 取参与快照（薄壳侧） ─────────────────────────────

/// 读出全部**未归档、未 `done`** 的清单快照。
///
/// `done` 清单一律不进 live：归档是生命周期终点，否则一份几个月前的归档清单会一直
/// 参与裁决。`archive/` 子目录由 [`requirement::list`] 只扫顶层天然排除。
pub fn live_snapshot(root: &Path) -> Result<Vec<LiveReq>> {
    let mut out = Vec::new();
    for r in requirement::list(root)? {
        let content = std::fs::read_to_string(&r.path).unwrap_or_default();
        if requirement::head_status(&content) == "done" {
            continue;
        }
        out.push(snapshot_of(root, &r, &content));
    }
    Ok(out)
}

fn snapshot_of(root: &Path, r: &Requirement, content: &str) -> LiveReq {
    let mut pending_steps = Vec::new();
    for (key, _) in STEPS.iter() {
        if requirement::step_status(content, key) != "approved" {
            pending_steps.push(*key);
        }
    }
    let (sum_errors, sum_warnings) = split_sums(content);
    LiveReq {
        id: r.id.clone(),
        path: r.path.clone(),
        approved: pending_steps.is_empty(),
        pending_steps,
        // 评论读不到（文件缺失 / 格式坏）**不得**当成「没有阻塞评论」——
        // 那正是「看着在拦、其实没拦」。读失败一律按「有阻塞」处理。
        has_open_blocking: match crate::comment::list(root, &r.id) {
            Ok(cs) => cs.iter().any(|c| c.is_blocking_open()),
            Err(_) => true,
        },
        declares: declares_of(content),
        sum_errors,
        sum_warnings,
    }
}

/// 内容冻结问题按严重级拆两半：Error 拦，Warn 只报。
fn split_sums(content: &str) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for i in requirement::verify_sums(content) {
        if i.severity.is_error() {
            errors.push(i.message);
        } else {
            warnings.push(i.message);
        }
    }
    (errors, warnings)
}

/// 抽第 2 段的 `GATE:TOUCH` 声明。
///
/// 段落定位失败（[`requirement::section_span`] 返回 `None`）时**按空声明**处理 ——
/// 绝不能回退整篇去取块（那会命中别处的 `GATE:TOUCH`，等于凭空给一份声明）。
/// 空声明没有放行权，会把该清单推进 R7/R8 报错分支（fail-closed 且可诊断）。
fn declares_of(content: &str) -> Vec<String> {
    let Some((start, end)) = requirement::section_span(content, 1) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let section = lines[start - 1..end].join("\n");
    let (paths, _bad, _) = crate::touch::declared(&section, start);
    // `bad`（`..` 逃逸 / 绝对路径）本就无法归一，`declared` 已把它们排除在 `paths` 外；
    // 这里不额外处理 —— 它们没有放行权，正合需要。
    paths
}

/// 判定 + 审计（**P1 的接入点**：CLI 与脚本都走这里，变更集由调用方取好）。
///
/// 与 [`resolve`] 的区别只在于变更集从哪来：本函数**不碰 git / stdin**，故可被
/// 单测用真实文件 + 显式路径直接驱动（`resolve` 依赖 `HOOK_STAGED_FILES` 这类
/// 进程级环境变量，并行测试下有竞态，auth.rs 已记录过这个坑）。
pub fn decide(
    root: &Path,
    live: &[LiveReq],
    paths: &[String],
    hint: Option<&str>,
    mode: MultiMode,
) -> Verdict {
    let exempt = crate::gate::touch_exempt_patterns(root);
    let v = judge(live, paths, hint, mode, &exempt);
    audit_verdict(root, live, &v);
    v
}

/// 反查命中集 `H`（R6）：已批清单里声明了本次任一路径的那些。
///
/// 抽出是因为 R9（hint 与命中集不相交）需要它来区分「不相交」与「无从归因」——
/// 两种情况都表现为候选集为空，但拦的理由与出路完全不同。
pub fn hit_list<'a>(live: &'a [LiveReq], paths: &[String], exempt: &[String]) -> Vec<&'a LiveReq> {
    let managed: Vec<&str> = paths
        .iter()
        .map(|p| p.as_str())
        .filter(|p| !p.trim().is_empty() && !exempt.iter().any(|g| crate::touch::glob_match(g, p)))
        .collect();
    live.iter()
        .filter(|r| {
            r.approved
                && r.declares
                    .iter()
                    .any(|d| managed.iter().any(|p| crate::touch::glob_match(d, p)))
        })
        .collect()
}

/// 本次变更集**归属**于哪些清单（**只算归属，不问"能不能写"**）。
///
/// 与 [`judge`] 共用同一套候选推导，故 `touch --scope strict` 与门禁裁决
/// 对"这份改动属于谁"永远同一答案 —— 两处各写一套就是第二个真相，而它们在
/// "多份清单都声明了该文件"这类边界上必然漂移。
///
/// 三段 / 阻塞评论 / 内容冻结一概不问：那是 [`judge`] 的事，本函数只回答归属。
pub fn owned_by<'a>(
    live: &'a [LiveReq],
    paths: &[String],
    hint: Option<&str>,
    exempt: &[String],
) -> Vec<&'a LiveReq> {
    let managed: Vec<&str> = paths
        .iter()
        .map(|p| p.as_str())
        .filter(|p| !p.trim().is_empty() && !exempt.iter().any(|g| crate::touch::glob_match(g, p)))
        .collect();
    let hits = |r: &LiveReq| -> bool {
        r.declares
            .iter()
            .any(|d| managed.iter().any(|p| crate::touch::glob_match(d, p)))
    };
    let h: Vec<&LiveReq> = live.iter().filter(|r| r.approved && hits(r)).collect();
    match hint {
        Some(id) => {
            let Some(hr) = live.iter().find(|r| r.id == id) else {
                return Vec::new();
            };
            if !h.is_empty() && !h.iter().any(|r| r.id == hr.id) {
                return Vec::new(); // 不相交 → 无归属（由调用方报歧义，不猜）
            }
            let mut v = h;
            if !v.iter().any(|r| r.id == hr.id) {
                v.push(hr);
            }
            v
        }
        None => {
            if h.is_empty() && live.len() == 1 {
                live.iter().collect()
            } else {
                h
            }
        }
    }
}

/// 端到端裁决：读 live 快照 + 取变更集 + 判 + 写审计。
///
/// **绕过窗口在这里判**（`.gates/.bypass` 与当前时间是薄壳的事，`judge` 不碰）。
pub fn resolve(root: &Path, ctx: &Ctx) -> Result<Verdict> {
    let live = live_snapshot(root)?;
    let verdict = if live.is_empty() {
        // R1 先于绕过：没有清单可批准时，绕过窗口不该凭空造出一个「通过」。
        judge(&live, &[], ctx.hint.as_deref(), MultiMode::Resolve, &[])
    } else if let Some(exp) = active_bypass(root) {
        Verdict::Bypassed {
            reqs: live.iter().map(|r| r.id.clone()).collect(),
            expires_epoch: exp,
        }
    } else {
        let paths = collect_paths(root, &ctx.source)?;
        let exempt = crate::gate::touch_exempt_patterns(root);
        let mode = multi_mode(root);
        let mut verdict = judge(&live, &paths, ctx.hint.as_deref(), mode, &exempt);
        // R14 分支名消歧：**只**在歧义（= 无从归因）时兜底，且永不覆盖反查结果。
        // 放在 judge 之外而不是给它加参数：分支名不是权威选择，拿它去过 R3/R9 那些
        // 「选择必须与变更集相交」的校验毫无意义 —— 它恰恰是变更集推不出答案才被问的。
        let known: Vec<String> = live.iter().map(|r| r.id.clone()).collect();
        let branch_says = (verdict.block_kind() == Some(BlockKind::Ambiguous)
            && ctx.hint.is_none())
        .then(|| {
            crate::gate::multi_bind_branch(root)
                .then(|| branch_req_hint(root, &known))
                .flatten()
        })
        .flatten();
        if let Some((id, multi)) = branch_says {
            if let Some(target) = live.iter().find(|r| r.id == id) {
                verdict = judge(&live, &paths, Some(target.id.as_str()), mode, &exempt);
                if verdict.is_pass() {
                    verdict = append_note(verdict, &branch_note(target, multi));
                }
            }
        }
        // `UnknownSelection` 的「已归档 / 拼错」补充信息要读磁盘，故只能在这层加。
        match (&verdict, ctx.hint.as_deref()) {
            (
                Verdict::Block {
                    kind: BlockKind::UnknownSelection,
                    message,
                    ..
                },
                Some(id),
            ) => Verdict::Block {
                kind: BlockKind::UnknownSelection,
                reqs: vec![],
                message: enrich_archived(root, id, live.len(), message),
            },
            _ => verdict,
        }
    };
    audit_verdict(root, &live, &verdict);
    Ok(verdict)
}

/// `UnknownSelection` 的补充信息：该 id 是否已归档（`judge` 是纯函数，看不到磁盘）。
fn enrich_archived(root: &Path, id: &str, live_count: usize, base: &str) -> String {
    let archived = requirement::list_archived(root)
        .map(|v| v.iter().any(|r| r.id == id))
        .unwrap_or(false);
    let mut extra = String::new();
    if archived {
        extra.push_str("  该清单已归档：归档是生命周期终点，不可再作为裁决对象。\n");
    } else if live_count == 0 {
        extra.push_str("  当前仓库没有任何未归档清单。\n");
    } else {
        extra.push_str("  该 id 不在 .gates/requirements/ 下（拼写错误，或清单已被删除）。\n");
    }
    let mut lines = base.lines();
    let mut out = lines.next().unwrap_or("").to_string();
    out.push('\n');
    for l in lines {
        if l.contains("  处置：") {
            out.push_str(&extra);
        }
        out.push_str(l);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// 读 `multi.mode`（缺省 `resolve`）。
pub fn multi_mode(root: &Path) -> MultiMode {
    if crate::gate::multi_mode_all(root) {
        MultiMode::All
    } else {
        MultiMode::Resolve
    }
}

/// 从**当前分支名**里取 `REQ-<id>`（R14）。返回 `(id, 同名出现次数)`。
///
/// 只认第一个：分支名里出现多个 `REQ-` 时不猜（一次改动确实可以跨多份需求，
/// 那种情况下应由 `--req` 或补声明来表达，不该由工具替人选一个）。
fn branch_req_hint(root: &Path, known: &[String]) -> Option<(String, usize)> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        return None;
    }
    let mut ids = match_branch_req(&branch, known);
    if ids.is_empty() {
        return None;
    }
    let n = ids.len();
    Some((ids.remove(0), n))
}

/// 从分支名里认出**真实存在**的需求 id（**纯函数**）。
///
/// 为什么不按字符切 token：分支名的 id 段常带后缀（`feature/REQ-001-x`、
/// `REQ-001/fix-thing`），而 `-` 本身是合法 id 字符（`normalize_id` 允许）。
/// 按字符切会切出 `REQ-001-x` 这种永远匹配不上任何清单的串。故改为
/// **拿已知 id 集合去前缀匹配** —— 认不出来的片段直接丢掉（宁可不消歧，
/// 也不消歧到一份错的清单上，那比不消歧更危险）。
///
/// 多个 id 时按在分支名里出现的先后返回（调用方取第一个并告警）。
pub fn match_branch_req(branch: &str, known: &[String]) -> Vec<String> {
    let b = branch.replace('\\', "/");
    let mut out: Vec<String> = Vec::new();
    for id in known {
        if b.contains(id.as_str()) && !out.contains(id) {
            // 记下首次出现位置，供同名片段排序用
            out.push(id.clone());
        }
    }
    out.sort_by_key(|id| b.find(id.as_str()).unwrap_or(usize::MAX));
    out
}

/// 从分支名里扫出全部 `REQ-<id>` 片段（**纯函数**，便于单测）。
///
/// 片段以非字母数字/连字符/下划线为界，故 `feature/REQ-001-x` 与
/// `REQ-001` 都认得，而 `REQ-0001`（自动编号污染形态）也认得 —— 这里不做编号格式
/// 校验，那是 `ids --check` 的职责；本函数只负责"分支名里有没有可指向的 id"。
pub fn branch_req_ids(branch: &str) -> Vec<String> {
    let b = branch.replace('\\', "/");
    let bytes: Vec<char> = b.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 'R'
            && bytes.get(i + 1) == Some(&'E')
            && bytes.get(i + 2) == Some(&'Q')
            && bytes.get(i + 3) == Some(&'-')
        {
            let mut j = i + 4;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '-' || bytes[j] == '_')
            {
                j += 1;
            }
            let id: String = bytes[i..j].iter().collect();
            out.push(id);
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn branch_note(target: &LiveReq, multi: usize) -> String {
    let mut n = format!("（本次由分支名消歧到 {}）", target.id);
    if multi > 1 {
        n.push_str(&format!(
            "；分支名里出现 {multi} 个 REQ- 片段，已取第一个 —— 若不是它，请显式指定：--req <ID>"
        ));
    }
    n
}

/// 给放行裁决追加一条说明（不改判据，只补可追溯信息）。
fn append_note(v: Verdict, note: &str) -> Verdict {
    match v {
        Verdict::Pass { reqs, note: old } => Verdict::Pass {
            reqs,
            note: Some(match old {
                Some(o) => format!("{o}\n{note}"),
                None => note.to_string(),
            }),
        },
        other => other,
    }
}

/// 取变更集（三个上下文各一种；`None` → 空集）。
fn collect_paths(root: &Path, source: &PathSource) -> Result<Vec<String>> {
    Ok(match source {
        PathSource::None => Vec::new(),
        PathSource::Staged => crate::touch::staged_files(root)?,
        PathSource::Range(base) => crate::touch::diff_files(root, base)?,
        PathSource::Stdin => {
            use std::io::Read as _;
            let mut payload = String::new();
            let _ = std::io::stdin().read_to_string(&mut payload);
            // 取不到路径不是一次可识别的文件写操作：交给后续门禁，不在这里硬拦
            // （与 `pretool_verdict` 的 `Continue` 同款理由）。
            match crate::json::file_path_of(&payload) {
                Some(p) if !p.trim().is_empty() => vec![p],
                _ => Vec::new(),
            }
        }
    })
}

/// 当前是否命中应急绕过窗口；返回过期时刻。
fn active_bypass(root: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join(".gates/.bypass")).ok()?;
    let exp = key_value(&text, "expires_epoch")?;
    if crate::gate::now_epoch() < exp {
        Some(exp)
    } else {
        None
    }
}

/// 从 `key=value` 文本里取一个 u64 字段（文件格式与 `gate::bypass` 落盘一致）。
fn key_value(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(v) = line.trim().strip_prefix(&format!("{key}=")) {
            let v = v.trim();
            if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
                return v.parse().ok();
            }
        }
    }
    None
}

fn audit_verdict(root: &Path, live: &[LiveReq], v: &Verdict) {
    let find = |id: &str| live.iter().find(|r| r.id == id);
    match v {
        Verdict::Pass { reqs, note } => {
            if reqs.is_empty() {
                crate::gate::audit(root, "PASS no-managed-path");
            } else {
                crate::gate::audit(root, &format!("PASS {}", reqs.join(" ")));
            }
            if let Some(n) = note {
                for l in n.lines() {
                    crate::gate::audit(root, &format!("NOTE {}", l.trim()));
                }
            }
        }
        Verdict::Bypassed {
            reqs,
            expires_epoch,
        } => {
            crate::gate::audit(
                root,
                &format!(
                    "BYPASS-HIT expires_epoch={expires_epoch} {}",
                    reqs.join(" ")
                ),
            );
        }
        Verdict::Block { kind, reqs, .. } => {
            let prefix = kind.audit_prefix();
            let detail = match kind {
                BlockKind::Ambiguous => {
                    let all: Vec<String> = live
                        .iter()
                        .map(|r| {
                            format!(
                                "{}({})",
                                r.id,
                                if r.approved { "approved" } else { "pending" }
                            )
                        })
                        .collect();
                    format!(" candidates={}", all.join(","))
                }
                BlockKind::StepNotApproved => {
                    let steps: Vec<String> = reqs
                        .iter()
                        .filter_map(|id| find(id))
                        .map(|r| r.pending_steps.join("+"))
                        .collect();
                    if steps.is_empty() {
                        String::new()
                    } else {
                        format!(" steps={}", steps.join(","))
                    }
                }
                _ => String::new(),
            };
            if reqs.is_empty() && detail.is_empty() {
                crate::gate::audit(root, prefix);
            } else {
                let ids = if reqs.is_empty() {
                    String::new()
                } else {
                    format!(" {}", reqs.join(" "))
                };
                crate::gate::audit(root, &format!("{prefix}{ids}{detail}"));
            }
        }
    }
}

// ───────────────────────────── 文案（三张牌） ─────────────────────────────

fn status_label(r: &LiveReq) -> String {
    if r.approved {
        "已批准".to_string()
    } else {
        format!("待审核（未过审: {}）", r.pending_steps.join(", "))
    }
}

fn unknown_selection_message(id: &str, live: &[LiveReq]) -> String {
    let mut msg = format!("[req-guard] ⛔ 拦截：指定的 {id} 不在可裁决的需求清单里。\n");
    msg.push_str(&format!(
        "  当前未归档清单：{}\n",
        if live.is_empty() {
            "（无）".to_string()
        } else {
            live.iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    msg.push_str("  处置：\n");
    msg.push_str(&format!(
        "    1) 确认 --req / HOOK_REQ 拼写（id 形如 REQ-001）\n    2) 该清单已完成 → req-guard done {id} --author <姓名>\n",
    ));
    msg.push_str(
        "    3) 本次改动确实属于别的清单 → 改用它：HOOK_REQ=<ID> 或 req-guard check --req <ID>",
    );
    msg
}

fn selection_mismatch_message(hr: &LiveReq, h: &[&LiveReq], managed: &[&str]) -> String {
    let mut msg = format!(
        "[req-guard] ⛔ 拦截：指定的需求 {} 与本次改动不相交。\n",
        hr.id
    );
    msg.push_str(&format!("  本次改动：{}\n", joined(managed)));
    msg.push_str(&format!(
        "  {} 的声明未命中上述路径（状态：{}）\n",
        hr.id,
        status_label(hr)
    ));
    msg.push_str(&format!(
        "  实际命中的已批清单：{}\n",
        h.iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    msg.push_str("  处置二选一：\n");
    msg.push_str(&format!(
        "    1) 本次改动确实属于 {} → 先补声明：\n       req-guard touch --declare {} --glob \"<路径>\" --reason \"...\"\n",
        hr.id, hr.id
    ));
    msg.push_str(
        "    2) 本次改动属于上面那份 → 改用它：HOOK_REQ=<ID> 或 req-guard check --req <ID>",
    );
    msg
}

fn ambiguous_message(live: &[LiveReq], managed: &[&str]) -> String {
    let mut msg = String::from(
        "[req-guard] ⛔ 拦截：无法确定本次改动属于哪份需求（仓库内有多份未归档清单，且没有任何已批清单声明这些路径）。\n",
    );
    msg.push_str(&format!("  本次改动：{}\n", joined(managed)));
    msg.push_str("  候选清单：\n");
    for r in live {
        msg.push_str(&format!(
            "    - {}（{}）  声明：{}\n",
            r.id,
            status_label(r),
            if r.declares.is_empty() {
                "（无有效声明）".to_string()
            } else {
                r.declares.join(", ")
            }
        ));
    }
    msg.push_str("  处置三选一：\n");
    msg.push_str("    1) 补声明：本次改动属于某份清单 → 在该清单「## 2. 技术方案」的 GATE:TOUCH 块补上它，\n");
    msg.push_str("       或 req-guard touch --declare <ID> --glob \"<路径>\" --reason \"...\"（会打回技术方案重审）\n");
    msg.push_str(
        "    2) 显式指定：本次改动只属于某一份 → HOOK_REQ=<ID> 或 req-guard check --req <ID>\n",
    );
    msg.push_str("    3) 归档无关清单：起草中且短期不并行 → req-guard done <ID> --author <姓名>\n");
    msg.push_str(
        "  确需本次放行：git commit --no-verify（服务端 req-guard check --base 仍会挡住）",
    );
    msg
}

fn step_message(bad: &[&LiveReq]) -> String {
    let mut msg = String::from(
        "[req-guard] ⛔ 拦截：本次改动所属的需求尚未通过审核，AI 不得编写/修改源码。\n",
    );
    for r in bad {
        msg.push_str(&format!(
            "    - {} 未过审步骤：{}\n",
            r.id,
            r.pending_steps.join(", ")
        ));
    }
    msg.push_str("          请补齐清单后由审核人执行：\n");
    msg.push_str("            req-guard approve <需求ID> --step <步骤> --reviewer <姓名>\n");
    msg.push_str(
        "          步骤顺序：decomposition(需求分解) → solution(技术方案) → testplan(测试计划)",
    );
    msg
}

fn blocking_message(bad: &[&LiveReq]) -> String {
    let mut msg = String::from("[req-guard] ⛔ 拦截：本次改动所属的需求存在未解决的阻塞性评论。\n");
    for r in bad {
        msg.push_str(&format!("    - {}\n", r.id));
    }
    msg.push_str("          查看与处理：\n");
    msg.push_str("            req-guard comments <需求ID>\n");
    msg.push_str("            req-guard resolve <需求ID> --comment C001 --reviewer <姓名>");
    msg
}

fn joined(items: &[&str]) -> String {
    if items.is_empty() {
        "（无受管路径）".to_string()
    } else {
        items.join(", ")
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

// ───────────────────────────── 单测 ─────────────────────────────

#[cfg(test)]
#[allow(non_snake_case)] // 与 ac.rs / touch.rs / section.rs 的中文测试命名一致：规则号 + 中文描述便于对读
mod tests {
    use super::*;

    const EXEMPT: [&str; 3] = [".gates/**", "target/**", "Cargo.lock"];

    fn req(id: &str, approved: bool, declares: &[&str]) -> LiveReq {
        LiveReq {
            id: id.to_string(),
            path: PathBuf::from(format!(".gates/requirements/{id}.md")),
            approved,
            pending_steps: if approved {
                Vec::new()
            } else {
                vec!["decomposition", "solution", "testplan"]
            },
            has_open_blocking: false,
            declares: declares.iter().map(|s| s.to_string()).collect(),
            sum_errors: Vec::new(),
            sum_warnings: Vec::new(),
        }
    }

    fn p(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn ex() -> Vec<String> {
        EXEMPT.iter().map(|s| s.to_string()).collect()
    }

    fn judge_ok(live: &[LiveReq], paths: &[&str], hint: Option<&str>) -> Verdict {
        judge(live, &p(paths), hint, MultiMode::Resolve, &ex())
    }

    fn ids(v: &Verdict) -> Vec<String> {
        v.reqs().to_vec()
    }

    // ── R1 ──────────────────────────────────────────────────────────────
    #[test]
    fn R1_无清单报NoRequirement() {
        let v = judge(&[], &p(&["core/src/a.rs"]), None, MultiMode::Resolve, &ex());
        assert_eq!(v.block_kind(), Some(BlockKind::NoRequirement));
        assert!(!v.is_pass());
    }

    #[test]
    fn R1_msg_给出创建命令() {
        let v = judge(&[], &[], None, MultiMode::Resolve, &ex());
        assert!(v.message().unwrap_or_default().contains("create -t"));
    }

    // ── R15 / G7 ────────────────────────────────────────────────────────
    #[test]
    fn R15_内容冻结遍历全部非done清单_与候选集无关() {
        let mut b = req("REQ-002", true, &["cli/**"]);
        b.sum_errors = vec!["「技术方案」的正文与批准时不一致".to_string()];
        // 本次改动命中 A，与 B 无关 —— 但 B 的冻结违规仍必须拦（G7）
        let v = judge_ok(
            &[req("REQ-001", true, &["core/**"]), b.clone()],
            &["core/a.rs"],
            None,
        );
        assert_eq!(v.block_kind(), Some(BlockKind::SumMismatch));
        assert_eq!(ids(&v), vec!["REQ-002".to_string()]);
        let v = judge_ok(
            &[req("REQ-001", true, &["core/**"]), b],
            &["cli/main.rs"],
            None,
        );
        assert_eq!(v.block_kind(), Some(BlockKind::SumMismatch));
    }

    #[test]
    fn R15_msg_指名清单与修复命令() {
        let mut a = req("REQ-001", true, &["core/**"]);
        a.sum_errors = vec!["「技术方案」的正文与批准时不一致".to_string()];
        let v = judge_ok(&[a], &["core/a.rs"], None);
        let m = v.message().unwrap_or_default();
        assert!(m.contains("REQ-001") && m.contains("amend"), "{m}");
    }

    #[test]
    fn R15_冻结告警不拦但进note() {
        let mut a = req("REQ-001", true, &["core/**"]);
        a.sum_warnings = vec!["「需求分解」已批准但未启用内容冻结（sum=-）".to_string()];
        let v = judge_ok(&[a], &["core/a.rs"], None);
        assert!(v.is_pass());
        let note = v.note().unwrap_or_default();
        assert!(note.contains("未启用内容冻结"), "告警必须照打：{note}");
    }

    // ── R3 ──────────────────────────────────────────────────────────────
    #[test]
    fn R3_指定不存在的清单报UnknownSelection() {
        let live = vec![req("REQ-001", true, &["core/**"])];
        let v = judge_ok(&live, &["core/a.rs"], Some("REQ-999"));
        assert_eq!(v.block_kind(), Some(BlockKind::UnknownSelection));
    }

    #[test]
    fn R3_msg_点名指定ID并列出现存清单() {
        let live = vec![req("REQ-001", true, &["core/**"])];
        let v = judge_ok(&live, &["core/a.rs"], Some("REQ-999"));
        let m = v.message().unwrap_or_default();
        assert!(m.contains("REQ-999"), "{m}");
        assert!(m.contains("REQ-001"), "须列出当前可选清单：{m}");
    }

    // ── R4 / 4.3.1 两个方向 ─────────────────────────────────────────────
    #[test]
    fn R4_未批清单不建索引_已批范围内不误锁() {
        let live = vec![
            req("REQ-001", true, &["core/src/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &["core/src/a.rs"], None);
        assert!(v.is_pass(), "草案 `**` 不得造成误锁：{v:?}");
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    #[test]
    fn R4对偶_未批清单声明范围内的文件必须拦() {
        // R4 的**镜像**：只测上一条时，重构能悄悄打破 G1 而测试全绿。
        let live = vec![
            req("REQ-001", false, &["core/src/**"]),
            req("REQ-002", true, &["cli/**"]),
        ];
        let v = judge_ok(&live, &["core/src/a.rs"], None);
        assert!(!v.is_pass(), "落在未批清单范围内的文件不得放行：{v:?}");
        assert_eq!(v.block_kind(), Some(BlockKind::Ambiguous));
    }

    #[test]
    fn R4对偶_msg_标出已批的那份() {
        let live = vec![
            req("REQ-001", false, &["core/src/**"]),
            req("REQ-002", true, &["cli/**"]),
        ];
        let v = judge_ok(&live, &["core/src/a.rs"], None);
        let m = v.message().unwrap_or_default();
        assert!(m.contains("REQ-002（已批准）"), "{m}");
        assert!(m.contains("REQ-001（待审核"), "{m}");
    }

    // ── R6 / 4.3.2 ──────────────────────────────────────────────────────
    #[test]
    fn R6_命中多份已批清单不报歧义() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["core/**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], None);
        assert!(v.is_pass(), "多份都已批不该报歧义：{v:?}");
        assert_eq!(ids(&v), vec!["REQ-001".to_string(), "REQ-002".to_string()]);
    }

    #[test]
    fn R6_未批清单即便声明同一路径也不进候选() {
        // 未批清单的声明**没有放行权**（§4.3.1）：它既不能授权，也不能因为
        // 「自己也声明了 core/**」就把已批的那两份拖下水。
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["core/**"]),
            req("REQ-003", false, &["core/**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], None);
        assert!(v.is_pass(), "两份已批清单已覆盖该路径：{v:?}");
        assert_eq!(
            ids(&v),
            vec!["REQ-001".to_string(), "REQ-002".to_string()],
            "未批的 REQ-003 不得进候选"
        );
    }

    #[test]
    fn R6_两份已批加一份未批_未批那份被指名() {
        // 与上一条互补：未批清单**在索引里缺席**，故它只能通过 R7/R8 被指名，
        // 不会因为路径相同而出现在拦截名单里。
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], None);
        assert!(v.is_pass());
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    #[test]
    fn R6_非done清单里混入未批清单不影响已批那份() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", false, &["cli/**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], None);
        assert!(v.is_pass(), "G2：无关未批清单不得阻断已批清单：{v:?}");
    }

    // ── R7 / G4 ─────────────────────────────────────────────────────────
    #[test]
    fn R7_单需求未批且路径无声明时拦() {
        let live = vec![req("REQ-001", false, &["core/**"])];
        let v = judge_ok(&live, &["docs/a.md"], None);
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    #[test]
    fn R7_单需求已批且路径无声明时放行() {
        // G4：单需求存量仓库行为与改造前逐字一致（未声明的路径不额外拦）
        let live = vec![req("REQ-001", true, &["core/**"])];
        let v = judge_ok(&live, &["docs/a.md"], None);
        assert!(v.is_pass(), "{v:?}");
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    // ── R8 ──────────────────────────────────────────────────────────────
    #[test]
    fn R8_多份清单且无声明命中时报歧义() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["cli/**"]),
        ];
        let v = judge_ok(&live, &["docs/a.md"], None);
        assert_eq!(v.block_kind(), Some(BlockKind::Ambiguous));
    }

    #[test]
    fn R8_msg_含三张牌() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["cli/**"]),
        ];
        let v = judge_ok(&live, &["docs/a.md"], None);
        let m = v.message().unwrap_or_default();
        for kw in ["touch --declare", "--req", "req-guard done"] {
            assert!(m.contains(kw), "出路「{kw}」缺失：\n{m}");
        }
        assert!(m.contains("docs/a.md"), "须点名本次改动：\n{m}");
        assert!(m.contains("core/**"), "须列出各候选声明以便自行判断：\n{m}");
    }

    // ── R9 ──────────────────────────────────────────────────────────────
    #[test]
    fn R9_hint与命中集不相交时拦() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["cli/**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], Some("REQ-002"));
        assert_eq!(v.block_kind(), Some(BlockKind::SelectionMismatch));
        let m = v.message().unwrap_or_default();
        assert!(m.contains("REQ-002") && m.contains("REQ-001"), "{m}");
    }

    #[test]
    fn R9_hint正是命中者时正常放行() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], Some("REQ-001"));
        assert!(v.is_pass(), "{v:?}");
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    #[test]
    fn R9_hint未批且不相交时拦的是hint本身() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], Some("REQ-002"));
        assert_eq!(
            v.block_kind(),
            Some(BlockKind::SelectionMismatch),
            "hint 不得成为绕过口：{v:?}"
        );
    }

    // ── R10 ─────────────────────────────────────────────────────────────
    #[test]
    fn R10_hint存在且命中集为空时以hint为唯一依据() {
        let live = vec![
            req("REQ-001", true, &["docs/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &["docs/a.md"], Some("REQ-001"));
        assert!(v.is_pass(), "{v:?}");
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    #[test]
    fn R10_hint本身未批时拦() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &["docs/a.md"], Some("REQ-002"));
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        assert_eq!(ids(&v), vec!["REQ-002".to_string()]);
    }

    // ── R5 ──────────────────────────────────────────────────────────────
    #[test]
    fn R5_豁免路径不参与反查且不降级为全量() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge_ok(&live, &[".gates/README.md", "target/x.bin"], None);
        assert_eq!(
            v.block_kind(),
            Some(BlockKind::Ambiguous),
            "剥完豁免即空集，不许当成「无改动」放行"
        );
    }

    #[test]
    fn R5_单需求时豁免路径不影响裁决() {
        let live = vec![req("REQ-001", true, &["core/**"])];
        let v = judge_ok(&live, &[".gates/README.md"], None);
        assert!(v.is_pass(), "{v:?}");
    }

    // ── R11 / R12 ───────────────────────────────────────────────────────
    #[test]
    fn R11_未批清单指名到步骤() {
        let live = vec![req("REQ-001", false, &["core/**"])];
        let v = judge_ok(&live, &["core/a.rs"], None);
        let m = v.message().unwrap_or_default();
        assert!(m.contains("REQ-001"), "{m}");
        assert!(m.contains("decomposition"), "须指名具体步骤：{m}");
        assert!(m.contains("approve"), "须给出修复命令：{m}");
    }

    #[test]
    fn R11_有未解决阻塞评论时拦() {
        let mut a = req("REQ-001", true, &["core/**"]);
        a.has_open_blocking = true;
        let v = judge_ok(&[a], &["core/a.rs"], None);
        assert_eq!(v.block_kind(), Some(BlockKind::OpenBlockingComment));
        assert!(v.message().unwrap_or_default().contains("resolve"));
    }

    #[test]
    fn R11_三段未批优先于阻塞评论() {
        let mut a = req("REQ-001", false, &["core/**"]);
        a.has_open_blocking = true;
        let v = judge_ok(&[a], &["core/a.rs"], None);
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
    }

    #[test]
    fn R12_全部过审时放行且reqs为全部候选() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["core/**"]),
        ];
        let v = judge_ok(&live, &["core/a.rs"], None);
        assert!(v.is_pass());
        assert_eq!(ids(&v), vec!["REQ-001".to_string(), "REQ-002".to_string()]);
    }

    // ── R13 ─────────────────────────────────────────────────────────────
    #[test]
    fn R13_保守档候选恒为live全体() {
        let live = vec![
            req("REQ-001", true, &["cli/**"]),
            req("REQ-002", false, &["core/**"]),
        ];
        let v = judge(&live, &p(&["core/a.rs"]), None, MultiMode::All, &ex());
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        assert_eq!(ids(&v), vec!["REQ-002".to_string()], "保守档与路径无关");
    }

    #[test]
    fn R13_保守档在无变更集时也拦未批() {
        let live = vec![
            req("REQ-001", true, &["cli/**"]),
            req("REQ-002", false, &["**"]),
        ];
        let v = judge(&live, &[], None, MultiMode::All, &ex());
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
    }

    // ── P3：`multi` 配置与分支名消歧（R13 / R14）──────────────────────

    #[test]
    fn match_branch_req_只认真实存在的id() {
        let known = vec!["REQ-001".to_string(), "REQ-002".to_string()];
        assert_eq!(
            match_branch_req("feature/REQ-001-x", &known),
            vec!["REQ-001"]
        );
        assert_eq!(match_branch_req("REQ-002", &known), vec!["REQ-002"]);
        assert_eq!(
            match_branch_req("REQ-001/fix-thing", &known),
            vec!["REQ-001"],
            "斜杠后的后缀不得并进 id"
        );
        assert_eq!(
            match_branch_req("feat/REQ-002-REQ-001", &known),
            vec!["REQ-002", "REQ-001"],
            "多个 id 按出现顺序返回，由调用方取第一个并告警"
        );
        assert!(match_branch_req("main", &known).is_empty());
        assert!(
            match_branch_req("feature/REQ-999", &known).is_empty(),
            "认不出就丢掉"
        );
        assert!(
            match_branch_req("feature/REQ-001-x", &["REQ-001-x".to_string()]) == vec!["REQ-001-x"],
            "带后缀的 id 本身存在时也要认（`REQ-004-req-002` 这类文件名形态）"
        );
    }

    #[test]
    fn multi_mode_默认resolve且可切all() {
        let root = crate::testutil::temp_dir("resolve-mode");
        std::fs::create_dir_all(root.join(".gates")).unwrap();
        assert_eq!(multi_mode(&root), MultiMode::Resolve, "缺省必须是 resolve");
        std::fs::write(root.join(".gates/req-guard.yaml"), "multi:\n  mode: all\n").unwrap();
        assert_eq!(multi_mode(&root), MultiMode::All);
        // 注释与大小写不得影响判定
        std::fs::write(
            root.join(".gates/req-guard.yaml"),
            "multi:\n  mode: ALL   # 保守档\n",
        )
        .unwrap();
        assert_eq!(multi_mode(&root), MultiMode::All);
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn multi_bind_branch_默认关闭() {
        let root = crate::testutil::temp_dir("resolve-bind");
        std::fs::create_dir_all(root.join(".gates")).unwrap();
        assert!(!crate::gate::multi_bind_branch(&root), "缺省必须关闭");
        std::fs::write(
            root.join(".gates/req-guard.yaml"),
            "multi:\n  bind_branch: true\n",
        )
        .unwrap();
        assert!(crate::gate::multi_bind_branch(&root));
        crate::testutil::cleanup(&root);
    }

    // ── P3：`owned_by` 与 judge 的候选集必须同源 ───────────────────────

    #[test]
    fn owned_by_与judge的候选集一致() {
        // 两处若各写一套归属逻辑，就会在「多份清单都声明了该文件」这类边界漂移。
        /// (live, paths, hint, mode)
        type Case = (
            Vec<LiveReq>,
            Vec<&'static str>,
            Option<&'static str>,
            MultiMode,
        );
        let cases: Vec<Case> = vec![
            (
                vec![
                    req("REQ-001", true, &["core/**"]),
                    req("REQ-002", false, &["**"]),
                ],
                vec!["core/a.rs"],
                None,
                MultiMode::Resolve,
            ),
            (
                vec![
                    req("REQ-001", true, &["core/**"]),
                    req("REQ-002", true, &["core/**"]),
                ],
                vec!["core/a.rs"],
                None,
                MultiMode::Resolve,
            ),
            (
                vec![
                    req("REQ-001", false, &["core/**"]),
                    req("REQ-002", true, &["cli/**"]),
                ],
                vec!["core/a.rs"],
                None,
                MultiMode::Resolve,
            ),
            (
                vec![
                    req("REQ-001", true, &["core/**"]),
                    req("REQ-002", true, &["cli/**"]),
                ],
                vec!["core/a.rs"],
                Some("REQ-002"),
                MultiMode::Resolve,
            ),
            (
                vec![req("REQ-001", true, &["docs/**"])],
                vec!["docs/a.md"],
                None,
                MultiMode::Resolve,
            ),
        ];
        for (live, paths, hint, mode) in cases {
            let owned: Vec<String> = owned_by(&live, &p(&paths), hint, &ex())
                .iter()
                .map(|r| r.id.clone())
                .collect();
            let v = judge(&live, &p(&paths), hint, mode, &ex());
            let from_verdict: Vec<String> = match &v {
                Verdict::Pass { reqs, .. } => reqs.clone(),
                Verdict::Bypassed { reqs, .. } => reqs.clone(),
                Verdict::Block { kind, reqs, .. } => {
                    // 拦截时 reqs 只列"有问题的那几份"，不能直接比；只比 kind 的可预期性
                    match kind {
                        BlockKind::Ambiguous
                        | BlockKind::SelectionMismatch
                        | BlockKind::UnknownSelection
                        | BlockKind::NoRequirement => {
                            assert!(
                                owned.is_empty(),
                                "{kind:?} 时 owned_by 应为空，实际 {owned:?}"
                            );
                            continue;
                        }
                        _ => reqs.clone(),
                    }
                }
            };
            assert_eq!(owned, from_verdict, "paths={paths:?} hint={hint:?}");
        }
    }

    #[test]
    fn owned_by_hint不相交时返回空() {
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["cli/**"]),
        ];
        assert!(owned_by(&live, &p(&["core/a.rs"]), Some("REQ-002"), &ex()).is_empty());
    }

    #[test]
    fn owned_by_命中集为空时以hint为唯一依据() {
        // 路径不属于任何已批清单 → 命中集为空 → hint 是唯一依据（R10）
        let live = vec![
            req("REQ-001", true, &["core/**"]),
            req("REQ-002", true, &["docs/**"]),
        ];
        let owned = owned_by(&live, &p(&["zzz/a.md"]), Some("REQ-001"), &ex());
        assert_eq!(
            owned.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["REQ-001"]
        );
    }

    // ── 枚举覆盖门槛 ────────────────────────────────────────────────────
    #[test]
    fn 枚举覆盖门槛_每个BlockKind至少被一个用例命中() {
        use std::collections::BTreeMap;
        let mut hit: BTreeMap<BlockKind, Vec<&str>> = BTreeMap::new();
        for (name, v) in cases() {
            if let Some(k) = v.block_kind() {
                hit.entry(k).or_default().push(name);
            }
        }
        for &k in ALL_KINDS.iter() {
            assert!(
                hit.get(&k).map(|v| !v.is_empty()).unwrap_or(false),
                "BlockKind::{} 没有任何用例命中（枚举即契约：新增变体必须被测）",
                k.as_str()
            );
        }
    }

    const ALL_KINDS: [BlockKind; 7] = [
        BlockKind::NoRequirement,
        BlockKind::UnknownSelection,
        BlockKind::Ambiguous,
        BlockKind::SelectionMismatch,
        BlockKind::StepNotApproved,
        BlockKind::OpenBlockingComment,
        BlockKind::SumMismatch,
    ];

    #[test]
    fn 枚举覆盖门槛_ALL_KINDS与枚举定义一致() {
        // 防「加了变体但忘了改本表」——那种情况下上表会静默少覆盖一个
        assert_eq!(ALL_KINDS.len(), 7);
        for k in ALL_KINDS {
            // as_str 不得为占位（枚举覆盖用例会靠它指出是哪个变体）
            assert!(!k.as_str().is_empty());
        }
    }

    /// 全部裁决场景的单一事实源：枚举覆盖门槛靠它判定「每个变体被测到了」。
    fn cases() -> Vec<(&'static str, Verdict)> {
        vec![
            (
                "R1",
                judge(&[], &p(&["core/a.rs"]), None, MultiMode::Resolve, &ex()),
            ),
            (
                "R3",
                judge_ok(
                    &[req("REQ-001", true, &["core/**"])],
                    &["core/a.rs"],
                    Some("REQ-9"),
                ),
            ),
            (
                "R8",
                judge_ok(
                    &[
                        req("REQ-001", true, &["core/**"]),
                        req("REQ-002", true, &["cli/**"]),
                    ],
                    &["docs/a.md"],
                    None,
                ),
            ),
            (
                "R9",
                judge_ok(
                    &[
                        req("REQ-001", true, &["core/**"]),
                        req("REQ-002", true, &["cli/**"]),
                    ],
                    &["core/a.rs"],
                    Some("REQ-002"),
                ),
            ),
            (
                "R11",
                judge_ok(&[req("REQ-001", false, &["core/**"])], &["core/a.rs"], None),
            ),
            ("R11b", {
                let mut a = req("REQ-001", true, &["core/**"]);
                a.has_open_blocking = true;
                judge_ok(&[a], &["core/a.rs"], None)
            }),
            ("R15", {
                let mut b = req("REQ-002", true, &["cli/**"]);
                b.sum_errors = vec!["正文被改".into()];
                judge_ok(
                    &[req("REQ-001", true, &["core/**"]), b],
                    &["core/a.rs"],
                    None,
                )
            }),
        ]
    }

    // ── 快照：段落定位失败必须按空声明 ──────────────────────────────────
    #[test]
    fn declares_段落标题写坏时按空处理() {
        let bad = "# REQ-001 x\n\n<!-- GATE:TOUCH -->\ncore/**\n<!-- /GATE:TOUCH -->\n";
        assert!(declares_of(bad).is_empty(), "定位失败不得回退整篇取块");
    }

    #[test]
    fn declares_正常清单取出声明() {
        let c = "# REQ-001 x\n\n## 2. 技术方案\n\n说明\n\n<!-- GATE:TOUCH -->\ncore/**\ncli/src/main.rs\n<!-- /GATE:TOUCH -->\n\n## 3. 测试计划\n\nx\n";
        assert_eq!(declares_of(c), vec!["core/**", "cli/src/main.rs"]);
    }

    #[test]
    fn declares_围栏内的二级标题不切错边界() {
        // REQ-008 的围栏修复是本模块的前置依赖：切错 → declares 取空 → 误拦
        let c = "# REQ-001 x\n\n## 2. 技术方案\n\n```\n## 2. 技术方案\n```\n\n<!-- GATE:TOUCH -->\ncore/**\n<!-- /GATE:TOUCH -->\n\n## 3. 测试计划\n\nx\n";
        assert_eq!(declares_of(c), vec!["core/**"]);
    }

    #[test]
    fn split_sums_按严重级拆两半() {
        let sealed = {
            let mut s =
                String::from("# REQ-001 x\n\n<!-- GATE:HEAD id=REQ-001 status=approved -->\n");
            for (k, l) in STEPS.iter() {
                s.push_str(&format!(
                    "<!-- GATE:STEP name={k} label={l} status=approved reviewer=- updated=- sum=- -->\n"
                ));
            }
            s.push_str(
                "\n## 1. 需求分解\n\n- x\n\n## 2. 技术方案\n\n- x\n\n## 3. 测试计划\n\n- x\n",
            );
            s
        };
        let (errors, warnings) = split_sums(&sealed);
        assert!(errors.is_empty(), "sum=- 只是告警：{errors:?}");
        assert_eq!(warnings.len(), 3, "三段各一条告警：{warnings:?}");
    }

    #[test]
    fn key_value_只认合法数字且不误匹配相似键() {
        assert_eq!(
            key_value("a=1\nexpires_epoch=123\n", "expires_epoch"),
            Some(123)
        );
        assert_eq!(key_value("expires_epoch=abc\n", "expires_epoch"), None);
        assert_eq!(key_value("x_expires_epoch=9\n", "expires_epoch"), None);
        assert_eq!(key_value("", "expires_epoch"), None);
    }

    // ── 快照（真机文件） ────────────────────────────────────────────────
    fn write_req(dir: &Path, id: &str, head: &str, step: &str, declares: &[&str]) {
        let mut s =
            format!("# {id} 测试\n\n<!-- GATE:HEAD id={id} status={head} created=2026-10-04 -->\n");
        for (k, l) in STEPS.iter() {
            s.push_str(&format!(
                "<!-- GATE:STEP name={k} label={l} status={step} reviewer=- updated=- -->\n"
            ));
        }
        s.push_str(
            "\n## 1. 需求分解\n\n- 背景：本用例夹具。\n\n## 2. 技术方案\n\n- 思路：夹具。\n",
        );
        if !declares.is_empty() {
            s.push_str("\n<!-- GATE:TOUCH -->\n");
            for d in declares {
                s.push_str(&format!("{d}\n"));
            }
            s.push_str("<!-- /GATE:TOUCH -->\n");
        }
        s.push_str("\n## 3. 测试计划\n\n- 计划：夹具。\n");
        std::fs::write(dir.join(format!("{id}.md")), s).unwrap();
    }

    #[test]
    fn live_snapshot_过滤done清单() {
        let root = crate::testutil::temp_dir("resolve-live");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        write_req(&dir, "REQ-002", "approved", "approved", &["cli/**"]);
        write_req(&dir, "REQ-003", "done", "approved", &["docs/**"]);
        let live = live_snapshot(&root).unwrap();
        let ids: Vec<&str> = live.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["REQ-001", "REQ-002"], "done 清单不得进 live");
        assert!(live[0].approved);
        assert_eq!(live[0].declares, vec!["core/**"]);
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn live_snapshot_未批清单记下未过审步骤() {
        let root = crate::testutil::temp_dir("resolve-steps");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "draft", "pending", &["core/**"]);
        let live = live_snapshot(&root).unwrap();
        assert!(!live[0].approved);
        assert_eq!(
            live[0].pending_steps,
            vec!["decomposition", "solution", "testplan"]
        );
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn live_snapshot_评论文件读不出时按有阻塞处理() {
        // 读不到评论 ≠ 没有阻塞评论 —— 那是「看着在拦、其实没拦」。
        // 非 UTF-8 的评论文件让 `comment::list` 走 `Err` 分支（文件不存在不算错，
        // 返回空 Vec 是正确的 —— 「没有评论」与「读不到评论」是两件事）。
        let root = crate::testutil::temp_dir("resolve-cmt-bad");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        std::fs::write(dir.join("REQ-001.comments.md"), [0xff, 0xfe, 0x00]).unwrap();
        let live = live_snapshot(&root).unwrap();
        assert_eq!(live.len(), 1);
        assert!(live[0].has_open_blocking, "评论读失败不得判成「无阻塞」");
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn live_snapshot_无评论文件且清单可读时判无阻塞() {
        let root = crate::testutil::temp_dir("resolve-nocmt");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        // `comment::list` 对不存在的评论文件返回空 Vec（不报错），故这里应有阻塞=false
        let live = live_snapshot(&root).unwrap();
        assert!(!live[0].has_open_blocking);
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn resolve_无清单时报NoRequirement且不读bypass() {
        let root = crate::testutil::temp_dir("resolve-noreq");
        crate::testutil::disable_auth(&root);
        let v = resolve(
            &root,
            &Ctx {
                source: PathSource::None,
                hint: None,
            },
        )
        .unwrap();
        assert_eq!(v.block_kind(), Some(BlockKind::NoRequirement));
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn resolve_绕过窗口命中时放行并标记() {
        let root = crate::testutil::temp_dir("resolve-bypass");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "draft", "pending", &["core/**"]);
        std::fs::write(
            root.join(".gates/.bypass"),
            format!(
                "reason=t\nactor=t\ncreated_epoch=0\nexpires_epoch={}\n",
                crate::gate::now_epoch() + 3600
            ),
        )
        .unwrap();
        let v = resolve(
            &root,
            &Ctx {
                source: PathSource::None,
                hint: None,
            },
        )
        .unwrap();
        assert!(v.is_bypassed(), "{v:?}");
        assert_eq!(v.reqs(), &["REQ-001".to_string()]);
        let log =
            std::fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap_or_default();
        assert!(log.contains("BYPASS-HIT expires_epoch="), "{log}");
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn resolve_绕过过期时不走绕过() {
        let root = crate::testutil::temp_dir("resolve-bypass-old");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "draft", "pending", &["core/**"]);
        std::fs::write(
            root.join(".gates/.bypass"),
            "reason=t\nactor=t\ncreated_epoch=0\nexpires_epoch=1\n",
        )
        .unwrap();
        let v = resolve(
            &root,
            &Ctx {
                source: PathSource::None,
                hint: None,
            },
        )
        .unwrap();
        assert!(!v.is_bypassed());
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn resolve_已归档的指定ID在文案里点名() {
        let root = crate::testutil::temp_dir("resolve-arch");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        let adir = dir.join("archive/2026");
        std::fs::create_dir_all(&adir).unwrap();
        write_req(&adir, "REQ-002", "done", "approved", &["cli/**"]);
        let v = resolve(
            &root,
            &Ctx {
                source: PathSource::None,
                hint: Some("REQ-002".to_string()),
            },
        )
        .unwrap();
        assert_eq!(v.block_kind(), Some(BlockKind::UnknownSelection));
        let m = v.message().unwrap_or_default();
        assert!(m.contains("已归档"), "须点明已归档：\n{m}");
        crate::testutil::cleanup(&root);
    }

    // ── §0.1 三个实测场景的真机回归（改动前它们分别是漏拦 / 误锁 / 认错人）──

    /// 造一个「多需求并行」仓库：`approved` + `pending` 两份清单，各带声明。
    fn parallel_repo(tag: &str) -> (PathBuf, PathBuf) {
        let root = crate::testutil::temp_dir(tag);
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        crate::testutil::disable_auth(&root);
        (root, dir)
    }

    fn audit_tail(root: &Path) -> String {
        std::fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap_or_default()
    }

    #[test]
    fn 场景A_未批清单加已批清单_写未批那份范围内必须拦() {
        // 改造前：exit 0 放行，审计记 PASS <已批那份>（漏拦）
        let (root, dir) = parallel_repo("resolve-sceneA");
        write_req(&dir, "REQ-001", "draft", "pending", &["core/src/**"]);
        write_req(&dir, "REQ-002", "approved", "approved", &["cli/src/**"]);
        let live = live_snapshot(&root).unwrap();
        let v = decide(
            &root,
            &live,
            &p(&["core/src/resolve.rs"]),
            None,
            MultiMode::Resolve,
        );
        assert!(!v.is_pass(), "落在未批清单范围内的改动不得放行：{v:?}");
        assert_eq!(v.block_kind(), Some(BlockKind::Ambiguous));
        let log = audit_tail(&root);
        assert!(
            log.contains("BLOCK-AMBIGUOUS") && log.contains("REQ-002(approved)"),
            "审计须指名状态：\n{log}"
        );
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn 场景B_已批清单加未批清单_写已批那份范围内必须放行() {
        // 改造前：exit 1，文案「需求 REQ-002 尚未通过审核」（误锁）
        let (root, dir) = parallel_repo("resolve-sceneB");
        write_req(&dir, "REQ-001", "approved", "approved", &["core/src/**"]);
        write_req(&dir, "REQ-002", "draft", "pending", &["cli/src/**"]);
        let live = live_snapshot(&root).unwrap();
        let v = decide(
            &root,
            &live,
            &p(&["core/src/resolve.rs"]),
            None,
            MultiMode::Resolve,
        );
        assert!(v.is_pass(), "无关未批清单不得阻断已批清单：{v:?}");
        assert_eq!(v.reqs(), &["REQ-001".to_string()]);
        assert!(
            audit_tail(&root).contains("PASS REQ-001"),
            "审计须记被裁决的那份：\n{}",
            audit_tail(&root)
        );
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn 场景C_逆序字典序最大者不是最新版时按声明裁决() {
        // 改造前：只看文件名逆序第一份，与「最新」「正在做」都无关。
        // 这里 owner 命名（并行档）下 alice 更新，但 bob 的字典序更大。
        let (root, dir) = parallel_repo("resolve-sceneC");
        write_req(
            &dir,
            "REQ-alice-001",
            "approved",
            "approved",
            &["core/src/**"],
        );
        write_req(&dir, "REQ-bob-002", "draft", "pending", &["docs/**"]);
        let live = live_snapshot(&root).unwrap();
        let v = decide(
            &root,
            &live,
            &p(&["core/src/resolve.rs"]),
            None,
            MultiMode::Resolve,
        );
        assert!(v.is_pass(), "裁决对象由声明决定，不受文件名序影响：{v:?}");
        assert_eq!(v.reqs(), &["REQ-alice-001".to_string()]);
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn decide_审计记全部候选而非只记一份() {
        let (root, dir) = parallel_repo("resolve-audit-multi");
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        write_req(&dir, "REQ-002", "approved", "approved", &["core/**"]);
        let live = live_snapshot(&root).unwrap();
        let v = decide(&root, &live, &p(&["core/a.rs"]), None, MultiMode::Resolve);
        assert!(v.is_pass());
        assert!(
            audit_tail(&root).contains("PASS REQ-001 REQ-002"),
            "多候选必须全记，否则审计读者会以为只判了一份"
        );
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn decide_未过审步骤进审计明细() {
        let (root, dir) = parallel_repo("resolve-audit-steps");
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        write_req(&dir, "REQ-002", "approved", "approved", &["core/**"]);
        // 把 REQ-002 的 testplan 打回 pending
        let f = dir.join("REQ-002.md");
        let c = std::fs::read_to_string(&f).unwrap().replace(
            "name=testplan label=测试计划 status=approved",
            "name=testplan label=测试计划 status=pending",
        );
        std::fs::write(&f, c).unwrap();
        let live = live_snapshot(&root).unwrap();
        let v = decide(&root, &live, &p(&["core/a.rs"]), None, MultiMode::Resolve);
        // 未批的那份不在索引里 → 候选只剩已批那份 → 放行（G2）
        assert!(v.is_pass(), "{v:?}");
        // 审计里 PASS 只列已批的那份
        assert!(
            audit_tail(&root).contains("PASS REQ-001"),
            "{}",
            audit_tail(&root)
        );
        assert!(!audit_tail(&root).contains("PASS REQ-002"));
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn decide_hint消歧与hint不匹配() {
        let (root, dir) = parallel_repo("resolve-hint");
        write_req(&dir, "REQ-001", "approved", "approved", &["core/**"]);
        write_req(&dir, "REQ-002", "approved", "approved", &["docs/**"]);
        let live = live_snapshot(&root).unwrap();
        let ok = decide(
            &root,
            &live,
            &p(&["docs/a.md"]),
            Some("REQ-002"),
            MultiMode::Resolve,
        );
        assert!(ok.is_pass(), "{ok:?}");
        let bad = decide(
            &root,
            &live,
            &p(&["core/a.rs"]),
            Some("REQ-002"),
            MultiMode::Resolve,
        );
        assert_eq!(bad.block_kind(), Some(BlockKind::SelectionMismatch));
        crate::testutil::cleanup(&root);
    }

    #[test]
    fn decide_保守档在真实仓库里拦未批() {
        let (root, dir) = parallel_repo("resolve-mode-all");
        write_req(&dir, "REQ-001", "approved", "approved", &["cli/**"]);
        write_req(&dir, "REQ-002", "draft", "pending", &["core/**"]);
        let live = live_snapshot(&root).unwrap();
        let v = decide(&root, &live, &p(&["core/a.rs"]), None, MultiMode::All);
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        assert!(
            audit_tail(&root).contains("steps=decomposition+solution+testplan"),
            "{}",
            audit_tail(&root)
        );
        crate::testutil::cleanup(&root);
    }
}

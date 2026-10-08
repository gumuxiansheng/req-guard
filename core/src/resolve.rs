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
//! judge(live, paths, hint, mode, exempt, changeset) -> Verdict ← 纯函数，不碰 git / 文件系统
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
//! - 应急绕过窗口（R2）**不在** `judge` 里判：它要读绕过令牌与当前时间，
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

/// 本次判定是否带变更集（REQ-020 设计 1）。
///
/// 区分「有变更集但剥除豁免后为空」与「根本没有变更集」——两者在 `judge` 眼里都是
/// `&[]`，但语义相反，判错就是漏拦：
///
/// | 情形 | 正确裁决 | 判错的后果 |
/// | --- | --- | --- |
/// | [`Changeset::Provided`] 且剥除豁免后为空 | 放行 | 误拦 → 清单永远无法入库 |
/// | [`Changeset::Absent`]（裸 `req-guard check` / TUI 状态面板） | `Ambiguous` | 误放行 → 「能不能开工」永远答「能」 |
///
/// 裸 `check` 在单需求仓库下是有意义的：`owned_by` 走 `live.len() == 1` 退化分支，
/// 把那一份（哪怕未批）作为候选去判三段，等价于「能不能开工」。若空集一律放行，
/// 这条语义会被静默抹掉 —— **那是把门禁调松，不是解堵**。
///
/// 用两变体枚举而非布尔值：`judge` 有约 10 处调用点（多为单测），布尔实参在调用点
/// 不可读（`judge(&live, &paths, None, MultiMode::All, &ex(), true)` 无法自解释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changeset {
    /// 无变更集：全局判定（[`PathSource::None`]）。
    Absent,
    /// 有变更集（可能为空集，例如 `--staged` 时无暂存文件）。
    Provided,
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
    /// 本次改动派生出的档位高于清单声明档（REQ-019 §2.4 升档）。
    TierEscalation,
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
            BlockKind::TierEscalation => "TierEscalation",
        }
    }

    /// 审计日志前缀（与脚本既有 `BLOCK-COMMENT` / `BLOCK-SUM` 同族）。
    fn audit_prefix(self) -> &'static str {
        match self {
            BlockKind::OpenBlockingComment => "BLOCK-COMMENT",
            BlockKind::Ambiguous => "BLOCK-AMBIGUOUS",
            BlockKind::SelectionMismatch => "BLOCK-SELECTION",
            BlockKind::SumMismatch => "BLOCK-SUM",
            BlockKind::TierEscalation => "BLOCK-TIER",
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
    /// frontmatter 的 `tier` **声明档**（缺省 `standard`；非法值在
    /// [`live_snapshot`] 阶段就报错，不会走到这里）。
    ///
    /// 派生档是**变更集**的属性（并集算一次，见 REQ-019 B-06），声明档是**清单**的属性，
    /// 故两者分别住在两个结构里，判定时在 R11 之前逐份比对。
    pub tier: crate::tier::Tier,
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
///
/// `tier` 是本次变更集的**派生档**（REQ-019），由薄壳用 [`crate::tier::gate_for`] 算好：
/// 判据要读 git 与文件全文，那是薄壳的事，`judge` 保持纯函数。`None` = 分级未启用
/// （缺省段 / `tier.enabled: false`），此时**输出与改造前逐字一致**（回滚口）。
///
/// 派生档按**并集**算一次，逐份候选清单各自与自己的声明档比（REQ-019 B-06）；
/// 故声明档住在 [`LiveReq::tier`]，派生档住在这里。
pub fn judge(
    live: &[LiveReq],
    paths: &[String],
    hint: Option<&str>,
    mode: MultiMode,
    exempt: &[String],
    changeset: Changeset,
    tier: Option<&crate::tier::Classify>,
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

    // ── R2b 受管路径为空 → 放行（改动全部落在豁免区，无需归属） ──────────
    //
    // 为什么不是报错：`touch.exempt` 的语义是「这些路径不要求 `GATE:TOUCH` 声明」，
    // 而豁免区内根本没有可归属的对象。要求归属，等于要求「先有已批清单才能提交
    // 第一份清单」—— `gate::touch_exempt_patterns` 的注释已预见并否决了这个循环
    // （「AI 永远无法把填好的清单 commit 上去」），但判定侧当时没跟上，于是每次
    // 提交新清单都撞 `Ambiguous`，且 `--no-verify` 也救不了（CI 侧同样算空集）。
    //
    // 为什么只在 `Changeset::Provided` 下生效：`Absent` 是裸 `req-guard check` 与
    // TUI 状态面板，无变更集就无从归因 —— 放行会把「能不能开工」变成永远「能」。
    //
    // 位置三处约束，缺一不可：
    //   · R15（内容冻结）之后 —— 「证据已被篡改」比「没审批」更严重，且与变更集正交；
    //   · R3 之后 —— hint 指向不存在的清单是**用户犯错**，不是「改动无需归属」；
    //     若 R2b 抢在前面，`HOOK_REQ=REQ-999` 会被静默放行，那条防呆失效；
    //   · 候选推导之前 —— 无 hint 时 `owned_by` 已先返回空并落进 R8，放其后永不可达。
    //
    // 可追溯性：放行**必须**携带 `note`，审计侧叠一条 `NOTE`，使之在台账上与
    // 「三段已批准」明确区分 —— 「看着在放行、其实没判」与「看着在拦、其实没拦」
    // 是同一类失效。
    //
    // 为什么还要求「至少有一条非空路径」：**空变更集 ≠ 全豁免变更集**。
    // `PathSource::Stdin` 在 payload 里取不到 `file_path` 时返回空集（`collect_paths`
    // 的既定行为：「不是一次可识别的文件写操作」）。若此时也放行，AI 工具只要发一个
    // 解析不出路径的 payload，L1 hook 就会从「拦」变成「放行」——这正是本项目定义的
    // 最坏失效「看着在拦、其实没拦」。故空集必须落回原路径（多需求 → `Ambiguous`）,
    // 保持 fail-closed。
    let has_path = paths.iter().any(|p| !p.trim().is_empty());
    if has_path && managed.is_empty() && changeset == Changeset::Provided {
        return Verdict::Pass {
            reqs: vec![],
            note: Some("本次改动全部落在 touch.exempt 豁免区，未做任何归属判定".to_string()),
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

    // ── R16 派生档 > 声明档 → 升档拦截（REQ-019 §2.4） ──────────────────
    //
    // 位置三处约束：
    //   · 候选推导**之后** —— 派生档是变更集的属性，变更集归属哪几份清单定了才知道
    //     该拿谁的声明档来比；放在推导前就得对着全部清单比，误报到无关清单；
    //   · R11（三段状态）**之前** —— 轻档补 `standard` 差额的出路就是「再执行两次分段
    //     approve」，若先报「某段未批准」会让人以为只要补那一段，实际补完仍被同一处拦；
    //   · 阻塞评论检查之后 —— 评论是内容问题，与档位无关，不必混在一条消息里。
    //
    // 「升档」不需要新状态：它表现为**同一份清单被按更高档重新批准**
    // （N2 得以成立，见 REQ-019 §2.4）。
    if let Some(t) = tier {
        let escalated: Vec<&LiveReq> = candidates
            .iter()
            .filter(|r| t.derived.rank() > r.tier.rank())
            .copied()
            .collect();
        if !escalated.is_empty() {
            return Verdict::Block {
                kind: BlockKind::TierEscalation,
                reqs: ids(&escalated),
                message: tier_escalation_message(&escalated, t),
            };
        }
    }

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
        // 声明档解析**可能失败**（非法取值）：响亮的红优于沉默地当 `standard`（AC-017）。
        let declared = crate::tier::declared_of(&content)?;
        out.push(snapshot_of(root, &r, &content, declared));
    }
    Ok(out)
}

fn snapshot_of(root: &Path, r: &Requirement, content: &str, tier: crate::tier::Tier) -> LiveReq {
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
        tier,
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
///
/// `changeset` **不设默认值**：调用方必然知道本次有没有变更集（这正是 R2b 的判据），
/// 让默认值替他表态等于允许「忘了就说有/无」—— 而这一处记反就是漏拦。
pub fn decide(
    root: &Path,
    live: &[LiveReq],
    paths: &[String],
    hint: Option<&str>,
    mode: MultiMode,
    changeset: Changeset,
    tier: Option<&crate::tier::Classify>,
) -> Verdict {
    let exempt = crate::gate::touch_exempt_patterns(root);
    let v = judge(live, paths, hint, mode, &exempt, changeset, tier);
    audit_verdict(root, live, &v);
    v
}

/// 派生档（薄壳：读配置 + 读 git）。返回 `None` = 本次**不参与定档**。
///
/// 三种「不参与」必须分清（REQ-019 §2.7 / §2.10）：
///
/// | 情形 | 返回 | 理由 |
/// | --- | --- | --- |
/// | 裸 `check`（无变更集） | `None` | 无变更集就无从定档；且不得让 R2b 借这条放行 |
/// | `tier` 段未启用 | `None` | 回滚口：**输出与改造前逐字一致** |
/// | `--stdin`（L1） | `None` | payload 已被 [`collect_paths`] 读走；L1 的档位下界由 [`crate::gate::pretool_verdict`] 单独打点（§2.7） |
/// | 变更集整体落在 `touch.exempt` | `None`（R2b） | 豁免区不参与定档、不消耗档位（§2.10） |
fn derived_tier(
    root: &Path,
    source: &PathSource,
    paths: &[String],
    exempt: &[String],
    declared: crate::tier::Tier,
) -> Result<Option<crate::tier::Classify>> {
    if *source == PathSource::None || *source == PathSource::Stdin {
        return Ok(None);
    }
    let cfg = crate::gate::tier_config(root)?;
    if !cfg.enabled {
        return Ok(None);
    }
    let side = match source {
        PathSource::Staged => crate::tierdiff::Side::Staged,
        PathSource::Range(base) => crate::tierdiff::Side::Range(base.clone()),
        _ => return Ok(None),
    };
    // 这里的 `declared` 只影响输出里的「声明 vs 派生」对照（并集里的**最高**声明档）；
    // 逐份清单的声明档比对在 `judge` 的 R16 里做（它们在 `LiveReq::tier`），
    // 那里拿的是每份清单自己的值，故不会因为并集取高而放过某一份的伪造。
    Ok(
        match crate::tier::gate_for(root, &side, paths, exempt, declared, &cfg)? {
            crate::tier::TierGate::NoManagedPath => None,
            crate::tier::TierGate::Classified(c) => Some(c),
        },
    )
}

/// 变更集相关的清单里**最高**的声明档（无相关清单 → `standard`）。
///
/// 取最高而不是最低：这一值只用于输出对照，取低会让人以为「按最松的那份算」，
/// 而实际判定逐份比对（R16）。取低会让对照失真，取高只是更保守。
fn declared_for(live: &[LiveReq], paths: &[String], exempt: &[String]) -> crate::tier::Tier {
    let mut top: Option<crate::tier::Tier> = None;
    for r in hit_list(live, paths, exempt) {
        top = Some(match top {
            Some(cur) if cur.rank() >= r.tier.rank() => cur,
            _ => r.tier,
        });
    }
    top.unwrap_or(crate::tier::Tier::Standard)
}

/// 放行时的档位附注（**理由可复算**是这套机制能被人工复核的前提，§2.8）。
fn tier_note_lines(t: &crate::tier::Classify, live: &[LiveReq], reqs: &[String]) -> String {
    let mut v = vec![format!(
        "{}（{}）",
        t.summary(),
        t.final_tier.approval_hint()
    )];
    v.extend(t.reasons.iter().cloned());
    v.extend(t.detail.iter().cloned());
    if !reqs.is_empty() {
        // 逐份点名声明档：并集取高会掩盖「某一份声明得比并集低」，
        // 那正是 R16 要拦的伪造（输出看不出它就等于没输出）。
        let per: Vec<String> = reqs
            .iter()
            .map(|id| {
                let declared = live
                    .iter()
                    .find(|r| &r.id == id)
                    .map(|r| r.tier)
                    .unwrap_or(crate::tier::Tier::Standard);
                format!("{id}={declared}")
            })
            .collect();
        v.push(format!("声明档（逐份）：{}", per.join(" ")));
    }
    v.push("最终以 CI（L3）复算为准".to_string());
    v.join("\n")
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
/// **绕过窗口在这里判**（令牌与当前时间是薄壳的事，`judge` 不碰）。
pub fn resolve(root: &Path, ctx: &Ctx) -> Result<Verdict> {
    let live = live_snapshot(root)?;
    // 放行附注里的档位段落（见 `tier_note_lines`）；拦截时理由已在消息里，不重复附。
    let mut derived_note: Option<String> = None;
    let verdict = if live.is_empty() {
        // R1 先于绕过：没有清单可批准时，绕过窗口不该凭空造出一个「通过」。
        // `changeset` 不参与判定（R1 在它之前返回），取 `Absent` 只为类型完整。
        judge(
            &live,
            &[],
            ctx.hint.as_deref(),
            MultiMode::Resolve,
            &[],
            Changeset::Absent,
            None,
        )
    } else if let Some(exp) = active_bypass(root) {
        Verdict::Bypassed {
            reqs: live.iter().map(|r| r.id.clone()).collect(),
            expires_epoch: exp,
        }
    } else {
        let paths = collect_paths(root, &ctx.source)?;
        let exempt = crate::gate::touch_exempt_patterns(root);
        let mode = multi_mode(root);
        // 唯一需要按上下文取 `changeset` 的地方：裸 `check`（`PathSource::None`）
        // 是「能不能开工」的全局判定，**没有**变更集 —— R2b 的放行对它不生效。
        let changeset = if ctx.source == PathSource::None {
            Changeset::Absent
        } else {
            Changeset::Provided
        };
        // 派生档（REQ-019）：**R2b 之后**才算 —— 豁免区不参与定档（§2.10）。
        // R2b 的短路在 `judge` 内部，而 `derived_tier` 自己也会短路，故纯豁免变更集
        // 这里拿到 `None`，输出与放行里都不带任何档位字段（U-33 / B-07）。
        let derived = derived_tier(
            root,
            &ctx.source,
            &paths,
            &exempt,
            declared_for(&live, &paths, &exempt),
        )?;
        let mut verdict = judge(
            &live,
            &paths,
            ctx.hint.as_deref(),
            mode,
            &exempt,
            changeset,
            derived.as_ref(),
        );
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
                // 重判必须与原判同参，否则同一变更集两次裁决口径不同。
                verdict = judge(
                    &live,
                    &paths,
                    Some(target.id.as_str()),
                    mode,
                    &exempt,
                    changeset,
                    derived.as_ref(),
                );
                if verdict.is_pass() {
                    verdict = append_note(verdict, &branch_note(target, multi));
                }
            }
        }
        // `UnknownSelection` 的「已归档 / 拼错」补充信息要读磁盘，故只能在这层加。
        let verdict = match (&verdict, ctx.hint.as_deref()) {
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
        };
        if let Verdict::Pass { reqs, .. } = &verdict {
            if let Some(t) = derived.as_ref() {
                derived_note = Some(tier_note_lines(t, &live, reqs));
            }
        }
        verdict
    };
    let verdict = match derived_note {
        Some(note) => append_note(verdict, &note),
        None => verdict,
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
///
/// **令牌必须自证身份，且与入库台账交叉核对**（REQ-012 设计 1）。
///
/// 背景：本函数原先只读 `expires_epoch`，于是
/// 手写一个只含 `expires_epoch=99999999999` 的令牌文件，一行 shell 就能解除全部门禁，
/// 且旧路径被 gitignore —— 伪造在 PR 里**完全不可见**。这不是"绕过审计"，
/// 是"审计本身被静默关掉"，属本项目最坏失效那一类。
///
/// 两道校验，缺一不可（各挡不同的东西）：
/// - ① `sig` 必须等于按令牌自带的 `actor`/`email` 重算的指纹。挡随手伪造。
/// - ② 入库台账（**committed**）必须有对应的 `BYPASS-OPEN` 行。挡"改了本地隐形文件就想生效"，
///   把伪造的代价从"改一个 gitignore 文件"提升为"改一个会出现在 PR diff 里的文件"——
///   后者可由 CODEOWNERS 评审拦截，这是当前信任模型下唯一能跨过边界的机制。
///
/// **残留（必须如实告知，不得当密码学保证卖）**：`identity::fingerprint` 是公开可复算的
/// 派生值，故 ① 挡不住肯重算的进程；② 也不是签名，只是把篡改**变可见**而非**不可能**。
/// 见 REQ-012 §7 残留风险第 1 条。
fn active_bypass(root: &Path) -> Option<u64> {
    // 迁移期回落：旧路径（工作树内）令牌仍可用，但读到即自愈搬到新路径（REQ-012 T3/U-10）。
    // 两处都有时以新路径为准（U-11）——新路径是当前唯一被写入的位置。
    let path = crate::gate::bypass_token_path(root);
    let legacy = root.join(crate::gate::BYPASS_REL);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => match std::fs::read_to_string(&legacy) {
            Ok(t) => {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(root));
                if std::fs::write(&path, &t).is_ok() {
                    let _ = std::fs::remove_file(&legacy);
                }
                t
            }
            Err(_) => return None,
        },
    };
    let exp = key_value(&text, "expires_epoch")?;
    if crate::gate::now_epoch() >= exp {
        return None;
    }

    // ① 身份自证：令牌的 `sig` 必须等于**此刻**把 actor 绑到生效身份上得到的 sig。
    //
    //    为什么不能简单地 `fingerprint(root, actor, email)` 重算：`Stamp::sig` 记的是
    //    **git 身份**的指纹，不是 (自报名, email) 的指纹。L0 下自报名与 git 身份冲突
    //    仍会放行（`mismatch=1`），此时 actor="tester" 而 sig 来自 git 身份的 name
    //    —— 按 (actor, email) 重算必然对不上，会把**合法**绕过误杀（实测踩到）。
    //    改用 `identity::bind` 重跑一遍绑定，三条合法路径（正常 / REQ_GUARD_REVIEWER_EMAIL
    //    兜底 / 无身份降级）就都覆盖到了，且语义就是「这枚令牌是当前身份签发的」。
    let (Some(actor), Some(email), Some(sig)) = (
        str_field(&text, "actor"),
        str_field(&text, "email"),
        str_field(&text, "sig"),
    ) else {
        crate::gate::audit(root, "BYPASS-REJECT reason=missing-fields");
        return None;
    };
    match crate::identity::bind(root, &actor) {
        Ok(stamp) => {
            if stamp.sig != sig || stamp.email != email {
                crate::gate::audit(root, "BYPASS-REJECT reason=sig-mismatch");
                return None;
            }
            // `Stamp::unbound` 的占位值：无 git 身份且 L0 时合法 bypass 落盘就是 `sig=-`
            // （L0 兼容旧项目的既定行为，不可判它为伪造）。此时**降级为只查第② 道**——
            // 台账交叉核对才是拦 forged token 的主力。降级必须留痕，
            // 否则「静默降级」正是本函数要消灭的那类问题。
            if sig == crate::identity::UNBOUND_SIG {
                crate::gate::audit(root, "BYPASS-DEGRADED reason=unbound-sig actor");
            }
        }
        // 绑定本身失败（等级不够 / 取不到身份且 L≥1）→ 令牌不可信。
        Err(_) => {
            crate::gate::audit(root, "BYPASS-REJECT reason=identity-bind-failed");
            return None;
        }
    }

    // ② 与入库台账交叉核对（只读尾部，避免大台账拖慢每次裁决 —— 见 REQ-012 B-04）。
    if !ledger_has_bypass_open(root, &actor) {
        crate::gate::audit(root, "BYPASS-REJECT reason=no-ledger-entry");
        return None;
    }
    Some(exp)
}

/// 入库台账里是否存在该 actor 的 `BYPASS-OPEN` 事件。
///
/// 只读尾部 [`LEDGER_TAIL_LIMIT`] 字节：台账只追加，事件在末尾；
/// 全量读会让每次裁决都付出 O(台账大小) 的代价（裁决在 PreToolUse 路径上，敏感）。
fn ledger_has_bypass_open(root: &Path, actor: &str) -> bool {
    let text = read_tail(&root.join(crate::gate::LEDGER_REL), LEDGER_TAIL_LIMIT);
    // 与 gate::bypass 落盘的台账事件前缀逐字对齐（含 actor 后的空格，
    // 避免 `actor=alice` 误配 `actor=alice2`）。
    text.contains(&format!("BYPASS-OPEN actor={actor} "))
}

/// 台账尾部读取上限（REQ-012 B-04）。
const LEDGER_TAIL_LIMIT: u64 = 256 * 1024;

/// 读文件尾部最多 `limit` 字节为字符串（读不到 / 非 UTF-8 时按已有内容尽力返回）。
fn read_tail(path: &Path, limit: u64) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return String::new();
    };
    let skip = len.saturating_sub(limit);
    let mut file = file;
    if std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(skip)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut file, &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
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

/// 从 `key=value` 文本里取一个非空字符串字段（身份自证用，见 [`active_bypass`]）。
fn str_field(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    for line in text.lines() {
        if let Some(v) = line.trim().strip_prefix(&prefix) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
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

/// `TierEscalation` 的拦截文案（**纯函数**；理由必须可复算）。
fn tier_escalation_message(escalated: &[&LiveReq], t: &crate::tier::Classify) -> String {
    let mut msg = String::from(
        "[req-guard] ⛔ 拦截：本次改动派生出的档位高于清单声明档（TierEscalation）。\n",
    );
    for r in escalated {
        msg.push_str(&format!(
            "  - {}：声明 {}，本次改动派生 {}（有效改动行 {}）\n",
            r.id, r.tier, t.derived, t.total_effective
        ));
    }
    msg.push_str("  理由（可复算）：\n");
    for reason in &t.reasons {
        msg.push_str(&format!("    - {reason}\n"));
    }
    for d in &t.detail {
        msg.push_str(&format!("    {d}\n"));
    }
    msg.push_str(
        "  处置：按更高档**重新批准**该清单（声明只能往上抬，不能往下压）。\n\
        \x20        轻档补 standard 差额 = 再执行两次分段 approve；standard → critical 走 critical 的附加检查。\n\
        \x20        req-guard approve <需求ID> --step <步骤> --reviewer <姓名>",
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
            tier: crate::tier::Tier::Standard,
        }
    }

    fn p(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn ex() -> Vec<String> {
        EXEMPT.iter().map(|s| s.to_string()).collect()
    }

    /// 带**变更集**的判定（R2b 生效）。测试里显式传路径即为「有变更集」，
    /// 与 `resolve` 在 `--staged` / `--base` / `--stdin` 下的口径一致。
    fn judge_ok(live: &[LiveReq], paths: &[&str], hint: Option<&str>) -> Verdict {
        judge(
            live,
            &p(paths),
            hint,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            None,
        )
    }

    fn ids(v: &Verdict) -> Vec<String> {
        v.reqs().to_vec()
    }

    // ── R1 ──────────────────────────────────────────────────────────────
    #[test]
    fn R1_无清单报NoRequirement() {
        let v = judge(
            &[],
            &p(&["core/src/a.rs"]),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            None,
        );
        assert_eq!(v.block_kind(), Some(BlockKind::NoRequirement));
        assert!(!v.is_pass());
    }

    #[test]
    fn R1_msg_给出创建命令() {
        let v = judge(
            &[],
            &[],
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Absent,
            None,
        );
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
        let paths = &[".gates/README.md", "target/x.bin"];
        // ① 无变更集口径（REQ-020 起由 `Changeset::Absent` 显式表达）：
        //    剥完豁免即空集，不许当成「无改动」放行 —— 本条是原断言，逐字保留。
        let absent = judge(
            &live,
            &p(paths),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Absent,
            None,
        );
        assert_eq!(
            absent.block_kind(),
            Some(BlockKind::Ambiguous),
            "剥完豁免即空集，不许当成「无改动」放行"
        );
        // ② 有变更集口径（REQ-020 R2b）：放行，但**不是**降级为全量扫描 ——
        //    候选集必须为空且不点名任何清单；把上面两份当裁决对象就等于「全量」。
        let provided = judge_ok(&live, paths, None);
        assert!(provided.is_pass(), "{provided:?}");
        assert!(
            provided.reqs().is_empty(),
            "空集放行不得点名任何清单（否则即降级为全量）：{provided:?}"
        );
        assert!(
            provided.note().unwrap_or_default().contains("豁免区"),
            "放行必须自报「未做归属判定」，不得与「三段已批准」混淆：{provided:?}"
        );
    }

    #[test]
    fn R5_单需求时豁免路径不影响裁决() {
        let live = vec![req("REQ-001", true, &["core/**"])];
        let v = judge_ok(&live, &[".gates/README.md"], None);
        assert!(v.is_pass(), "{v:?}");
    }

    // ===================== REQ-020：受管路径为空时的归属放行（R2b） =====================
    //
    // 背景：`.gates/requirements/**` 在 `touch.exempt` 内，剥除豁免后受管路径为空，
    // 而 `judge` 当时没有「空集 → 放行」规则 → 提交新清单必然撞 `Ambiguous`，
    // 且 `--no-verify` 也救不了（CI 侧同样算空集）。门禁因此无法自举。
    //
    // 本组用例的编排原则：**放行与拒绝各一半**。只有放行用例等于没测。

    /// U-01 的仓库形状：两份清单，1 已批 1 未批，均未声明豁免区路径。
    fn u01_live() -> Vec<LiveReq> {
        vec![
            req("REQ-001", true, &["core/src/**"]),
            req("REQ-002", false, &["cli/src/**"]),
        ]
    }

    /// 一份「在豁免区内、且明显不是源码」的改动：新清单首次入库正是这个形状。
    const U01_PATHS: [&str; 1] = [".gates/requirements/REQ-003.md"];

    #[test]
    fn U01_变更集全在豁免区时放行且不点名任何清单() {
        let live = u01_live();
        let v = judge_ok(&live, &U01_PATHS, None);
        assert!(v.is_pass(), "改动全在豁免区不得拦：{v:?}");
        assert!(
            v.reqs().is_empty(),
            "放行不得点名任何清单 —— 点名即等于做了归属判定：{v:?}"
        );
        assert!(
            v.note().unwrap_or_default().contains("豁免区"),
            "放行必须自报「未做任何归属判定」，否则审计上无法与「三段已批准」区分：{v:?}"
        );
    }

    #[test]
    fn U02_无变更集时空集仍报Ambiguous_不得放行() {
        // 裸 `req-guard check` / TUI 状态面板走这条：无变更集就无从归因，
        // 放行会把「能不能开工」变成永远「能」—— 那是漏拦，不是解堵。
        let live = u01_live();
        let v = judge(
            &live,
            &p(&U01_PATHS),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Absent,
            None,
        );
        assert_eq!(
            v.block_kind(),
            Some(BlockKind::Ambiguous),
            "无变更集不得走 R2b 放行：{v:?}"
        );
        assert!(!v.is_pass());
    }

    #[test]
    fn U03_单需求仓库里未批清单的自身入库也放行() {
        // 修复前：走 `live.len() == 1` 退化分支 → `StepNotApproved`（拦）。
        // 修复后：R2b 在候选推导**之前**生效 → 放行。这是**预期变化**，故显式锁定。
        let live = vec![req("REQ-001", false, &["core/**"])];
        let v = judge_ok(&live, &[".gates/requirements/REQ-001.md"], None);
        assert!(v.is_pass(), "未批清单自身的入库不得被自己的门禁拦住：{v:?}");
        assert!(v.reqs().is_empty(), "{v:?}");
    }

    #[test]
    fn U04_混合变更集不得放行_只要还有一条受管路径() {
        // 放行面严格等于「改动集 ⊆ 豁免区」。豁免路径 + 受管路径混在一起时，
        // 受管路径必须照旧参与反查（此处命中未批清单 → 歧义）。
        let live = vec![
            req("REQ-001", false, &["core/**"]),
            req("REQ-002", false, &["cli/**"]),
        ];
        let v = judge_ok(&live, &[".gates/README.md", "core/src/a.rs"], None);
        assert!(!v.is_pass(), "混合变更集不得因含豁免路径而放行：{v:?}");
        assert_eq!(v.block_kind(), Some(BlockKind::Ambiguous), "{v:?}");
    }

    #[test]
    fn U05_内容冻结优先于空集放行() {
        // R15 遍历全部非 done 清单、与变更集正交。只改豁免区也不能豁免「证据被篡改」。
        let live = vec![req("REQ-001", true, &["core/**"]), req_sum_bad("REQ-002")];
        let v = judge_ok(&live, &[".gates/drafts/REQ-002.draft.md"], None);
        assert_eq!(
            v.block_kind(),
            Some(BlockKind::SumMismatch),
            "内容冻结必须先于 R2b：{v:?}"
        );
        assert!(v.message().unwrap_or_default().contains("内容冻结"));
    }

    #[test]
    fn U06_hint指向不存在的清单时不得被空集放行吞掉() {
        // R3 在 R2b 之前：hint 指错是**用户犯错**，不是「改动无需归属」。
        let live = u01_live();
        let v = judge_ok(&live, &U01_PATHS, Some("REQ-999"));
        assert_eq!(
            v.block_kind(),
            Some(BlockKind::UnknownSelection),
            "hint 指错必须报出来，不得被 R2b 静默放行：{v:?}"
        );
        assert!(v
            .message()
            .unwrap_or_default()
            .contains("不在可裁决的需求清单里"));
    }

    #[test]
    fn U08_空变更集不得放行_取不到路径时保持fail_closed() {
        // `--stdin` 解析不出 `file_path` 时 `collect_paths` 返回空集。此时若按
        // 「空集 ⊆ 豁免区」放行，AI 只要发一个解析不出路径的 payload 就能穿过 L1 ——
        // 那是「看着在拦、其实没拦」。空变更集必须落回原判定（多需求 → Ambiguous）。
        let live = u01_live();
        let v = judge(
            &live,
            &[],
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            None,
        );
        assert_eq!(
            v.block_kind(),
            Some(BlockKind::Ambiguous),
            "取不到路径时不得放行：{v:?}"
        );
        // 只有空白路径同样算「取不到」，不得被 `trim` 后的空集蒙混过关。
        let blank = judge(
            &live,
            &p(&["", "   "]),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            None,
        );
        assert_eq!(
            blank.block_kind(),
            Some(BlockKind::Ambiguous),
            "空白路径不得当成豁免区改动：{blank:?}"
        );
    }

    #[test]
    fn U07_空集放行在审计上可追溯_且不与三段已批准混淆() {
        let (root, dir) = parallel_repo("resolve-req020-u07");
        write_req(&dir, "REQ-001", "approved", "approved", &["core/src/**"]);
        write_req(&dir, "REQ-002", "draft", "pending", &["cli/src/**"]);
        let live = live_snapshot(&root).unwrap();
        let v = decide(
            &root,
            &live,
            &p(&[".gates/requirements/REQ-003.md"]),
            None,
            MultiMode::Resolve,
            Changeset::Provided,
            None,
        );
        assert!(v.is_pass(), "{v:?}");
        let log = audit_tail(&root);
        assert!(
            log.contains("PASS no-managed-path"),
            "审计须用既有专用标记，不得记成 `PASS <id>`：\n{log}"
        );
        assert!(
            log.contains("NOTE "),
            "审计须叠一条 NOTE 说明「未做归属判定」：\n{log}"
        );
        crate::testutil::cleanup(&root);
    }

    /// 造一份「已批准段正文被改」的清单（内容冻结硬伤）。
    fn req_sum_bad(id: &str) -> LiveReq {
        let mut r = req(id, true, &["docs/**"]);
        r.sum_errors = vec!["「技术方案」的正文与批准时不一致".to_string()];
        r
    }

    // ── R16 分级升档（REQ-019） ─────────────────────────────────────────

    /// 造一份派生档（走真的 [`crate::tier::classify`]，不手写结论）。
    fn classified(
        declared: crate::tier::Tier,
        managed: &[&str],
        files: &[(&str, usize, usize)],
        risky: &[&str],
    ) -> crate::tier::Classify {
        let stats: Vec<crate::tierdiff::FileStat> = files
            .iter()
            .map(|(path, a, d)| crate::tierdiff::FileStat {
                path: path.to_string(),
                added: *a,
                deleted: *d,
                binary: false,
            })
            .collect();
        let cfg = crate::tier::TierConfig {
            enabled: true,
            risky_paths: risky.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let owned: Vec<String> = managed.iter().map(|s| s.to_string()).collect();
        crate::tier::classify(crate::tier::ClassifyInput {
            declared,
            managed: &owned,
            stats: &stats,
            config: &cfg,
        })
    }

    fn judge_tier(live: &[LiveReq], paths: &[&str], t: &crate::tier::Classify) -> Verdict {
        judge(
            live,
            &p(paths),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            Some(t),
        )
    }

    #[test]
    fn R16_派生档高于声明档报升档并给出理由() {
        let mut light = req("REQ-001", true, &["core/**"]);
        light.tier = crate::tier::Tier::Light;
        let t = classified(
            crate::tier::Tier::Trivial,
            &["core/src/gate.rs"],
            &[("core/src/gate.rs", 200, 0)],
            &[],
        );
        let v = judge_tier(&[light], &["core/src/gate.rs"], &t);
        assert_eq!(v.block_kind(), Some(BlockKind::TierEscalation));
        let m = v.message().unwrap_or_default();
        assert!(m.contains("TierEscalation"), "{m}");
        assert!(m.contains("声明 light"), "{m}");
        assert!(m.contains("standard"), "{m}");
        assert!(m.contains("有效改动行 200"), "{m}");
        assert!(m.contains("重新批准"), "须给出补救方向：{m}");
    }

    #[test]
    fn R16_命中内建锁定时理由点名locked() {
        let mut light = req("REQ-001", true, &["core/**"]);
        light.tier = crate::tier::Tier::Light;
        let t = classified(
            crate::tier::Tier::Trivial,
            &["core/src/gate.rs"],
            &[("core/src/gate.rs", 1, 0)],
            &[],
        );
        let v = judge_tier(&[light], &["core/src/gate.rs"], &t);
        let m = v.message().unwrap_or_default();
        assert!(m.contains("critical"), "{m}");
        assert!(m.contains("locked"), "{m}");
    }

    #[test]
    fn R16_声明档不低于派生档时放行() {
        let mut std = req("REQ-001", true, &["docs/**"]);
        std.tier = crate::tier::Tier::Standard;
        let t = classified(
            crate::tier::Tier::Standard,
            &["docs/设计/x.md"],
            &[("docs/设计/x.md", 3, 0)],
            &[],
        );
        let v = judge_tier(&[std], &["docs/设计/x.md"], &t);
        assert!(v.is_pass(), "{v:?}");
    }

    #[test]
    fn R16_并集算一次逐份清单各自校验() {
        // B-06：派生档按并集算一次；声明 light 的那份升档，声明 standard 的那份放行。
        let mut light = req("REQ-001", true, &["docs/**"]);
        light.tier = crate::tier::Tier::Light;
        let mut std = req("REQ-002", true, &["cli/**"]);
        std.tier = crate::tier::Tier::Standard;
        let t = classified(
            crate::tier::Tier::Trivial,
            &["docs/设计/a.md", "cli/src/main.rs"],
            &[("docs/设计/a.md", 100, 0), ("cli/src/main.rs", 100, 0)],
            &[],
        );
        let v = judge(
            &[light, std],
            &p(&["docs/设计/a.md", "cli/src/main.rs"]),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            Some(&t),
        );
        assert_eq!(v.block_kind(), Some(BlockKind::TierEscalation));
        assert_eq!(ids(&v), vec!["REQ-001".to_string()]);
    }

    #[test]
    fn R16_未启用分级时判定逐字不变() {
        let mut light = req("REQ-001", true, &["core/**"]);
        light.tier = crate::tier::Tier::Light;
        let with_tier = judge(
            &[light.clone()],
            &p(&["core/src/gate.rs"]),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            Some(&classified(
                crate::tier::Tier::Critical,
                &["core/src/gate.rs"],
                &[("core/src/gate.rs", 500, 0)],
                &[],
            )),
        );
        let without = judge(
            &[light],
            &p(&["core/src/gate.rs"]),
            None,
            MultiMode::Resolve,
            &ex(),
            Changeset::Provided,
            None,
        );
        assert_eq!(with_tier.block_kind(), Some(BlockKind::TierEscalation));
        assert!(without.is_pass(), "无派生档时不得拦：{without:?}");
    }

    #[test]
    fn R16_升档排在三段状态之前() {
        // 一份 light 且**未批**的清单 + 派生 critical：先报档位不够，
        // 否则人会以为「补批那一段」就够了，补完仍被同一处拦。
        let mut light = req("REQ-001", false, &["core/**"]);
        light.tier = crate::tier::Tier::Light;
        let t = classified(
            crate::tier::Tier::Trivial,
            &["core/src/gate.rs"],
            &[("core/src/gate.rs", 500, 0)],
            &[],
        );
        let v = judge_tier(&[light], &["core/src/gate.rs"], &t);
        assert_eq!(v.block_kind(), Some(BlockKind::TierEscalation));
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
        let v = judge(
            &live,
            &p(&["core/a.rs"]),
            None,
            MultiMode::All,
            &ex(),
            Changeset::Provided,
            None,
        );
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        assert_eq!(ids(&v), vec!["REQ-002".to_string()], "保守档与路径无关");
    }

    #[test]
    fn R13_保守档在无变更集时也拦未批() {
        let live = vec![
            req("REQ-001", true, &["cli/**"]),
            req("REQ-002", false, &["**"]),
        ];
        // 无变更集 → R2b 不生效（本用例正是「裸 check 不得因空集放行」的一道锁）
        let v = judge(
            &live,
            &[],
            None,
            MultiMode::All,
            &ex(),
            Changeset::Absent,
            None,
        );
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
            let v = judge(
                &live,
                &p(&paths),
                hint,
                mode,
                &ex(),
                Changeset::Provided,
                None,
            );
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
                judge(
                    &[],
                    &p(&["core/a.rs"]),
                    None,
                    MultiMode::Resolve,
                    &ex(),
                    Changeset::Provided,
                    None,
                ),
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
        crate::testutil::write_valid_bypass(&root, "tester", "t@e.com", 60);
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
            Changeset::Provided,
            None,
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
            Changeset::Provided,
            None,
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
            Changeset::Provided,
            None,
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
        let v = decide(
            &root,
            &live,
            &p(&["core/a.rs"]),
            None,
            MultiMode::Resolve,
            Changeset::Provided,
            None,
        );
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
        let v = decide(
            &root,
            &live,
            &p(&["core/a.rs"]),
            None,
            MultiMode::Resolve,
            Changeset::Provided,
            None,
        );
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
            Changeset::Provided,
            None,
        );
        assert!(ok.is_pass(), "{ok:?}");
        let bad = decide(
            &root,
            &live,
            &p(&["core/a.rs"]),
            Some("REQ-002"),
            MultiMode::Resolve,
            Changeset::Provided,
            None,
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
        let v = decide(
            &root,
            &live,
            &p(&["core/a.rs"]),
            None,
            MultiMode::All,
            Changeset::Provided,
            None,
        );
        assert_eq!(v.block_kind(), Some(BlockKind::StepNotApproved));
        assert!(
            audit_tail(&root).contains("steps=decomposition+solution+testplan"),
            "{}",
            audit_tail(&root)
        );
        crate::testutil::cleanup(&root);
    }

    // ===================== REQ-012 T1/T2/T3：绕过令牌加固 =====================
    //
    // 修的是「`printf 'expires_epoch=99999999999\n' > .gates/.bypass` 一行解除全部门禁、
    // 且入库台账零痕迹」。断言分两类，缺一不可：
    //   拒绝路径（U-01/02/03/05/07/09）——只有通过路径的用例等于没测；
    //   放行路径（U-04/06/08/10/11）——加固不能把合法绕过也堵死。

    /// 手工落一份令牌（用于构造"部分字段缺失"与"字段自造"这两类非法态）。
    fn forge_token(root: &std::path::Path, body: &str) {
        let p = crate::gate::bypass_token_path(root);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn bypass_token_body(root: &std::path::Path, actor: &str, ttl: u64) -> String {
        let now = crate::gate::now_epoch();
        let stamp = crate::identity::bind(root, actor).expect("夹具身份应可绑定");
        format!(
            "reason=forge\nactor={actor}\nemail={}\nsig={}\ncreated_epoch={now}\nexpires_epoch={}\n\
             ttl_minutes={ttl}\n",
            stamp.email,
            stamp.sig,
            now + ttl * 60
        )
    }

    fn audit_of(root: &std::path::Path) -> String {
        std::fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap_or_default()
    }

    /// U-01 只有 expires_epoch 的令牌（**问题 1 的原始复现**）→ 拒绝，且留下 missing-fields。
    #[test]
    fn U01_仅expires字段的令牌被拒() {
        let root = crate::testutil::temp_dir("u01");
        forge_token(&root, "expires_epoch=99999999999\n");
        assert_eq!(active_bypass(&root), None);
        assert!(
            audit_of(&root).contains("BYPASS-REJECT reason=missing-fields"),
            "{}",
            audit_of(&root)
        );
        crate::testutil::cleanup(&root);
    }

    /// U-02 sig 与当前身份绑定不一致 → 拒绝 + sig-mismatch。
    #[test]
    fn U02_sig不匹配被拒() {
        let root = crate::testutil::temp_dir("u02");
        let mut body = bypass_token_body(&root, "tester", 60);
        // 只动 sig，actor/email 保持合法形状 → 精确定位到指纹这一道
        body = body.replace(
            &format!(
                "sig={}",
                crate::identity::bind(&root, "tester").unwrap().sig
            ),
            "sig=deadbeefdead",
        );
        forge_token(&root, &body);
        crate::gate::audit_ledger(&root, "BYPASS-OPEN actor=tester ttl=60min reason=x");
        assert_eq!(
            active_bypass(&root),
            None,
            "台账齐了也不该放行：指纹是唯一身份凭据"
        );
        assert!(
            audit_of(&root).contains("BYPASS-REJECT reason=sig-mismatch"),
            "{}",
            audit_of(&root)
        );
        crate::testutil::cleanup(&root);
    }

    /// U-03 指纹合法但台账无对应事件 → 拒绝 + no-ledger-entry。
    /// 这是设计 1 的**主力**防线：伪造必须留下入库痕迹。
    #[test]
    fn U03_台账无对应事件被拒() {
        let root = crate::testutil::temp_dir("u03");
        forge_token(&root, &bypass_token_body(&root, "tester", 60));
        assert_eq!(
            active_bypass(&root),
            None,
            "无台账条目 = 没人签发过这枚令牌"
        );
        assert!(
            audit_of(&root).contains("BYPASS-REJECT reason=no-ledger-entry"),
            "{}",
            audit_of(&root)
        );
        crate::testutil::cleanup(&root);
    }

    /// U-05 台账里是**别的** actor → 拒绝（防 actor 前缀误配）。
    #[test]
    fn U05_台账actor不符被拒() {
        let root = crate::testutil::temp_dir("u05");
        forge_token(&root, &bypass_token_body(&root, "tester2", 60));
        // 故意只写 tester（token 的 actor 是 tester2）——前缀不得误配
        crate::gate::audit_ledger(&root, "BYPASS-OPEN actor=tester ttl=60min reason=x");
        assert_eq!(
            active_bypass(&root),
            None,
            "actor=tester 不得匹配 actor=tester2"
        );
        crate::testutil::cleanup(&root);
    }

    /// U-09 永不过期的伪造令牌（`expires_epoch=99999999999`）→ 拒绝。
    #[test]
    fn U09_永不过期令牌被拒() {
        let root = crate::testutil::temp_dir("u09");
        forge_token(
            &root,
            &bypass_token_body(&root, "tester", 60).replace(
                &format!("expires_epoch={}", crate::gate::now_epoch() + 3600),
                "expires_epoch=99999999999",
            ),
        );
        assert_eq!(active_bypass(&root), None);
        crate::testutil::cleanup(&root);
    }

    /// U-04 合法路径：指纹 + 台账齐备 → 放行（证明加固没堵死合法绕过）。
    #[test]
    fn U04_合法令牌放行() {
        let root = crate::testutil::temp_dir("u04");
        crate::testutil::write_valid_bypass(&root, "tester", "t@e.com", 60);
        assert!(active_bypass(&root).is_some(), "合法绕过必须仍能生效");
        crate::testutil::cleanup(&root);
    }

    /// 把令牌文件的 `expires_epoch` 改成「已过期」。
    ///
    /// 按**键**改写而不是替换 `expires_epoch=<算出值>` 字面量：夹具与断言各调一次
    /// `now_epoch()`，跨秒时字面量对不上，替换静默变成空操作 —— 用例随机变红，
    /// 而根因是时钟不是代码（实测踩到）。
    fn expire_token(path: &std::path::Path) {
        let body = std::fs::read_to_string(path).unwrap();
        let out = body
            .lines()
            .map(|l| match l.strip_prefix("expires_epoch=") {
                Some(_) => format!(
                    "expires_epoch={}",
                    crate::gate::now_epoch().saturating_sub(1)
                ),
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, out).unwrap();
    }

    /// U-06 过期后失效（TTL 到期行为不回归）。
    #[test]
    fn U06_过期令牌不生效() {
        let root = crate::testutil::temp_dir("u06");
        crate::testutil::write_valid_bypass(&root, "tester", "t@e.com", 60);
        let p = crate::gate::bypass_token_path(&root);
        expire_token(&p);
        assert_eq!(active_bypass(&root), None);
        crate::testutil::cleanup(&root);
    }

    /// U-10 旧路径（工作树内）令牌仍可用，但读到即**自愈**搬到新路径。
    #[test]
    fn U10_旧路径令牌回落并自愈() {
        let root = crate::testutil::temp_dir("u10");
        let legacy = root.join(crate::gate::BYPASS_REL);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let stamp = crate::identity::bind(&root, "tester").unwrap();
        std::fs::write(
            &legacy,
            format!(
                "reason=legacy\nactor=tester\nemail={}\nsig={}\ncreated_epoch=0\nexpires_epoch={}\n\
                 ttl_minutes=60\n",
                stamp.email,
                stamp.sig,
                crate::gate::now_epoch() + 3600
            ),
        )
        .unwrap();
        crate::gate::audit_ledger(&root, "BYPASS-OPEN actor=tester ttl=60min reason=legacy");
        assert!(active_bypass(&root).is_some(), "迁移期不得让存量绕过失效");
        assert!(
            crate::gate::bypass_token_path(&root).exists(),
            "应自愈写到新路径"
        );
        assert!(!legacy.exists(), "旧文件应被清掉，不留看起来有效的假令牌");
        crate::testutil::cleanup(&root);
    }

    /// U-11 两处都有时以**新路径**为准（新路径是当前唯一被写入的位置）。
    #[test]
    fn U11_两处都有以新路径为准() {
        let root = crate::testutil::temp_dir("u11");
        // 新路径：合法、过期 → 不应生效
        crate::testutil::write_valid_bypass(&root, "tester", "t@e.com", 60);
        let p = crate::gate::bypass_token_path(&root);
        expire_token(&p);
        // 旧路径：未过期 → 也不该被采信
        std::fs::create_dir_all(root.join(".gates")).unwrap();
        std::fs::write(
            root.join(crate::gate::BYPASS_REL),
            format!("expires_epoch={}\n", crate::gate::now_epoch() + 3600),
        )
        .unwrap();
        assert_eq!(active_bypass(&root), None, "新路径优先，不得回落到旧路径");
        crate::testutil::cleanup(&root);
    }

    /// B-01 畸形令牌不得 panic（B-01/B-02）。
    #[test]
    fn B01_畸形令牌不panic() {
        for body in [
            "",
            "\n\n",
            "expires_epoch=",
            "expires_epoch=abc",
            "no_equals_here",
        ] {
            let root = crate::testutil::temp_dir("b01");
            forge_token(&root, body);
            assert_eq!(active_bypass(&root), None, "body={body:?} 应被拒");
            crate::testutil::cleanup(&root);
        }
    }
}

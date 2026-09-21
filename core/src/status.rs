//! 结构化的清单状态模型。
//!
//! **为什么单独成型**：CLI 只打印文本、TUI/GUI 要渲染控件与颜色、CI 只判退出码——
//! 三者都需要"数据"而不是一堆 `println`。把状态解析收敛为结构体后，
//! 各前端只负责渲染，`approved / pending / rejected` 的语义只有一份实现，
//! 从根本上杜绝"GUI 与 CLI 判定不一致"。

use crate::comment;
use crate::error::{GateError, Result};
use crate::requirement::{self, Requirement, STEPS};
use std::fs;
use std::path::{Path, PathBuf};

/// 单个步骤的审核状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// 尚未审核（GATE 行缺 status 或为 pending）。
    Pending,
    /// 审核人已批准。
    Approved,
    /// 审核人已打回。
    Rejected,
}

impl StepState {
    /// 由 GATE 行原始值解析；未知值一律视为 `Pending`（fail-closed）。
    pub fn from_raw(raw: &str) -> StepState {
        match raw {
            "approved" => StepState::Approved,
            "rejected" => StepState::Rejected,
            _ => StepState::Pending,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StepState::Pending => "pending",
            StepState::Approved => "approved",
            StepState::Rejected => "rejected",
        }
    }

    /// 中文标签（三个前端共用，避免各写一套）。
    pub fn label(self) -> &'static str {
        match self {
            StepState::Pending => "待审核",
            StepState::Approved => "已通过",
            StepState::Rejected => "已打回",
        }
    }

    /// 是否已通过（解锁判定的唯一依据）。
    pub fn is_approved(self) -> bool {
        matches!(self, StepState::Approved)
    }
}

/// 一个步骤的状态快照。
#[derive(Debug, Clone)]
pub struct StepStatus {
    /// 步骤键：`decomposition` / `solution` / `testplan`。
    pub key: &'static str,
    /// 中文名：需求分解 / 技术方案 / 测试计划。
    pub label: &'static str,
    pub state: StepState,
    /// 审核人（未审核时为 `None`）。
    pub reviewer: Option<String>,
    /// 审核时间（未审核时为 `None`）。
    pub updated: Option<String>,
}

/// 清单整体状态（对应 `GATE:HEAD status=`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqState {
    /// 刚创建，尚未有任何审核动作。
    Draft,
    /// 审核进行中。
    InReview,
    /// 三段全部通过。
    Approved,
    /// 存在被打回的步骤，需 AI 修改后重审。
    ChangesRequested,
    /// 已归档（拦截脚本会跳过）。
    Done,
}

impl ReqState {
    pub fn from_raw(raw: &str) -> ReqState {
        match raw {
            "approved" => ReqState::Approved,
            "changes_requested" => ReqState::ChangesRequested,
            "in_review" => ReqState::InReview,
            "done" => ReqState::Done,
            _ => ReqState::Draft,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReqState::Draft => "draft",
            ReqState::InReview => "in_review",
            ReqState::Approved => "approved",
            ReqState::ChangesRequested => "changes_requested",
            ReqState::Done => "done",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReqState::Draft => "未开始",
            ReqState::InReview => "审核中",
            ReqState::Approved => "已通过",
            ReqState::ChangesRequested => "待修改",
            ReqState::Done => "已归档",
        }
    }
}

/// 一条需求的完整状态快照。
#[derive(Debug, Clone)]
pub struct ReqStatus {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub state: ReqState,
    /// 三段全部 `approved` → AI 可以开始编码。
    pub unlocked: bool,
    pub steps: Vec<StepStatus>,
    /// 未 resolve 的评论总数。
    pub open_comments: usize,
    /// 其中阻塞性的数量（> 0 即拦截编码）。
    pub blocking_comments: usize,
}

impl ReqStatus {
    /// 当前是否被门禁卡住（未解锁，或存在未解决的阻塞性评论）。
    pub fn is_blocked(&self) -> bool {
        !self.unlocked || self.blocking_comments > 0
    }

    /// 已通过的步骤数（进度展示用）。
    pub fn approved_count(&self) -> usize {
        self.steps.iter().filter(|s| s.state.is_approved()).count()
    }

    /// 下一步该审核的步骤键（前端据此高亮或置灰按钮）。
    ///
    /// 规则：第一个非 `approved` 的步骤；全通过则返回 `None`。
    pub fn next_step(&self) -> Option<&'static str> {
        self.steps
            .iter()
            .find(|s| !s.state.is_approved())
            .map(|s| s.key)
    }

    /// 指定步骤当前是否"允许被审核"（强制顺序下，前置步骤必须已通过）。
    pub fn can_review(&self, step: &str) -> bool {
        for s in &self.steps {
            if s.key == step {
                return true;
            }
            if !s.state.is_approved() {
                return false;
            }
        }
        false
    }
}

/// 读取单条需求的状态快照。
pub fn req_get(root: &Path, id: &str) -> Result<ReqStatus> {
    let r = requirement::find(root, id)?;
    let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
        path: Some(r.path.clone()),
        source: e,
    })?;
    build(root, &r, &content)
}

/// 列出全部需求的状态快照（按文件名字典序）。
pub fn req_list(root: &Path) -> Result<Vec<ReqStatus>> {
    let mut out = Vec::new();
    for r in requirement::list(root)? {
        let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
            path: Some(r.path.clone()),
            source: e,
        })?;
        out.push(build(root, &r, &content)?);
    }
    Ok(out)
}

/// 列出**归档区**全部需求的状态快照（`req-guard status --archived`）。
///
/// 归档区是只读历史，状态全为 [Done]；列出它是为了让人能按年找回已完结需求。
/// 列表里 hit 到历史文件时路径显示 archive/ 相对位置，便于人工翻阅。
pub fn req_list_archived(root: &Path) -> Result<Vec<ReqStatus>> {
    let mut out = Vec::new();
    for r in requirement::list_archived(root)? {
        let content = fs::read_to_string(&r.path).map_err(|e| GateError::Io {
            path: Some(r.path.clone()),
            source: e,
        })?;
        out.push(build(root, &r, &content)?);
    }
    Ok(out)
}

/// 由清单正文构建状态快照。
fn build(root: &Path, r: &Requirement, content: &str) -> Result<ReqStatus> {
    let steps: Vec<StepStatus> = STEPS
        .iter()
        .map(|(key, label)| {
            let state = StepState::from_raw(&requirement::step_status(content, key));
            StepStatus {
                key,
                label,
                state,
                reviewer: dash_to_none(requirement::step_reviewer(content, key)),
                updated: dash_to_none(requirement::step_updated(content, key)),
            }
        })
        .collect();

    let unlocked = steps.iter().all(|s| s.state.is_approved());
    let (open_comments, blocking_comments) = comment::summary(root, &r.id)?;

    Ok(ReqStatus {
        id: r.id.clone(),
        title: r.title.clone(),
        path: r.path.clone(),
        state: ReqState::from_raw(&requirement::head_status(content)),
        unlocked,
        steps,
        open_comments,
        blocking_comments,
    })
}

/// `-` / 空串 → `None`（GATE 行的空值约定）。
fn dash_to_none(v: String) -> Option<String> {
    if v.is_empty() || v == "-" {
        None
    } else {
        Some(v)
    }
}

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cleanup, temp_dir};

    fn setup(tag: &str) -> PathBuf {
        let root = temp_dir(tag);
        crate::requirement::create(&root, None, "状态快照").unwrap();
        root
    }

    fn approve_all(root: &Path) {
        for step in ["decomposition", "solution", "testplan"] {
            crate::requirement::review(root, "REQ-001", step, "寇工", true, "", true).unwrap();
        }
    }

    #[test]
    fn 状态映射_未知值一律按保守处理() {
        assert_eq!(StepState::from_raw("approved"), StepState::Approved);
        assert_eq!(StepState::from_raw("rejected"), StepState::Rejected);
        assert_eq!(StepState::from_raw(""), StepState::Pending);
        assert_eq!(StepState::from_raw("weird"), StepState::Pending);

        assert_eq!(ReqState::from_raw("in_review"), ReqState::InReview);
        assert_eq!(ReqState::from_raw("approved"), ReqState::Approved);
        assert_eq!(
            ReqState::from_raw("changes_requested"),
            ReqState::ChangesRequested
        );
        assert_eq!(ReqState::from_raw("done"), ReqState::Done);
        assert_eq!(ReqState::from_raw("nonsense"), ReqState::Draft);
    }

    #[test]
    fn 快照_未审核时的形状() {
        let root = setup("status-draft");
        let s = req_get(&root, "REQ-001").unwrap();
        assert_eq!(s.id, "REQ-001");
        assert_eq!(s.title, "状态快照");
        assert_eq!(s.state, ReqState::Draft);
        assert!(!s.unlocked);
        assert!(s.is_blocked());
        assert_eq!(s.approved_count(), 0);
        assert_eq!(s.next_step(), Some("decomposition"));
        assert!(s.can_review("decomposition"));
        assert!(!s.can_review("solution"), "前置未过时不得审后一步");
        assert_eq!((s.open_comments, s.blocking_comments), (0, 0));
        cleanup(&root);
    }

    #[test]
    fn 快照_三段通过后解锁() {
        let root = setup("status-approved");
        approve_all(&root);
        let s = req_get(&root, "REQ-001").unwrap();
        assert_eq!(s.state, ReqState::Approved);
        assert!(s.unlocked);
        assert!(!s.is_blocked());
        assert_eq!(s.approved_count(), 3);
        assert_eq!(s.next_step(), None);
        assert_eq!(s.steps[0].reviewer.as_deref(), Some("寇工"));
        assert!(s.steps[0].updated.is_some(), "审核时间应被解析出来");
        cleanup(&root);
    }

    #[test]
    fn 快照_阻塞评论使已解锁的需求仍被卡() {
        let root = setup("status-blocked");
        approve_all(&root);
        crate::comment::add(
            &root,
            "REQ-001",
            crate::comment::NewComment {
                step: Some("solution"),
                author: "寇工",
                text: "需补充回退步骤",
                quote: None,
                blocking: true,
                reply: None,
            },
        )
        .unwrap();

        let s = req_get(&root, "REQ-001").unwrap();
        assert!(s.unlocked, "三段本身已通过");
        assert!(s.is_blocked(), "阻塞评论未解决 → 仍应判定被卡");
        assert_eq!((s.open_comments, s.blocking_comments), (1, 1));
        cleanup(&root);
    }

    #[test]
    fn 列表与单条结果一致() {
        let root = setup("status-list");
        let all = req_list(&root).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, req_get(&root, "REQ-001").unwrap().id);
        cleanup(&root);
    }
}

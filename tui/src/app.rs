//! TUI 的状态机与事件循环。
//!
//! 所有动作都落到 `req-guard-core`：审核走 `requirement::review`、绕过走 `gate::bypass`、
//! 门禁判定走 `gate::gate_check`——与 CLI 完全等价，不存在"界面自己判一遍"。

use crate::ui;
use req_guard_core::error::{GateError, Result};
use req_guard_core::status::{self, ReqStatus};
use req_guard_core::{comment, gate, requirement};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 自动刷新间隔（轻量轮询，不做文件系统监听）。
const REFRESH: Duration = Duration::from_secs(3);
/// 事件轮询步长。
const TICK: Duration = Duration::from_millis(200);

/// 当前是否处于输入弹窗，以及弹窗要收集什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// 新建需求：输入标题。
    NewTitle,
    /// 批准当前段：输入审核人。
    ApproveReviewer,
    /// 打回当前段：输入审核人。
    RejectReviewer,
    /// 打回当前段：输入原因。
    RejectReason,
    /// 应急绕过：输入原因。
    BypassReason,
    /// 新增评论：输入作者。
    CommentAuthor,
    /// 新增评论：输入内容（`blocking` 决定是否阻塞编码）。
    CommentText { blocking: bool },
    /// 关闭（resolve）评论：输入审核人。
    ResolveAuthor,
}

impl Prompt {
    /// 弹窗标题（含对输入内容的说明）。
    pub fn title(self) -> &'static str {
        match self {
            Prompt::NewTitle => "新建需求 — 输入标题（Enter 确认 / Esc 取消）",
            Prompt::ApproveReviewer => "批准当前段 — 输入审核人",
            Prompt::RejectReviewer => "打回当前段 — 输入审核人",
            Prompt::RejectReason => "打回当前段 — 输入原因（必填）",
            Prompt::BypassReason => "应急绕过 — 输入原因（必填，默认 60 分钟）",
            Prompt::CommentAuthor => "新增评论 — 输入作者（人类姓名；AI 只能回复，不能新开）",
            Prompt::CommentText { blocking } => {
                if blocking {
                    "新增**阻塞**评论 — 输入内容（必填，未关闭即拦截 AI 编码）"
                } else {
                    "新增普通评论 — 输入内容（必填）"
                }
            }
            Prompt::ResolveAuthor => "关闭（resolve）评论 — 输入审核人（只能人类）",
        }
    }
}

/// 覆盖层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Audit,
    /// 评论面板（唯一的**可交互**覆盖层：其余覆盖层任意键即关）。
    Comments,
}

/// 焦点环的顺序（也是界面上面板的排布顺序）：
/// 需求列表（左）→ 三段（右上）→ 正文（右下）。
///
/// `↑↓` 作用于**当前聚焦的面板**；`←→`/`Tab` 在环上移动焦点。
/// 之所以要有焦点概念：需求列表与三段都是**纵向**列表，`↑↓` 必须只服务其中一个，
/// 否则"按 ↓ 到底该动谁"无法自洽。
const FOCUS_RING: [Focus; 3] = [Focus::Requirements, Focus::Steps, Focus::Body];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Requirements,
    Steps,
    Body,
}

/// TUI 全局状态。
pub struct App {
    pub focus: Focus,
    pub root: PathBuf,
    pub reqs: Vec<ReqStatus>,
    /// 选中的需求下标。
    pub selected: usize,
    /// 选中的步骤下标（0..3）。
    pub step: usize,
    /// 当前段的正文（只读展示，只含选中那一段）。
    pub body: Vec<String>,
    pub body_scroll: u16,
    pub overlay: Overlay,
    /// 审计日志行（打开 `L` 时加载）。
    pub audit: Vec<String>,
    /// 当前需求的评论（打开 `m` 时经 core 读入）。
    pub comments: Vec<comment::Comment>,
    /// 评论面板里的选中项与滚动位置。
    pub comment_sel: usize,
    pub comment_scroll: u16,
    /// 新增评论是否阻塞（由 `n` / `N` 决定，跨两个输入弹窗传递）。
    pending_blocking: bool,
    /// 进行中的弹窗与输入缓冲。
    pub prompt: Option<Prompt>,
    pub input: String,
    /// 上次操作的结论（底部提示栏）。
    pub message: Option<String>,
    /// 打回流程的中间态：已输入的审核人。
    pending_reviewer: String,
    pub should_quit: bool,
    last_refresh: Instant,
}

impl App {
    pub fn new(root: &Path) -> App {
        let mut app = App {
            // 默认聚焦"三段"：审核是这个界面的主任务，进来按 ↑↓ 就该能换段
            focus: Focus::Steps,
            root: root.to_path_buf(),
            reqs: Vec::new(),
            selected: 0,
            step: 0,
            body: Vec::new(),
            body_scroll: 0,
            overlay: Overlay::None,
            audit: Vec::new(),
            comments: Vec::new(),
            comment_sel: 0,
            comment_scroll: 0,
            pending_blocking: false,
            prompt: None,
            input: String::new(),
            message: None,
            pending_reviewer: String::new(),
            should_quit: false,
            last_refresh: Instant::now(),
        };
        app.reload();
        app
    }

    /// 当前选中的需求（列表为空时为 `None`）。
    pub fn current(&self) -> Option<&ReqStatus> {
        self.reqs.get(self.selected)
    }

    /// 当前选中的步骤键。
    pub fn current_step(&self) -> Option<&'static str> {
        let r = self.current()?;
        r.steps.get(self.step).map(|s| s.key)
    }

    /// 重新读取需求列表与正文。
    pub fn reload(&mut self) {
        let previous = self.current().map(|r| r.path.clone());
        let scroll = self.body_scroll;
        match status::req_list(&self.root) {
            Ok(list) => {
                self.reqs = list;
                if let Some(index) = self
                    .reqs
                    .iter()
                    .position(|r| Some(&r.path) == previous.as_ref())
                {
                    self.selected = index;
                }
                if self.selected >= self.reqs.len() {
                    self.selected = self.reqs.len().saturating_sub(1);
                }
            }
            Err(e) => self.message = Some(format!("读取需求失败：{}", e)),
        }
        let unchanged = self.current().map(|r| &r.path) == previous.as_ref();
        if !unchanged {
            self.step = 0;
        }
        self.load_body();
        if unchanged {
            self.body_scroll = scroll;
        }
        // 评论面板开着时同步刷新：AI 在另一个终端回复了评论，3s 内就该看到。
        if self.overlay == Overlay::Comments {
            self.reload_comments();
        }
        self.last_refresh = Instant::now();
    }

    /// 加载当前需求正文（只读）。切段时直接在本段起始处打开正文。
    ///
    /// 切段规则由 core 的 [`requirement::section_of`] 提供（与 GUI 同一条规则）：
    /// 界面只显示**选中那一段**，而不是整篇清单——否则审核人得自己在全文里找对应段，
    /// 极易"看错段点错批准"。
    pub fn load_body(&mut self) {
        self.body.clear();
        self.body_scroll = 0;
        let Some(r) = self.current() else { return };
        match std::fs::read_to_string(&r.path) {
            Ok(text) => {
                self.body = requirement::section_of(&text, self.step)
                    .lines()
                    .map(|l| l.to_string())
                    .collect();
            }
            Err(e) => {
                self.body = vec![format!("读取正文失败：{}", e)];
            }
        }
        self.body_scroll = self
            .body
            .iter()
            .position(|l| !l.trim().is_empty() && !l.starts_with("<!--"))
            .map_or(0, |i| u16::try_from(i).unwrap_or(0));
    }

    /// 时间到了就自动刷新（3s 轻量轮询）。
    fn maybe_auto_refresh(&mut self) {
        if self.last_refresh.elapsed() >= REFRESH {
            self.reload();
        }
    }

    /// 处理按键。
    pub fn on_key(&mut self, key: KeyEvent) {
        // 只响应按下事件，避免 Windows 上的 Release/Repeat 造成双触发。
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }
        if self.overlay == Overlay::Comments {
            self.on_comments_key(key);
            return;
        }
        if self.overlay != Overlay::None {
            // 其余覆盖层是只读的，任意键关闭。
            self.overlay = Overlay::None;
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char('L') => {
                self.audit = gate::audit_tail(&self.root, 200).unwrap_or_default();
                self.overlay = Overlay::Audit;
            }
            KeyCode::Char('R') => {
                self.reload();
                self.message = Some("已刷新".into());
            }
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Requirements => self.move_selection(-1),
                Focus::Steps => self.move_step(-1),
                Focus::Body => {
                    self.body_scroll = self.body_scroll.saturating_sub(1);
                    self.clamp_scroll();
                }
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Requirements => self.move_selection(1),
                Focus::Steps => self.move_step(1),
                Focus::Body => {
                    self.body_scroll = self.body_scroll.saturating_add(1);
                    self.clamp_scroll();
                }
            },
            // ←→/Tab 只负责**移动焦点**，不再直接切段：三段是纵向列表，切段归 ↑↓
            // （历史缺陷：焦点状态机从未被赋值，导致 ↑↓ 只动需求列表、←→ 反而在切段）
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.cycle_focus(true),
            KeyCode::PageDown | KeyCode::Char('J') => {
                self.body_scroll = self.body_scroll.saturating_add(10)
            }
            KeyCode::PageUp | KeyCode::Char('K') => {
                self.body_scroll = self.body_scroll.saturating_sub(10)
            }
            KeyCode::Char('a') => self.begin_approve(),
            KeyCode::Char('r') => self.begin_reject(),
            KeyCode::Char('n') => self.open_prompt(Prompt::NewTitle),
            KeyCode::Char('g') => self.run_check(),
            KeyCode::Char('b') => self.open_prompt(Prompt::BypassReason),
            // 审核人最高频的动作之一就是"留一条意见"，必须一步可达。
            KeyCode::Char('m') => self.open_comments(),
            _ => {}
        }
    }

    /// 在 [`FOCUS_RING`] 上移动焦点（`←→` / `Tab` / `Shift+Tab`），到端点回绕。
    fn cycle_focus(&mut self, forward: bool) {
        let n = FOCUS_RING.len();
        let i = FOCUS_RING
            .iter()
            .position(|f| *f == self.focus)
            .unwrap_or(0);
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.focus = FOCUS_RING[next];
    }

    fn move_selection(&mut self, delta: isize) {
        if self.reqs.is_empty() {
            return;
        }
        let len = self.reqs.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1);
        if next as usize != self.selected {
            self.selected = next as usize;
            self.step = 0;
            self.load_body();
            self.comments.clear();
            self.comment_sel = 0;
            self.comment_scroll = 0;
        }
    }

    fn move_step(&mut self, delta: isize) {
        if self.current().is_none() {
            return;
        }
        let len = requirement::STEPS.len() as isize;
        let next = (self.step as isize + delta).clamp(0, len - 1);
        if next as usize != self.step {
            self.step = next as usize;
            self.load_body();
        }
    }

    fn clamp_scroll(&mut self) {
        let max = self.body.len().saturating_sub(1).min(u16::MAX as usize) as u16;
        self.body_scroll = self.body_scroll.min(max);
    }

    fn open_prompt(&mut self, p: Prompt) {
        self.prompt = Some(p);
        self.input.clear();
        self.message = None;
    }

    fn begin_approve(&mut self) {
        let Some(r) = self.current() else {
            self.message = Some("没有可审核的需求，先按 n 新建".into());
            return;
        };
        let Some(step) = self.current_step() else {
            return;
        };
        // 强制顺序：前置未过时直接给出提示，而不是等 core 报错
        if !r.can_review(step) {
            self.message = Some(format!(
                "顺序不满足：请先批准更早的步骤（当前段 {} 为待审）",
                step
            ));
            return;
        }
        self.open_prompt(Prompt::ApproveReviewer);
    }

    fn begin_reject(&mut self) {
        if self.current().is_none() {
            self.message = Some("没有可审核的需求，先按 n 新建".into());
            return;
        }
        self.open_prompt(Prompt::RejectReviewer);
    }

    /// 打开评论面板（读当前需求的评论）。
    pub fn open_comments(&mut self) {
        if self.current().is_none() {
            self.message = Some("没有可评论的需求，先按 n 新建".into());
            return;
        }
        self.reload_comments();
        self.overlay = Overlay::Comments;
    }

    /// 重新读评论，并把选中项夹回合法范围。
    pub fn reload_comments(&mut self) {
        self.comments = match self.current() {
            Some(r) => comment::list(&self.root, &r.id).unwrap_or_default(),
            None => Vec::new(),
        };
        if self.comment_sel >= self.comments.len() {
            self.comment_sel = self.comments.len().saturating_sub(1);
        }
        self.clamp_comment_scroll();
    }

    /// 新增一条评论（不改步骤状态——"审核人只评论、不改正文"）。
    ///
    /// 锚定**当前选中段**：界面上无法表达"不锚定任何段"的总评，而"这句话在说哪一段"
    /// 恰恰是审核意见最要紧的信息（CLI 侧另支持 `--quote` 做行号锚定）。
    pub fn add_comment(&mut self, blocking: bool) {
        let Some(r) = self.current() else {
            self.message = Some("没有可评论的需求".into());
            return;
        };
        let id = r.id.clone();
        let author = self.pending_reviewer.clone();
        if author.trim().is_empty() {
            self.message = Some("评论作者不能为空".into());
            return;
        }
        let text = self.input.trim().to_string();
        if text.is_empty() {
            self.message = Some("评论内容不能为空".into());
            return;
        }
        let step = self.current_step();
        match comment::add(
            &self.root,
            &id,
            comment::NewComment {
                step,
                author: &author,
                text: &text,
                quote: None,
                blocking,
                reply: None,
            },
        ) {
            Ok(c) => {
                self.prompt = None;
                self.input.clear();
                self.pending_reviewer.clear();
                self.reload();
                self.reload_comments();
                if let Some(i) = self.comments.iter().position(|x| x.id == c.id) {
                    self.comment_sel = i;
                    // 新加的多半在末尾：直接把它滚进视野。
                    self.move_comment(0);
                }
                self.message = Some(format!(
                    "已添加{}评论 {}",
                    if blocking { "阻塞" } else { "普通" },
                    c.id
                ));
            }
            Err(e) => self.message = Some(format!("添加评论失败：{}", e)),
        }
    }

    /// 关闭（resolve）选中的评论。
    ///
    /// 走"界面进程内签发凭据"：`comment::resolve` 内含
    /// `auth::ensure_human(.., ScopeCheck::Exact("resolve:<需求ID>"))`，
    /// L3 下必须是范围票据，签发范围要逐字对上。
    pub fn resolve_comment(&mut self) {
        let Some(r) = self.current() else {
            self.message = Some("没有可操作的需求".into());
            return;
        };
        let id = r.id.clone();
        let Some(c) = self.comments.get(self.comment_sel).cloned() else {
            self.message = Some("没有可关闭的评论".into());
            return;
        };
        if c.state == comment::CommentState::Resolved {
            self.message = Some(format!("{} 已是关闭状态", c.id));
            return;
        }
        let reviewer = self.pending_reviewer.clone();
        if reviewer.trim().is_empty() {
            self.message = Some("关闭人不能为空（关闭权归审核人）".into());
            return;
        }
        if !self.prepare_credential(&format!("resolve:{}", id)) {
            return;
        }
        let outcome = comment::resolve(&self.root, &id, &c.id, &reviewer);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(_) => {
                self.prompt = None;
                self.input.clear();
                self.pending_reviewer.clear();
                self.reload();
                self.reload_comments();
                self.message = Some(format!("已关闭 {}（解除阻塞）", c.id));
            }
            Err(e) => self.message = Some(format!("关闭失败：{}", e)),
        }
    }

    /// 正文被 AI 改过之后重算行号锚点（命中的刷新行号，找不到原文的标 `stale`）。
    pub fn refresh_comment_anchors(&mut self) {
        let Some(r) = self.current() else {
            self.message = Some("没有可操作的需求".into());
            return;
        };
        let id = r.id.clone();
        match comment::refresh_anchors(&self.root, &id) {
            Ok(0) => {
                self.reload_comments();
                self.message = Some("行号锚点已重算：全部命中".into());
            }
            Ok(stale) => {
                self.reload_comments();
                self.message = Some(format!(
                    "行号锚点已重算：{} 条找不到原文（已标 stale）",
                    stale
                ));
            }
            Err(e) => self.message = Some(format!("重算锚点失败：{}", e)),
        }
    }

    /// 评论面板内的按键（唯一可交互的覆盖层，故在此分流而不是"任意键关闭"）。
    fn on_comments_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') => {
                self.overlay = Overlay::None;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_comment(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_comment(1),
            KeyCode::PageUp => {
                self.comment_scroll = self.comment_scroll.saturating_sub(8);
                self.clamp_comment_scroll();
            }
            KeyCode::PageDown => {
                self.comment_scroll = self.comment_scroll.saturating_add(8);
                self.clamp_comment_scroll();
            }
            // n 普通评论 / N 阻塞评论：区分大小写是因为"阻塞与否"后果完全不同，
            // 不值得为它再加一层选择弹窗（且误点的代价是门禁直接被拦住）。
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let blocking = key.code == KeyCode::Char('N');
                if self.current().is_none() {
                    self.message = Some("没有可评论的需求".into());
                    return;
                }
                self.pending_blocking = blocking;
                self.open_prompt(Prompt::CommentAuthor);
            }
            KeyCode::Char('x') => {
                if self.comments.is_empty() {
                    self.message = Some("还没有评论可关闭".into());
                    return;
                }
                self.open_prompt(Prompt::ResolveAuthor);
            }
            KeyCode::Char('A') => self.refresh_comment_anchors(),
            _ => {}
        }
    }

    fn move_comment(&mut self, delta: isize) {
        if self.comments.is_empty() {
            return;
        }
        let len = self.comments.len() as isize;
        self.comment_sel = (self.comment_sel as isize + delta).clamp(0, len - 1) as usize;
        // **最小幅度**滚动：只有选中项滚出可视范围才动（每条评论占 2 行）。
        // 早先把滚动位置直接钉到选中项，短列表反而会把表头与前几条评论顶出视野。
        let top = comment_top_line(self.comment_sel) as usize;
        let bottom = top + COMMENT_LINES;
        if top < self.comment_scroll as usize {
            self.comment_scroll = top as u16;
        } else if bottom > self.comment_scroll as usize + COMMENT_PAGE_LINES {
            self.comment_scroll = (bottom - COMMENT_PAGE_LINES) as u16;
        }
    }

    fn clamp_comment_scroll(&mut self) {
        let max = comment_total_lines(self.comments.len()).saturating_sub(COMMENT_PAGE_LINES);
        self.comment_scroll = self.comment_scroll.min(max as u16);
    }

    fn run_check(&mut self) {
        match gate::gate_check(&self.root) {
            Ok(v) => {
                self.message = Some(match v {
                    // 绕过放行与正常放行必须一眼分得开：两者的事后责任完全不同。
                    gate::GateVerdict::Pass { bypassed, .. } => {
                        if bypassed {
                            "门禁检查：放行 ⚠️（命中应急绕过窗口，非三段批准）".to_string()
                        } else {
                            "门禁检查：放行 ✅".to_string()
                        }
                    }
                    gate::GateVerdict::Block { detail, .. } => format!(
                        "门禁检查：拦截 ⛔（{}）",
                        detail.first().cloned().unwrap_or_default()
                    ),
                })
            }
            Err(e) => self.message = Some(format!("门禁检查失败：{}", e)),
        }
    }

    /// 弹窗模式下的按键。
    fn on_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.pending_reviewer.clear();
                self.input.clear();
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Enter => self.submit_prompt(),
            _ => {}
        }
    }

    /// 界面进程内签发凭据（见 [`req_guard_core::auth::ui_issue_credential`]）。
    ///
    /// 返回 `false` 表示签发失败（此时 `message` 已写明原因，调用方应放弃本次动作）。
    /// L0 下该调用是无操作且返回 `true`——保持"未开加固的项目照旧可用"。
    fn prepare_credential(&mut self, scope: &str) -> bool {
        match req_guard_core::auth::ui_issue_credential(&self.root, scope) {
            Ok(_) => true,
            Err(e) => {
                self.message = Some(format!("签发界面凭据失败：{}", e));
                false
            }
        }
    }

    /// 提交弹窗（按类型分派到 core）。
    fn submit_prompt(&mut self) {
        let Some(p) = self.prompt else { return };
        let value = self.input.trim().to_string();
        match p {
            Prompt::NewTitle => {
                if value.is_empty() {
                    self.message = Some("标题不能为空".into());
                    return;
                }
                match requirement::create(&self.root, None, &value) {
                    Ok(r) => {
                        self.prompt = None;
                        self.input.clear();
                        self.reload();
                        self.message = Some(format!("已创建 {}", r.id));
                    }
                    Err(e) => self.message = Some(format!("创建失败：{}", e)),
                }
            }
            Prompt::ApproveReviewer => {
                if value.is_empty() {
                    self.message = Some("审核人不能为空".into());
                    return;
                }
                self.pending_reviewer = value;
                self.review(false);
            }
            Prompt::RejectReviewer => {
                if value.is_empty() {
                    self.message = Some("审核人不能为空".into());
                    return;
                }
                // 打回分两步：先审核人，再原因（原因必填，会写进清单与评论）。
                self.pending_reviewer = value;
                self.input.clear();
                self.prompt = Some(Prompt::RejectReason);
            }
            Prompt::RejectReason => {
                if value.is_empty() {
                    self.message = Some("打回必须填写原因".into());
                    return;
                }
                self.input = value;
                self.review(true);
            }
            Prompt::CommentAuthor => {
                if value.is_empty() {
                    self.message = Some("评论作者不能为空".into());
                    return;
                }
                self.pending_reviewer = value;
                self.input.clear();
                self.prompt = Some(Prompt::CommentText {
                    blocking: self.pending_blocking,
                });
            }
            Prompt::CommentText { blocking } => {
                if value.is_empty() {
                    self.message = Some("评论内容不能为空".into());
                    return;
                }
                self.input = value;
                self.add_comment(blocking);
            }
            Prompt::ResolveAuthor => {
                if value.is_empty() {
                    self.message = Some("关闭人不能为空（关闭权归审核人）".into());
                    return;
                }
                self.pending_reviewer = value;
                self.resolve_comment();
            }
            Prompt::BypassReason => {
                if value.is_empty() {
                    self.message = Some("应急绕过必须填写原因".into());
                    return;
                }
                let actor = "tui";
                // 界面进程内签发凭据（以"人类亲手操作界面"为在场证明，见 core auth）。
                if !self.prepare_credential("bypass") {
                    return;
                }
                let outcome = gate::bypass(&self.root, &value, actor, 60);
                req_guard_core::auth::clear_credential();
                match outcome {
                    Ok(_) => {
                        self.prompt = None;
                        self.input.clear();
                        self.message = Some("已开启应急绕过 60 分钟（已记审计）".into());
                    }
                    Err(e) => self.message = Some(format!("绕过失败：{}", e)),
                }
            }
        }
    }

    /// 执行批准/打回。
    fn review(&mut self, reject: bool) {
        // 先把需要的值取出来，结束对 self 的借用，后续才能改 self.input / self.prompt。
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => return,
        };
        let reviewer = self.pending_reviewer.clone();
        let reason = if reject {
            self.input.trim().to_string()
        } else {
            String::new()
        };
        let strict = gate::strict_order(&self.root);
        // 界面进程内签发凭据：以"人类亲手操作界面"为在场证明，全程不落 stdout/环境变量
        // （见 core auth::ui_issue_credential）。L0 时为无操作，保持旧行为。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome =
            requirement::review(&self.root, &id, step, &reviewer, !reject, &reason, strict);
        req_guard_core::auth::clear_credential();

        match outcome {
            Ok(_) => {
                // 打回时把原因同时留成评论，保证 AI 能看到"为什么被打回"。
                if reject {
                    let _ = comment::add(
                        &self.root,
                        &id,
                        comment::NewComment {
                            step: Some(step),
                            author: &reviewer,
                            text: &reason,
                            quote: None,
                            blocking: true,
                            reply: None,
                        },
                    );
                }
                self.prompt = None;
                self.input.clear();
                self.pending_reviewer.clear();
                self.reload();
                self.message = Some(format!(
                    "{} {} / {}",
                    if reject { "已打回" } else { "已批准" },
                    id,
                    requirement::step_label(step)
                ));
            }
            Err(e) => self.message = Some(format!("操作失败：{}", e)),
        }
    }
}

/// 弹窗里能放多少行评论（保守取值：弹窗 24 行 - 边框 2 - 头部 2 - 空行 1 - 键位 2）。
/// 用于"选中项滚出可视范围时才滚动"，值偏小只会让翻页早一点，不影响正确性。
const COMMENT_PAGE_LINES: usize = 17;

/// 一条评论在面板里占的行数（1 行头 + 1 行正文）。
///
/// 面板**不折行**（正文超长直接截断），所以"第 i 条评论的起始行"就是确定的——
/// 滚动位置才能用一个整数算准，折行的话这段算术会全废。
const COMMENT_LINES: usize = 2;

/// 第 `i` 条评论的首行行号。
fn comment_top_line(i: usize) -> u16 {
    u16::try_from(i * COMMENT_LINES).unwrap_or(u16::MAX)
}

/// `n` 条评论共占多少行。
fn comment_total_lines(n: usize) -> usize {
    n * COMMENT_LINES
}

/// 初始化终端并进入事件循环。
pub fn run(root: &Path) -> Result<()> {
    install_panic_hook();
    let mut terminal = init_terminal()?;
    let result = event_loop(&mut terminal, root);
    // 无论成败都要恢复终端。
    let restore = restore_terminal(&mut terminal);
    result.and(restore)
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().map_err(|e| crate::term_err("进入原始模式", e))?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| crate::term_err("切换备用屏", e))?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(|e| GateError::Validation(format!("创建终端失败：{}", e)))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().map_err(|e| crate::term_err("退出原始模式", e))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| crate::term_err("退出备用屏", e))?;
    terminal
        .show_cursor()
        .map_err(|e| crate::term_err("恢复光标", e))
}

/// panic 时也要把终端还原，否则用户会面对一个乱码回显的 shell。
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, root: &Path) -> Result<()> {
    let mut app = App::new(root);
    loop {
        terminal
            .draw(|f| ui::render(f, &app))
            .map_err(|e| GateError::Validation(format!("绘制失败：{}", e)))?;

        if event::poll(TICK).map_err(|e| GateError::Validation(format!("事件轮询失败：{}", e)))?
        {
            match event::read()
                .map_err(|e| GateError::Validation(format!("读取事件失败：{}", e)))?
            {
                Event::Key(key) => app.on_key(key),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        app.maybe_auto_refresh();
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

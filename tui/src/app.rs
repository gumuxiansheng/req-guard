//! TUI 的状态机与事件循环。
//!
//! 所有动作都落到 `req-guard-core`：审核走 `requirement::review`、绕过走 `gate::bypass`、
//! 门禁判定走 `gate::gate_check_with`（带当前需求作消歧提示，见 [`App::run_check`]）——
//! 与 CLI 完全等价，不存在"界面自己判一遍"。

use crate::ui;
use req_guard_core::error::{GateError, Result};
use req_guard_core::status::{self, ReqStatus};
use req_guard_core::{comment, gate, requirement, resolve, touch};

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

/// 四类只读体检的结果（REQ-016 G1/G3/G5/G6）。
///
/// 与 GUI 侧的 `HealthReport` 同构但是**两份类型**：它住在各前端自己的 crate 里
/// （core 不提供这个形状，本需求也不许新增 core API）。两端的差别只在渲染方式，
/// 判定全部来自 core 的同几个函数——**没有第二个真相**。
#[derive(Debug, Default, Clone)]
pub struct HealthReport {
    pub touch: Vec<touch::TouchIssue>,
    pub sums: Vec<(String, req_guard_core::requirement::SumIssue)>,
    pub ac: Vec<req_guard_core::ac::AcIssue>,
    pub cross: Vec<touch::CrossRefIssue>,
    /// 第 2 段定位是否失败——失败时**不给**任何具体引用的结论。
    pub cross_locate_failed: bool,
}

/// 体检的四组（顺序即界面上的子标签顺序）。
pub const HEALTH_GROUPS: [&str; 4] = ["① 变更范围", "② 内容一致性", "③ 验收标准", "④ 交叉引用"];

/// 「哪些浮层是**分组式**的、各有几个子标签」的登记表。
///
/// 存在的理由是一条容易被后续需求破坏的不变量：分组式信息（体检四组、卫生三组）
/// 应当收在**一个**浮层里用数字键切换，而不是拆成 N 个浮层——拆开以后
/// 每多一类就要多记一个按键，而它们的用法完全同构。
///
/// 单测断言的是这张表里的**分组数**，而不是"overlay 枚举总数"：
/// 后者会被每个新需求改动，于是那条测试退化成例行公事。
pub fn overlay_tab_count() -> std::collections::BTreeMap<Overlay, usize> {
    let mut m = std::collections::BTreeMap::new();
    m.insert(Overlay::Health, 4); // REQ-016：变更范围 / 内容一致性 / 验收标准 / 交叉引用
    m.insert(Overlay::Hygiene, 3); // REQ-017：编号冲突 / 门禁自检 / 环境
    m
}

/// 工程卫生面板的结果（REQ-017）：编号冲突 + 门禁自检。
///
/// 自检用 `Option`：它要实跑脚本，可能失败——失败时如实报，不伪装成"没问题"。
#[derive(Debug, Default, Clone)]
pub struct HygieneReport {
    pub id_issues: Vec<req_guard_core::idcheck::IdIssue>,
    /// `(硬伤数, 告警数, 全部文本)`。
    pub selfcheck: Option<(usize, usize, Vec<String>)>,
}

/// 返工率的展示阈值（与 GUI 同口径）。
pub const REWORK_REVIEW_THRESHOLD: usize = 3;

/// 归档动作作用在哪儿：单份 / 到期清扫（与 `dry_run` 正交）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveWhich {
    One(String),
    Due,
}

/// 当前是否处于输入弹窗，以及弹窗要收集什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// 新建需求第一步：可选的自定义编号（**直接回车 = 自动编号**）。
    ///
    /// ⚠️ 这是本需求唯一的**交互回归**：`n` 从"一步输标题"变成两步。
    /// 缓解：这一步的标题文案写明可以留空，且 `Esc` 可一步取消。
    NewId,
    /// 新建需求第二步：输入标题。
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
    /// 修订（amend）当前段：输入提请人。
    AmendReviewer,
    /// 修订（amend）当前段：输入改稿说明（必填）。
    AmendReason,
    /// 绑定内容摘要（seal）：输入原因（首次补绑定可留空）。
    SealReason,
    /// 归档（done）：输入操作人（REQ-015 G1）。
    DoneActor,
    /// 物理归档：输入操作人（REQ-015 G2；同一人同时用于预演与执行）。
    ArchiveActor,
    /// 追加变更范围声明（REQ-016 G2）：输入路径 / glob。
    DeclareGlob,
    /// 追加变更范围声明：输入原因。
    DeclareReason,
}

impl Prompt {
    /// 弹窗标题（含对输入内容的说明）。
    pub fn title(self) -> &'static str {
        match self {
            Prompt::NewId => "新建需求 ①/② — 自定义编号（直接回车 = 自动编号 / Esc 取消）",
            Prompt::NewTitle => "新建需求 ②/② — 输入标题（Enter 确认 / Esc 取消）",
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
            Prompt::AmendReviewer => "请求修订当前段 — 输入提请人",
            Prompt::AmendReason => "请求修订 — 输入改稿说明（必填，会写进清单与台账）",
            Prompt::SealReason => "绑定内容摘要 — 输入原因（首次补绑定可留空；已绑定过再封必填）",
            // 文案里必须带「归档」而不是「完成」：done = 移出门禁管辖，
            // **不是**交付完成（REQ-015 N1）。写成「完成」用户会理解错。
            Prompt::DoneActor => "归档（done）当前需求 — 输入操作人（≠ 交付完成）",
            Prompt::ArchiveActor => "物理归档 — 输入操作人（预演 / 执行 / 清理到期）",
            Prompt::DeclareGlob => {
                "追加变更范围声明 — 输入路径 / glob（提交会打回技术方案并清空摘要）"
            }
            Prompt::DeclareReason => "追加变更范围声明 — 输入原因（记入台账）",
        }
    }
}

/// 覆盖层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Overlay {
    None,
    Help,
    Audit,
    /// 门禁检查结果（REQ-014 G4）。
    ///
    /// 为什么是浮层而不是 footer：footer 固定 4 行（`ui.rs` 外层布局的
    /// `Constraint::Length(4)`，去掉上下边框只剩 2 行可用），而多需求仓库里
    /// `Ambiguous` 会产出「候选清单」十几行——任何固定高度的 footer 都装不下，
    /// 硬塞的结果是截断掉用户最需要看的处置建议。这里放浮层可滚动，
    /// footer 只留一行摘要。
    Check,
    /// 评论面板（唯一的**可交互**覆盖层：其余覆盖层任意键即关）。
    Comments,
    /// 归档确认（REQ-015 G4 的第一次 `d`）。
    ///
    /// 独立一个浮层而不是复用 [`Overlay::Check`]：Check 的按键语义是
    /// 「↑↓ 滚动、任意键关闭」，而这里需要「再按一次 d 才继续」——
    /// 复用会把第二次 `d` 当成"关闭浮层"吃掉，用户按了第二次却什么都没发生。
    DoneConfirm,
    /// 体检浮层（REQ-016 G1/G3/G5/G6）。
    ///
    /// 四类体检收在**一个**浮层里、用 `1`–`4` 切子标签，而不是四个独立 overlay：
    /// overlay 每多一个，审核人要记的按键就多一个，而它们的用法完全同构。
    Health,
    /// 工程卫生浮层（REQ-017）：编号冲突 / 门禁自检 / 环境。
    ///
    /// 与 [`Overlay::Health`] 分开而不是塞进去加第 5 个标签：两者的信息频次不同
    /// （体检是每次改动都看，卫生是偶尔看一眼），混在一起会让体检的 4 个标签
    /// 变成 6 个，选择成本上升。
    Hygiene,
    /// 归档区只读视图（REQ-015 G3/T8）：列出 `status::req_list_archived`，
    /// 明确标注只读并挡掉一切写动作。
    Archived,
    /// 归档操作的结果（`src → dst` 列表；REQ-015 G2 的「预演」出口）。
    Archive,
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
    /// 上一次门禁检查的完整明细（`Overlay::Check` 渲染，逐行可读）。
    pub check_detail: Vec<String>,
    /// 上一次门禁检查的口径说明（REQ-014 G3：必须显示，否则分不清
    /// 「忘了 `git add`」与「需求本身有问题」）。
    pub check_scope: String,
    /// 上一次拦截的类别名（放行时为空串）。
    pub check_kind: String,
    /// 检查结果浮层的滚动位置。
    pub check_scroll: u16,
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
    /// 归档确认：是否已知悉「阻塞评论会失效」（REQ-015 G4 的勾选门禁）。
    ///
    /// TUI 没有勾选框，用 `d` 连按两次实现同一道关：第一次亮出三项事实，
    /// 第二次才是确认。**只在确有未关闭阻塞评论时**才要求第二次
    /// （没有阻塞评论还要求一次无意义的确认，只会训练用户闭眼按）。
    pub ack_blocking: bool,
    /// 归档区只读视图：是否正处于历史浏览态。
    pub browsing_archived: bool,
    /// 归档区清单（打开时读一次并缓存，不进 3 秒轮询）。
    pub archived: Vec<ReqStatus>,
    /// 归档区视图的选中项与滚动位置。
    pub archived_sel: usize,
    /// 归档操作的结果（`源路径 → 归档路径`）。
    pub archive_plan: Vec<(String, String)>,
    /// 归档结果浮层的滚动位置。
    pub archive_scroll: u16,
    /// 顶栏身份区文本（姓名 <邮箱> sig=… · L<等级>）。
    pub who: String,
    /// 体检结果缓存（`None` = 还没算过；打开浮层时算一次，不进轮询）。
    pub health: Option<HealthReport>,
    /// 体检浮层当前子标签（0..4）。
    pub health_tab: usize,
    /// 声明流程的中间态：已输入的 glob。
    pending_glob: String,
    /// 凭据签发 / 清除的调用次数（AC-010 要证只读体检为 0）。
    pub cred_issued: usize,
    pub cred_cleared: usize,
    /// 新建流程的中间态：已输入的自定义编号（空 = 自动编号）。
    pending_new_id: String,
    /// 工程卫生面板的缓存（编号冲突 + 自检；打开时算一次，不进轮询）。
    pub hygiene: Option<HygieneReport>,
    /// 卫生浮层的当前分组（0 = 编号冲突，1 = 自检，2 = 环境）。
    pub hygiene_tab: usize,
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
            check_detail: Vec::new(),
            check_scope: String::new(),
            check_kind: String::new(),
            check_scroll: 0,
            comments: Vec::new(),
            comment_sel: 0,
            comment_scroll: 0,
            pending_blocking: false,
            prompt: None,
            input: String::new(),
            message: None,
            pending_reviewer: String::new(),
            ack_blocking: false,
            browsing_archived: false,
            archived: Vec::new(),
            archived_sel: 0,
            archive_plan: Vec::new(),
            archive_scroll: 0,
            who: String::new(),
            health: None,
            health_tab: 0,
            pending_glob: String::new(),
            cred_issued: 0,
            cred_cleared: 0,
            pending_new_id: String::new(),
            hygiene: None,
            hygiene_tab: 0,
            should_quit: false,
            last_refresh: Instant::now(),
        };
        app.reload();
        app.refresh_who();
        app
    }

    /// 当前选中的需求（列表为空时为 `None`）。
    ///
    /// 归档区视图下语义变为「选中的历史清单」，于是正文渲染一行都不用改。
    pub fn current(&self) -> Option<&ReqStatus> {
        if self.browsing_archived {
            self.archived.get(self.archived_sel)
        } else {
            self.reqs.get(self.selected)
        }
    }

    /// 当前是否只读（归档区视图）。判定只看这个标志位——
    /// 它来自 core 给的 `state == Done`，前端不另判路径（REQ-015 关键设计 7）。
    pub fn read_only(&self) -> bool {
        self.browsing_archived
    }

    /// 只读视图下挡住写动作的统一入口。
    fn deny_when_read_only(&mut self) -> bool {
        if self.read_only() {
            self.message = Some("归档区是只读历史，不能执行写动作".to_string());
            true
        } else {
            false
        }
    }

    /// 刷新顶栏身份区（`identity::describe` + `auth::effective_level`）。
    pub fn refresh_who(&mut self) {
        self.who = format!(
            "{} · L{}",
            req_guard_core::identity::describe(&self.root),
            req_guard_core::auth::effective_level(&self.root)
        );
    }

    /// 审核人/操作人输入框的预填值（REQ-015 G5：三个前端共用 core 的一份实现）。
    pub fn prefill_reviewer(&self) -> String {
        req_guard_core::identity::resolve_claimed(&self.root, None, "审核人", "--reviewer")
            .unwrap_or_default()
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
        if self.overlay == Overlay::Check {
            self.on_check_key(key.code);
            return;
        }
        // 归档确认：`d` = 继续，其余任意键 = 放弃（保留 `Esc` 的直觉）。
        if self.overlay == Overlay::DoneConfirm {
            if key.code == KeyCode::Char('d') {
                self.overlay = Overlay::None;
                self.input = self.prefill_reviewer();
                self.prompt = Some(Prompt::DoneActor);
            } else {
                self.overlay = Overlay::None;
                self.ack_blocking = false;
                self.message = Some("已取消归档".to_string());
            }
            return;
        }
        // 归档区是**可交互**的（要能上下翻历史清单），故单独走一套按键。
        if self.overlay == Overlay::Hygiene {
            self.on_hygiene_key(key.code);
            return;
        }
        if self.overlay == Overlay::Health {
            self.on_health_key(key.code);
            return;
        }
        if self.overlay == Overlay::Archived {
            self.on_archived_key(key.code);
            return;
        }
        if self.overlay == Overlay::Archive {
            self.on_archive_key(key.code);
            return;
        }
        if self.overlay != Overlay::None {
            // 其余覆盖层是只读的，任意键关闭。
            self.overlay = Overlay::None;
            return;
        }
        self.on_main_key(key);
    }

    /// 主界面的按键（无覆盖层时生效）。
    ///
    /// 抽出来是因为归档区视图要复用它：那个视图是**模式**而不是弹窗
    /// （左栏换成了历史清单，`browsing_archived` 为真），若把它的按键做成
    /// 「任意键即关」，用户按 `a` 会先被浮层吞掉一次——写动作看起来没反应，
    /// 而"没反应"在最坏情况下会被理解成"其实生效了"。
    fn on_main_key(&mut self, key: KeyEvent) {
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
            // `a` 在归档区视图下**不得**触发批准：core 对 `status=done` 有护栏，
            // 但界面不该给出一个"点了才发现被拒"的按钮（REQ-015 G3 / AC-015）。
            KeyCode::Char('a') => {
                if self.read_only() {
                    self.message = Some("归档区是只读历史，不能批准".into());
                } else {
                    self.begin_approve();
                }
            }
            KeyCode::Char('r') => self.begin_reject(),
            KeyCode::Char('n') => {
                if self.read_only() {
                    self.message = Some("归档区是只读历史，不能新建需求".into());
                } else {
                    // 两步：先编号（可留空 = 自动编号），再标题。
                    self.pending_new_id.clear();
                    self.input.clear();
                    self.open_prompt(Prompt::NewId);
                }
            }
            KeyCode::Char('g') => self.run_check(),
            // c：复看上一次的检查结果（不重跑）。理由与 REQ-016 的「只读体检不进
            // 轮询」同源：重跑一次判定会再写一条审计，而"再读一遍"不需要。
            KeyCode::Char('c') => {
                if self.check_detail.is_empty() {
                    self.message = Some("还没有检查结果：按 g 执行门禁检查".into());
                } else {
                    self.check_scroll = 0;
                    self.overlay = Overlay::Check;
                }
            }
            KeyCode::Char('b') => self.open_prompt(Prompt::BypassReason),
            // 审核人最高频的动作之一就是"留一条意见"，必须一步可达。
            KeyCode::Char('m') => self.open_comments(),
            // e=修订（方向没错、只是要改）；s=seal（绑定内容摘要，人类专属兜底）。
            KeyCode::Char('e') => self.begin_amend(),
            KeyCode::Char('s') => self.begin_seal(),
            // ── REQ-015：需求生命周期 ──
            // d=归档（done）；有未关闭阻塞评论时需按两次（TUI 没有勾选框）。
            KeyCode::Char('d') => self.begin_done(),
            // A=物理归档（预演 → 执行）；`Shift+A` 在预演之后确认执行。
            KeyCode::Char('A') => self.begin_archive(),
            KeyCode::Char('D') => self.confirm_archive(),
            // v=在 live 列表与归档区（只读历史）之间切换。
            KeyCode::Char('v') => self.toggle_archived_view(),
            // i=身份与鉴权等级（回答「我现在能不能审批」）。
            KeyCode::Char('i') => self.message = Some(self.who.clone()),
            // ── REQ-016：校验报告 ──
            // H=体检（只读，不签发凭据）；W=追加变更范围声明；S=一键批量绑定；G=刷新摘要。
            KeyCode::Char('H') => self.run_health(),
            KeyCode::Char('W') => {
                if self.deny_when_read_only() {
                    return;
                }
                self.pending_glob.clear();
                self.input.clear();
                self.prompt = Some(Prompt::DeclareGlob);
            }
            KeyCode::Char('S') => self.seal_all_unsealed(),
            KeyCode::Char('G') => self.refresh_digest(),
            // Y=工程卫生（编号冲突 / 自检 / 环境）。只读，不签发凭据。
            KeyCode::Char('Y') => self.run_hygiene(),
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
        if self.deny_when_read_only() {
            return;
        }
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
        if self.deny_when_read_only() {
            return;
        }
        if self.current().is_none() {
            self.message = Some("没有可审核的需求，先按 n 新建".into());
            return;
        }
        self.open_prompt(Prompt::RejectReviewer);
    }

    /// 请求修订当前段（REQ-004 G1，`requirement::amend`）。
    ///
    /// 与打回机械同构（回退待审 + 清 `sum=` + 必须重审），区别在段标签（`amended`）
    /// 与台账事件（`AMEND`）。单列一个键是因为"方向没错、只是漏个约束"用打回表达
    /// 会让台账里"否决"与"改稿"混成一坨，事后答不出"哪几段反复返工"。
    fn begin_amend(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        if self.current().is_none() {
            self.message = Some("没有可修订的需求，先按 n 新建".into());
            return;
        }
        // 已通过段同样可修订（且那一段**没有**"打回"按钮）：若把 amend 也限在未通过段，
        // "刚批准就想改一句"这条反馈路径在界面上就是死的。
        self.open_prompt(Prompt::AmendReviewer);
    }

    /// 绑定内容摘要（`requirement::seal`）：存量清单首次启用冻结 / 改稿后重新绑定。
    fn begin_seal(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        if self.current().is_none() {
            self.message = Some("没有可绑定的需求，先按 n 新建".into());
            return;
        }
        self.open_prompt(Prompt::SealReason);
    }

    /// 执行修订（提请人与说明都已就绪）。
    fn amend(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(r) = self.current() else {
            self.message = Some("没有可修订的需求".into());
            return;
        };
        let id = r.id.clone();
        let Some(step) = self.current_step() else {
            return;
        };
        let reviewer = self.pending_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.message = Some("修订提请人不能为空".into());
            return;
        }
        let reason = self.input.trim().to_string();
        if reason.is_empty() {
            self.message = Some("修订必须说明改什么、为什么改".into());
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 与批准 / 打回同一套界面进程内凭据（scope 仍是 `<需求>:<段>`）。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::amend(&self.root, &id, step, &reviewer, &reason, strict);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.prompt = None;
                self.input.clear();
                self.pending_reviewer.clear();
                self.reload();
                self.message = Some(format!(
                    "已请求修订 {} / {}（{}）—— 摘要已清空，段回到待审，请改完重新批准",
                    id,
                    requirement::step_label(step),
                    reviewer
                ));
            }
            Err(e) => self.message = Some(format!("修订失败：{}", e)),
        }
    }

    /// 执行内容摘要绑定。
    fn seal(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(r) = self.current() else {
            self.message = Some("没有可绑定的需求".into());
            return;
        };
        let id = r.id.clone();
        let reason = self.input.trim().to_string();
        if !self.prepare_credential("seal") {
            return;
        }
        let outcome = requirement::seal(&self.root, &id, &reason);
        self.clear_credential();
        match outcome {
            Ok(bound) => {
                self.prompt = None;
                self.input.clear();
                self.reload();
                let sums: Vec<String> = bound
                    .iter()
                    .map(|(label, sum)| format!("{}={}…", label, &sum[..8.min(sum.len())]))
                    .collect();
                self.message = Some(format!(
                    "已绑定 {} 段内容摘要{}",
                    bound.len(),
                    if sums.is_empty() {
                        String::new()
                    } else {
                        format!("：{}", sums.join("，"))
                    }
                ));
            }
            Err(e) => self.message = Some(format!("绑定失败：{}", e)),
        }
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
        if self.deny_when_read_only() {
            return;
        }
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
        if self.deny_when_read_only() {
            return;
        }
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
        self.clear_credential();
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

    /// 执行门禁检查，并打开结果浮层（REQ-014 G1/G4）。
    ///
    /// 走 [`gate::gate_check_with`] 并把**当前选中需求**作为消歧提示：裸的
    /// `gate_check` 既无变更集也无 hint，于是只要仓库里有 ≥2 份未归档清单，
    /// 判定就恒为 `Ambiguous` —— 界面上明明有"我这份能不能开工"的答案却丢掉，
    /// 把一个恒报拦截的按钮交给审核人。
    ///
    /// 口径（REQ-014 关键设计 1）：
    /// - 有选中项 → `hint = Some(当前 id)` + `PathSource::Staged`；
    /// - 无选中项 → `hint = None` + `PathSource::None`，**与改前逐字一致**
    ///   （不拿空变更集去反查，否则"忘了 `git add`"会伪装成"没有需求"）。
    ///
    /// footer 只放一行摘要、完整明细放浮层：多需求仓库的 `Ambiguous` 有十几行候选，
    /// footer 的 2 行可用高度装不下（见 [`Overlay::Check`]）。
    pub(crate) fn run_check(&mut self) {
        let (ctx, scope) = self.check_ctx();
        self.check_scope = scope;
        match gate::gate_check_with(&self.root, &ctx) {
            Ok(v) => {
                // 类别要在 detail 补 summary **之前**认：detail 首条可能是
                // "涉及需求：…" 那行，而每类别的判别词都在 summary 的首行里。
                self.check_kind = if v.is_pass() {
                    String::new()
                } else {
                    check_kind_of(v.summary(), v.detail())
                        .unwrap_or_default()
                        .to_string()
                };
                let mut lines = v.detail().to_vec();
                if lines.is_empty() || v.bypassed() {
                    lines.insert(0, v.summary().to_string());
                }
                // 绕过放行与正常放行必须一眼分得开：两者的事后责任完全不同。
                let summary = match (v.is_pass(), v.bypassed()) {
                    (true, true) => "门禁检查：放行 ⚠️（命中应急绕过窗口，非三段批准）".to_string(),
                    (true, false) => "门禁检查：放行 ✅".to_string(),
                    (false, _) => format!(
                        "门禁检查：拦截 ⛔ [{}]",
                        if self.check_kind.is_empty() {
                            "见详情"
                        } else {
                            &self.check_kind
                        }
                    ),
                };
                self.check_detail = lines;
                self.check_scroll = 0;
                self.message = Some(summary);
                self.overlay = Overlay::Check;
            }
            Err(e) => {
                self.check_kind = String::new();
                self.message = Some(format!("门禁检查失败：{}", e));
            }
        }
    }

    /// 本次检查用的裁决上下文 + 给人看的口径说明。
    ///
    /// 口径**必须显示出来**（REQ-014 G3）：`Staged` 为空时浮层要写明「空」，
    /// 否则结果会因为"忘了 `git add`"而长得像是另一回事。
    /// 取数用 [`req_guard_core::touch::staged_files`]，与 `resolve::collect_paths`
    /// 在 `PathSource::Staged` 下走的是同一个函数（`core/src/resolve.rs:739`），
    /// 故这里显示的条数与判定实际用的变更集一致；代价是一次额外的
    /// `git diff --cached`，只在按键触发时发生、不在 3 秒轮询路径上。
    pub(crate) fn check_ctx(&self) -> (resolve::Ctx, String) {
        let hint = self.current().map(|r| r.id.clone());
        let Some(id) = hint else {
            return (
                resolve::Ctx {
                    source: resolve::PathSource::None,
                    hint: None,
                },
                "全局判定（未选择需求）· 变更集：无".to_string(),
            );
        };
        let scope = match touch::staged_files(&self.root) {
            Ok(v) if v.is_empty() => format!("按 {id} 判定 · 变更集：staged（空）"),
            Ok(v) => format!("按 {id} 判定 · 变更集：staged（{} 个文件）", v.len()),
            Err(e) => format!("按 {id} 判定 · 变更集：staged（读取失败：{e}）"),
        };
        (
            resolve::Ctx {
                source: resolve::PathSource::Staged,
                hint: Some(id),
            },
            scope,
        )
    }

    /// 检查结果浮层的按键（只读，任意键关；`↑↓` / `k` `j` 滚动明细）。
    pub(crate) fn on_check_key(&mut self, key: KeyCode) {
        let page = CHECK_PAGE_LINES as u16;
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.check_scroll = self.check_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.check_scroll = self.check_scroll.saturating_add(1)
            }
            KeyCode::PageUp => self.check_scroll = self.check_scroll.saturating_sub(page),
            KeyCode::PageDown => self.check_scroll = self.check_scroll.saturating_add(page),
            KeyCode::Home => self.check_scroll = 0,
            _ => {
                self.overlay = Overlay::None;
                return;
            }
        }
        let max = u16::try_from(self.check_total_lines()).unwrap_or(u16::MAX);
        self.check_scroll = self.check_scroll.min(max.saturating_sub(page));
    }

    /// 检查结果浮层的总行数（明细行数，跨所有逻辑行）。
    pub(crate) fn check_total_lines(&self) -> usize {
        self.check_detail.iter().map(|l| l.lines().count()).sum()
    }

    // ---------- REQ-017：工程卫生（编号冲突 / 自检 / 环境）----------

    /// 跑一次工程卫生面板（**只读**，不签发凭据）。
    pub fn run_hygiene(&mut self) {
        let root = self.root.clone();
        let root = root.as_path();
        let mut report = HygieneReport::default();
        match req_guard_core::idcheck::check(root) {
            Ok(v) => report.id_issues = v,
            Err(e) => self.message = Some(format!("编号冲突检测失败：{}", e)),
        }
        // `base=None`（本地无 PR 视角，硬判只会误报）、`semantic=true`
        // （REQ-012 实测「只做子串存在性检查形同虚设」）。
        let mut problems = gate::verify_install_with(root, None, true);
        let warns = gate::verify_warnings(root);
        let errors = problems.len();
        problems.extend(warns.iter().cloned());
        report.selfcheck = Some((errors, warns.len(), problems));

        self.hygiene = Some(report);
        self.hygiene_tab = 0;
        self.overlay = Overlay::Hygiene;
    }

    /// 编号冲突的 `(错误数, 警告数)`。
    pub fn id_issue_counts(&self) -> (usize, usize) {
        let Some(h) = &self.hygiene else {
            return (0, 0);
        };
        let errors = h
            .id_issues
            .iter()
            .filter(|i| matches!(i.severity, req_guard_core::issue::Severity::Error))
            .count();
        (errors, h.id_issues.len() - errors)
    }

    /// 卫生浮层当前分组的文本（渲染与断言共用）。
    pub fn hygiene_lines(&self) -> Vec<String> {
        let Some(h) = &self.hygiene else {
            return vec!["（还没跑过卫生检查）".to_string()];
        };
        match self.hygiene_tab {
            0 => h
                .id_issues
                .iter()
                .map(|i| format!("[{}] {}", i.severity.as_str(), i.message))
                .collect(),
            1 => match &h.selfcheck {
                Some((e, w, msgs)) => {
                    let mut out = vec![format!("自检结果：错误 {e} / 警告 {w}")];
                    out.extend(msgs.iter().cloned());
                    out
                }
                None => vec!["（还没跑过自检）".to_string()],
            },
            _ => self.env_lines(),
        }
    }

    /// 环境摘要（身份 / 等级 / AI 上下文 / 票据**脱敏**摘要）。
    pub fn env_lines(&self) -> Vec<String> {
        let mut out = vec![
            format!("身份：{}", req_guard_core::identity::describe(&self.root)),
            format!(
                "鉴权等级（auth）：L{} · AI 上下文：{}",
                req_guard_core::auth::effective_level(&self.root),
                if req_guard_core::auth::is_ai_context() {
                    "是"
                } else {
                    "否"
                }
            ),
        ];
        out.push(match req_guard_core::token::summary() {
            Some(s) if s.enabled => {
                let left = match s.minutes_left {
                    Some(m) => format!("剩余 {m} 分钟"),
                    None => "已过期".to_string(),
                };
                format!(
                    "票据：通道已启用 · 模式 {} · {} · {}",
                    s.mode,
                    if s.scoped {
                        "范围绑定"
                    } else {
                        "未绑范围"
                    },
                    left
                )
            }
            Some(_) => "票据：通道未启用（L0–L2 无需票据）".to_string(),
            None => "票据：读不到凭据库".to_string(),
        });
        out
    }

    /// 某份需求的分段返工次数（≥ [`REWORK_REVIEW_THRESHOLD`] 时提示复盘）。
    pub fn rework_lines(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (key, n) in req_guard_core::requirement::amend_counts(&self.root, id) {
            let label = req_guard_core::requirement::step_label(key.as_str());
            out.push(format!("{label}：改稿 {n} 次"));
            if n >= REWORK_REVIEW_THRESHOLD {
                out.push(format!(
                    "⚠ {label} 已改稿 {n} 次，建议复盘：反复返工通常说明方案当初没想清楚"
                ));
            }
        }
        out
    }

    /// 卫生浮层的按键（`1`–`3` 切分组，任意其它键关闭）。
    pub(crate) fn on_hygiene_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('1') => self.hygiene_tab = 0,
            KeyCode::Char('2') => self.hygiene_tab = 1,
            KeyCode::Char('3') => self.hygiene_tab = 2,
            KeyCode::Tab => self.hygiene_tab = (self.hygiene_tab + 1) % 3,
            KeyCode::Char('Y') => self.run_hygiene(),
            _ => self.overlay = Overlay::None,
        }
    }

    // ---------- REQ-016：校验报告（只读体检 + 两个写入动作）----------

    /// 跑一次四类只读体检并缓存（**不签发凭据**：只读体检不消耗审批资格）。
    pub fn run_health(&mut self) {
        let root = self.root.clone();
        let root = root.as_path();
        let mut report = HealthReport::default();

        match touch::staged_files(root) {
            Ok(files) => match touch::check(
                root,
                touch::TouchScope::Union,
                None,
                &touch::Changed::Staged(files),
            ) {
                Ok(v) => report.touch = v,
                Err(e) => self.message = Some(format!("变更范围体检失败：{}", e)),
            },
            Err(e) => self.message = Some(format!("读取暂存区失败：{}", e)),
        }

        for r in self.reqs.clone() {
            if let Ok(content) = std::fs::read_to_string(&r.path) {
                for issue in req_guard_core::requirement::verify_sums(&content) {
                    report.sums.push((r.id.clone(), issue));
                }
            }
        }

        match req_guard_core::ac::check(root, &req_guard_core::ac::AcTarget::All) {
            Ok(v) => report.ac = v,
            Err(e) => self.message = Some(format!("验收标准体检失败：{}", e)),
        }

        // 交叉引用：**必须**用 `section_span` 定位第 2 段（`section_of` 会回退整篇，
        // 那会把 fail-closed 变成 fail-open）。
        if let Some(id) = self.current().map(|r| r.id.clone()) {
            if let Ok(r) = req_guard_core::requirement::find(root, &id) {
                if let Ok(content) = std::fs::read_to_string(&r.path) {
                    match App::cross_ref_slice(&content) {
                        Some((section, first_line)) => {
                            report.cross = touch::check_cross_refs(root, &section, first_line);
                        }
                        None => report.cross_locate_failed = true,
                    }
                }
            }
        }

        self.health = Some(report);
        self.health_tab = 0;
        self.overlay = Overlay::Health;
    }

    /// 交叉引用体检要校验的那一段切片：`(正文, 起始行号)`；定位失败返回 `None`。
    ///
    /// ★ **必须**用 `requirement::section_span`，不能用 `section_of`：
    /// 后者定位失败会**回退整篇**，于是"没找到第 2 段"被静默改写成"校验了整篇"——
    /// 把 fail-closed 变成 fail-open。抽成具名函数是为了让这条纪律
    /// **可被判决性实验打中**（AC-026）。
    pub fn cross_ref_slice(content: &str) -> Option<(String, usize)> {
        let (start, end) = req_guard_core::requirement::section_span(content, 1)?;
        let section = content
            .lines()
            .skip(start)
            .take(end - start)
            .collect::<Vec<_>>()
            .join("\n");
        Some((section, start + 1))
    }

    /// 每组的 `(错误数, 警告数)`，按 [`HEALTH_GROUPS`] 的顺序。
    pub fn health_counts(&self) -> [(usize, usize); 4] {
        let empty = (0usize, 0usize);
        let Some(h) = &self.health else {
            return [empty; 4];
        };
        let count = |v: Vec<req_guard_core::issue::Severity>| {
            (
                v.iter()
                    .filter(|s| matches!(s, req_guard_core::issue::Severity::Error))
                    .count(),
                v.iter()
                    .filter(|s| matches!(s, req_guard_core::issue::Severity::Warn))
                    .count(),
            )
        };
        [
            count(h.touch.iter().map(|i| i.severity).collect()),
            count(h.sums.iter().map(|(_, i)| i.severity).collect()),
            count(h.ac.iter().map(|i| i.severity).collect()),
            (h.cross.len(), 0),
        ]
    }

    /// 当前子标签的问题行（渲染与断言共用同一份文本）。
    pub fn health_lines(&self) -> Vec<String> {
        let Some(h) = &self.health else {
            return vec!["（还没跑过体检：按 H）".to_string()];
        };
        match self.health_tab {
            0 => h.touch.iter().map(|i| i.message.clone()).collect(),
            1 => h
                .sums
                .iter()
                .map(|(id, i)| format!("{} {}", id, i.message))
                .collect(),
            2 => h.ac.iter().map(|i| i.message.clone()).collect(),
            _ => {
                if h.cross_locate_failed {
                    // 定位失败 = 无从校验：如实说，**不给**任何具体引用的结论。
                    return vec!["第 2 段定位失败：无法校验交叉引用".to_string()];
                }
                h.cross
                    .iter()
                    .map(|i| {
                        format!(
                            "第 {} 行 → {} {}：{}",
                            i.line,
                            i.path,
                            i.section,
                            if i.target_missing {
                                "目标文件不存在"
                            } else {
                                "小节号不存在"
                            }
                        )
                    })
                    .collect()
            }
        }
    }

    /// 体检浮层的按键：`1`–`4` 切子标签，`↑↓` 滚动，任意其它键关闭。
    pub(crate) fn on_health_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('1') => self.health_tab = 0,
            KeyCode::Char('2') => self.health_tab = 1,
            KeyCode::Char('3') => self.health_tab = 2,
            KeyCode::Char('4') => self.health_tab = 3,
            KeyCode::Tab => self.health_tab = (self.health_tab + 1) % 4,
            // `H` 重跑一次（会重算 —— 与 GUI 的「重新体检」同一个语义）。
            KeyCode::Char('H') => self.run_health(),
            KeyCode::Char('D') => self.refresh_digest(),
            _ => self.overlay = Overlay::None,
        }
    }

    /// 追加变更范围声明的警示文本（与 GUI 同源，读同一个 core 配置）。
    pub fn declare_warning(&self) -> String {
        if gate::touch_reapprove(&self.root) {
            "提交后技术方案将被打回 pending 并需重新过审，且该段的内容摘要（sum=）会被清空"
                .to_string()
        } else {
            "提交后技术方案状态保持不变（本仓库 touch.reapprove 为 false）".to_string()
        }
    }

    /// 提交声明（两条提示之后）。
    fn declare_touch(&mut self, glob: String, reason: String, actor: &str) {
        let Some(id) = self.current().map(|r| r.id.clone()) else {
            self.message = Some("没有可声明范围的需求".to_string());
            return;
        };
        if !self.prepare_credential("any") {
            return;
        }
        let out = touch::declare(&self.root, &id, &[glob], &reason, actor);
        self.clear_credential();
        match out {
            Ok(added) => {
                self.reload();
                self.health = None;
                self.message = Some(if added.is_empty() {
                    "该路径已在声明里，未重复追加".to_string()
                } else {
                    format!("已追加 {} 条声明", added.len())
                });
            }
            Err(e) => self.message = Some(format!("声明失败：{}", e)),
        }
    }

    /// 一键批量绑定（走 `seal_all` —— 它内部就是「过滤掉 done → `seal_many`」，
    /// 界面不自己遍历：自己遍历就是第二个真相，且在 L3 下必坏）。
    pub fn seal_all_unsealed(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let actor = self.prefill_reviewer();
        if actor.is_empty() {
            self.message = Some("操作人不能为空（界面取自 git 身份）".to_string());
            return;
        }
        if !self.prepare_credential("seal") {
            return;
        }
        let out = req_guard_core::requirement::seal_all(&self.root, "界面一键批量绑定");
        self.clear_credential();
        match out {
            Ok(list) => {
                let ok = list.iter().filter(|o| !o.bound.is_empty()).count();
                self.reload();
                self.health = None;
                self.message = Some(format!("成功 {} / 尝试 {}", ok, list.len()));
            }
            Err(e) => self.message = Some(format!("批量绑定中止：{}", e)),
        }
    }

    /// 刷新审计摘要（非审批类，不签发凭据）。
    pub fn refresh_digest(&mut self) {
        match gate::audit_digest(&self.root) {
            Ok((path, text, n)) => {
                self.message = Some(format!(
                    "已刷新 DIGEST（{}，{} 行）：{}",
                    path.display(),
                    n,
                    text
                ));
            }
            Err(e) => self.message = Some(format!("刷新审计摘要失败：{}", e)),
        }
    }

    /// 清除界面凭据并计数（AC-013 要证「签发与清除各 1 次」）。
    fn clear_credential(&mut self) {
        self.cred_cleared += 1;
        req_guard_core::auth::clear_credential();
    }

    // ---------- REQ-015：需求生命周期（done / 归档 / 归档区浏览）----------

    /// 归档确认的三项事实（REQ-015 G4；与 GUI 的 `done_confirm_lines` 同源）。
    pub fn done_facts(&self) -> (usize, usize) {
        match self.current() {
            Some(r) => (r.approved_count(), r.blocking_comments),
            None => (0, 0),
        }
    }

    /// 三项事实的文本（渲染与断言共用，避免两处漂移）。
    pub fn done_confirm_lines(&self) -> Vec<String> {
        let (approved, blocking) = self.done_facts();
        vec![
            format!("① 当前进度：{}/3 已通过", approved),
            format!("② 未关闭阻塞评论：{} 条", blocking),
            "③ 归档后门禁不再管辖本需求，其上的阻塞评论随之失效。".to_string(),
        ]
    }

    /// `d` 键：归档（done）当前需求。
    ///
    /// 有未关闭阻塞评论时**第一次只亮事实**、第二次才真做（TUI 没有勾选框，
    /// 用「按两次」实现同一道关）。没有阻塞评论时一次即可——
    /// 无意义的第二次只会训练用户闭眼按。
    pub fn begin_done(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        if self.current().is_none() {
            self.message = Some("没有可归档的需求".to_string());
            return;
        }
        let (_, blocking) = self.done_facts();
        if blocking > 0 && !self.ack_blocking {
            self.ack_blocking = true;
            self.check_detail = self.done_confirm_lines();
            self.overlay = Overlay::DoneConfirm;
            self.message = Some("确认归档：看清上面三项事实后，再按一次 d".to_string());
            return;
        }
        self.input = self.prefill_reviewer();
        self.prompt = Some(Prompt::DoneActor);
    }

    /// 提交「归档（done）」的操作人并执行。
    fn do_done(&mut self, actor: String) {
        let Some(id) = self.current().map(|r| r.id.clone()) else {
            self.message = Some("没有可归档的需求".to_string());
            return;
        };
        if !self.prepare_credential(&format!("done:{}", id)) {
            return;
        }
        let outcome = req_guard_core::requirement::done(&self.root, &id, &actor);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.ack_blocking = false;
                self.reload();
                if self.selected >= self.reqs.len() && !self.reqs.is_empty() {
                    self.selected = self.reqs.len() - 1;
                }
                self.message = Some(format!(
                    "已归档 {id}：门禁不再管辖本需求（如需找回用 CLI `req-guard status --archived`）"
                ));
            }
            Err(e) => self.message = Some(format!("归档失败：{}", e)),
        }
    }

    /// `A` 键：物理归档菜单（预演 / 执行 / 清理到期）。
    pub fn begin_archive(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        self.input = self.prefill_reviewer();
        self.prompt = Some(Prompt::ArchiveActor);
    }

    /// 归档结果浮层的按键（只读，任意键关；`↑↓` 滚动）。
    pub(crate) fn on_archive_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.archive_scroll = self.archive_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.archive_scroll = self.archive_scroll.saturating_add(1)
            }
            _ => self.overlay = Overlay::None,
        }
    }

    /// 到期天数（`archive.after_days`，默认 30）。界面必须把它显示出来。
    pub fn archive_after_days(&self) -> u32 {
        gate::archive_after_days(&self.root)
    }

    /// 预演 / 执行归档（`which` 决定单份还是到期清扫）。
    pub(crate) fn run_archive(&mut self, which: ArchiveWhich, dry_run: bool) {
        let actor = self.input.trim().to_string();
        if actor.is_empty() {
            self.message = Some("操作人不能为空".to_string());
            return;
        }
        // scope 是**固定字面量 "archive"**（不带 id），与 `done:{id}` 的形状不同——
        // 那是历史遗留（done 按需求绑、archive 按动作绑），界面照抄不得统一。
        if !self.prepare_credential("archive") {
            return;
        }
        let days = self.archive_after_days();
        let outcome = match &which {
            ArchiveWhich::One(i) => {
                req_guard_core::requirement::archive_one(&self.root, &actor, i, dry_run)
            }
            ArchiveWhich::Due => {
                req_guard_core::requirement::archive_due(&self.root, &actor, days, dry_run)
            }
        };
        self.clear_credential();
        match outcome {
            Ok(moved) => {
                self.archive_plan = moved
                    .iter()
                    .map(|a| (a.src.display().to_string(), a.dst.display().to_string()))
                    .collect();
                self.archive_scroll = 0;
                let verb = if dry_run { "预演" } else { "已归档" };
                let note = match which {
                    ArchiveWhich::Due => format!("（到期判据：done 满 {} 天）", days),
                    ArchiveWhich::One(_) => String::new(),
                };
                // 诚实边界：`archive_due` 是 best-effort，返回值不含失败原因，
                // 故只说搬了几条 + 提示复核，不写「全部成功」。
                self.message = Some(format!(
                    "{} {} 条{}{}",
                    verb,
                    moved.len(),
                    note,
                    if dry_run {
                        "（磁盘未改动）"
                    } else {
                        "；若有遗漏请用 CLI 复核"
                    }
                ));
                self.overlay = Overlay::Archive;
                if !dry_run {
                    self.reload();
                }
            }
            Err(e) => self.message = Some(format!("归档失败：{}", e)),
        }
    }

    /// `v` 键：切换 live / 归档区视图。
    pub fn toggle_archived_view(&mut self) {
        if self.browsing_archived {
            self.browsing_archived = false;
            self.reload();
            self.message = Some("已切回需求列表（live）".to_string());
            return;
        }
        match status::req_list_archived(&self.root) {
            Ok(list) => {
                self.archived = list;
                self.browsing_archived = true;
                self.archived_sel = 0;
                self.load_body();
                self.overlay = Overlay::Archived;
                self.message = if self.archived.is_empty() {
                    Some("归档区是空的（还没有需求被归档）".to_string())
                } else {
                    Some(format!("归档区 {} 份（只读历史）", self.archived.len()))
                };
            }
            Err(e) => self.message = Some(format!("读取归档区失败：{}", e)),
        }
    }

    /// 归档区视图的按键：`↑↓` 选、`Esc`/任意键退出。
    pub(crate) fn on_archived_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => {
                self.archived_sel = self.archived_sel.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.archived.is_empty() {
                    self.archived_sel = (self.archived_sel + 1).min(self.archived.len() - 1);
                }
            }
            // `v` 在归档区浮层里必须仍然可用：它是这个视图唯一的出口键，
            // 被"任意键关闭"吞掉的话，用户得先关浮层再按一次才知道能出去。
            KeyCode::Char('v') => {
                self.overlay = Overlay::None;
                self.toggle_archived_view();
                return;
            }
            // 其它键：关掉浮层后**交回主界面**处理。
            // 归档区是模式而非弹窗，吞掉按键会让写动作"按了没反应"。
            other => {
                self.overlay = Overlay::None;
                self.on_main_key(KeyEvent::new(other, KeyModifiers::NONE));
                return;
            }
        }
        self.load_body();
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
        self.cred_issued += 1;
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
            Prompt::NewId => {
                // 留空 = 自动编号：**与改前逐字一致**（存量用户零感知）。
                // 有值时先给形态提示（`lint_id`），**只提示不阻断**。
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    let hints: Vec<String> = req_guard_core::idcheck::lint_id(trimmed)
                        .into_iter()
                        .map(|i| i.message)
                        .collect();
                    if !hints.is_empty() {
                        self.message =
                            Some(format!("编号形态提示（不阻断）：{}", hints.join("；")));
                    }
                }
                self.pending_new_id = trimmed.to_string();
                self.input.clear();
                self.prompt = Some(Prompt::NewTitle);
            }
            Prompt::NewTitle => {
                if value.is_empty() {
                    self.message = Some("标题不能为空".into());
                    return;
                }
                let want = std::mem::take(&mut self.pending_new_id);
                let id_arg = if want.is_empty() {
                    None
                } else {
                    Some(want.as_str())
                };
                match requirement::create(&self.root, id_arg, &value) {
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
            Prompt::AmendReviewer => {
                if value.is_empty() {
                    self.message = Some("修订提请人不能为空".into());
                    return;
                }
                self.pending_reviewer = value;
                self.input.clear();
                self.prompt = Some(Prompt::AmendReason);
            }
            Prompt::AmendReason => {
                self.input = value;
                self.amend();
            }
            Prompt::SealReason => {
                // 原因可留空：首次补绑定不需要理由，是否"已绑定过"由 core 判定。
                self.input = value;
                self.seal();
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
                self.clear_credential();
                match outcome {
                    Ok(_) => {
                        self.prompt = None;
                        self.input.clear();
                        self.message = Some("已开启应急绕过 60 分钟（已记审计）".into());
                    }
                    Err(e) => self.message = Some(format!("绕过失败：{}", e)),
                }
            }
            Prompt::DoneActor => {
                // 留空**不得**静默采用预填值：「没填」与「填了预填值」在审计上不可区分。
                if value.is_empty() {
                    self.message = Some("操作人不能为空".into());
                    return;
                }
                self.prompt = None;
                self.input.clear();
                self.do_done(value);
            }
            Prompt::DeclareGlob => {
                if value.is_empty() {
                    self.message = Some("路径不能为空".into());
                    return;
                }
                // 两步：先路径、再原因。原因要走台账，不能悄悄用固定串。
                self.pending_glob = value;
                self.input.clear();
                self.prompt = Some(Prompt::DeclareReason);
            }
            Prompt::DeclareReason => {
                if value.is_empty() {
                    self.message = Some("原因不能为空（会记入台账）".into());
                    return;
                }
                self.prompt = None;
                let actor = self.prefill_reviewer();
                let glob = std::mem::take(&mut self.pending_glob);
                self.declare_touch(glob, value, &actor);
            }
            Prompt::ArchiveActor => {
                if value.is_empty() {
                    self.message = Some("操作人不能为空".into());
                    return;
                }
                self.prompt = None;
                // 先预演（只算不搬），把 `src → dst` 亮给用户看。
                // 这与 GUI 的两个按钮等价，只是 TUI 用「先预演再执行」的顺序表达同一个谨慎。
                let which = self
                    .current()
                    .map(|r| ArchiveWhich::One(r.id.clone()))
                    .unwrap_or(ArchiveWhich::Due);
                self.run_archive(which, true);
                self.pending_reviewer = value;
            }
        }
    }

    /// 在预演之后真正执行归档（操作人沿用预演时输入的那个）。
    pub fn confirm_archive(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let actor = self.pending_reviewer.trim().to_string();
        if actor.is_empty() {
            self.message = Some("操作人不能为空".into());
            return;
        }
        self.input = actor;
        let which = match self.current() {
            Some(r) if !self.archive_plan.is_empty() => ArchiveWhich::One(r.id.clone()),
            _ => ArchiveWhich::Due,
        };
        self.run_archive(which, false);
    }

    /// 执行批准/打回。
    fn review(&mut self, reject: bool) {
        if self.deny_when_read_only() {
            return;
        }
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
        self.clear_credential();

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
            // ⚠️ 这里**不能**套一层泛泛的兜底措辞：`e` 里可能是一句完整的、
            // 带三条取解的鉴权文案（缺凭据 / 身份冲突 / AI 上下文），
            // 再包一层「××失败」，用户就会以为是自己操作错了，而不是"缺一张票"。
            // 动作名 + 原文，不多一个字。
            Err(e) => self.message = Some(format!("批准/打回失败：{e}")),
        }
    }
}

/// 弹窗里能放多少行评论（保守取值：弹窗 24 行 - 边框 2 - 头部 2 - 空行 1 - 键位 2）。
/// 用于"选中项滚出可视范围时才滚动"，值偏小只会让翻页早一点，不影响正确性。
const COMMENT_PAGE_LINES: usize = 17;

/// 检查结果浮层里能放多少行明细（保守取值：弹窗 20 行 - 边框 2 - 口径 1 - 类别 1 - 空行 1 - 键位 2）。
/// 同上：偏小只影响翻页早晚，不影响正确性。
const CHECK_PAGE_LINES: usize = 13;

/// 从门禁裁决的文案里认出拦截类别，返回 [`resolve::BlockKind::as_str`] 的字面量。
///
/// **为什么不直接拿 `BlockKind`**：`block_kind()` 只存在于 `resolve::Verdict` 上，
/// `gate_check_with` 返回的是 `GateVerdict`（`core/src/gate.rs:739-743` 的注释说明
/// 这是刻意保持的形状）。REQ-014 的 G5 要求 core 判定零改动、AC-010 要求 core
/// 测试条数不变，故本期**不给 core 加方法**，改在前端认类别。
///
/// ⚠️ 代价（REQ-014 关键设计 3 已记为遗留）：识别靠**判别词**，而判别词是 core
/// 文案的一部分。gui/src/app.rs 的同名函数是同一张表——将来若改 core 的裁决文案，
/// 两处都要同步改。更稳的做法是给 `GateVerdict` 加 `block_kind()` 供两端共用，
/// 留待单独立项。
///
/// 关键词取每个类别**首行**里那段唯一的说法（`core/src/resolve.rs` 的 R1 / R15
/// 内联文案与五个 `*_message` 函数），彼此不重叠；顺序即优先级。
pub(crate) fn check_kind_of(summary: &str, detail: &[String]) -> Option<&'static str> {
    let text = format!("{summary}\n{}", detail.join("\n"));
    const KINDS: &[(&str, &str)] = &[
        ("内容冻结校验失败", "SumMismatch"),
        ("不在可裁决的需求清单里", "UnknownSelection"),
        ("未找到待开发的需求清单", "NoRequirement"),
        ("与本次改动不相交", "SelectionMismatch"),
        ("存在未解决的阻塞性评论", "OpenBlockingComment"),
        ("无法确定本次改动属于哪份需求", "Ambiguous"),
        ("尚未通过审核", "StepNotApproved"),
    ];
    KINDS
        .iter()
        .find(|(needle, _)| text.contains(needle))
        .map(|(_, kind)| *kind)
}

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

//! GUI 状态机与渲染。
//!
//! 所有动作都落到 `req-guard-core`：审核走 `requirement::review`、新建走 `requirement::create`、
//! 绕过走 `gate::bypass`、判定走 `gate::gate_check`——与 CLI / TUI 完全等价。

use eframe::egui;
use req_guard_core::status::{self, ReqStatus, StepState};
use req_guard_core::{comment, gate, requirement};

use crate::markdown;
use crate::palette::Tone;
use std::path::Path;
use std::time::{Duration, Instant};

/// 轻量轮询间隔（不做文件系统监听）。
const REFRESH: Duration = Duration::from_secs(3);

/// 当前打开的弹窗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    None,
    NewReq,
    Approve,
    Reject,
    Bypass,
    Audit,
    CheckResult,
    /// 评论面板：看评论 / 新增 / 关闭（resolve）/ 重算行号锚点。
    Comments,
    /// 新增评论（`input_blocking` 决定是否阻塞）。
    NewComment,
    /// 关闭（resolve）选中的评论：需要审核人署名。
    ResolveComment,
    /// 修订（amend）当前段：审核人 + 必须给出改稿说明。
    Amend,
    /// 绑定内容摘要（seal）：给存量清单补绑定 / 改稿后重新绑定。
    Seal,
}

/// 管理台状态。
pub struct App {
    pub root: std::path::PathBuf,
    pub reqs: Vec<ReqStatus>,
    /// 选中的需求下标。
    pub selected: usize,
    /// 选中的步骤下标。
    pub step: usize,
    /// 当前需求**全文**（只读；按段展示时用 [`requirement::section_of`] 切片）。
    pub body: String,
    /// 正文是否按 Markdown 渲染（`false` = 原文等宽视图）。
    ///
    /// 默认渲染：审核人读的是内容而不是 `- [ ]` / `**`。原文视图保留下来是因为
    /// 审核人偶尔要逐字核对 GATE 标记行、或整段复制去别处。
    pub render_md: bool,
    /// 渲染视图的解析缓存：同一段同一份原文只解析一次（egui 每帧都重画）。
    md: markdown::Cache,
    pub dialog: Dialog,
    pub input_title: String,
    pub input_reviewer: String,
    pub input_reason: String,
    /// 底部状态提示。
    pub message: Option<String>,
    /// 底部状态提示的色调（红=失败 / 绿=成功 / 琥珀=提示；见 [`set_msg`]）。
    ///
    /// 与 `message` 成对写入：单独留 `message` 是历史包袱——所有提示都渲染成黄色，
    /// 于是"创建失败"和"已创建"长得一样，颜色完全没起到区分作用。
    pub msg_tone: Tone,
    /// 最近一次门禁检查结果。
    pub check_pass: bool,
    pub check_detail: Vec<String>,
    pub audit: Vec<String>,
    /// 当前需求的评论（打开面板时经 `comment::list` 读入）。
    pub comments: Vec<comment::Comment>,
    /// 评论面板里选中的评论下标。
    pub comment_sel: usize,
    /// 新增评论的正文输入。
    pub input_comment: String,
    /// 新增评论是否阻塞（未 resolve 即拦截编码）。
    pub input_blocking: bool,
    last_refresh: Instant,
}

impl App {
    pub fn new(root: &Path) -> App {
        let mut app = App {
            root: root.to_path_buf(),
            reqs: Vec::new(),
            selected: 0,
            step: 0,
            body: String::new(),
            render_md: true,
            md: markdown::Cache::default(),
            dialog: Dialog::None,
            input_title: String::new(),
            input_reviewer: String::new(),
            input_reason: String::new(),
            message: None,
            msg_tone: Tone::Warning,
            check_pass: false,
            check_detail: Vec::new(),
            audit: Vec::new(),
            comments: Vec::new(),
            comment_sel: 0,
            input_comment: String::new(),
            input_blocking: false,
            last_refresh: Instant::now(),
        };
        app.reload();
        app
    }

    pub fn current(&self) -> Option<&ReqStatus> {
        self.reqs.get(self.selected)
    }

    pub fn current_step(&self) -> Option<&'static str> {
        self.current()?.steps.get(self.step).map(|s| s.key)
    }

    /// 写一条底部提示。
    ///
    /// 色调按语义给：**失败/被拒 → `Tone::Danger`（红）**、**成功 → `Tone::Success`（绿）**、
    /// **校验不通过这类"还差点什么"→ `Tone::Warning`（琥珀，默认）**。
    /// 颜色之外再配一个图标（见 [`Tone::rich`]），色弱用户不看颜色也分得出级别。
    fn set_msg(&mut self, tone: Tone, text: impl Into<String>) {
        self.message = Some(text.into());
        self.msg_tone = tone;
    }

    /// 重新读取需求与正文。
    pub fn reload(&mut self) {
        match status::req_list(&self.root) {
            Ok(list) => {
                self.reqs = list;
                if self.selected >= self.reqs.len() {
                    self.selected = self.reqs.len().saturating_sub(1);
                }
            }
            Err(e) => self.set_msg(Tone::Danger, format!("读取需求失败：{}", e)),
        }
        self.load_body();
        self.last_refresh = Instant::now();
    }

    fn load_body(&mut self) {
        let body = match self.current() {
            Some(r) => {
                std::fs::read_to_string(&r.path).unwrap_or_else(|e| format!("读取正文失败：{}", e))
            }
            None => String::new(),
        };
        // 只有正文真的变了才作废缓存：3s 轮询每次都清会把每帧解析变成每 3s 解析（能忍，但没必要）。
        if body != self.body {
            self.body = body;
            // 正文变了 → 旧的解析结果作废（否则切需求后会渲染上一份内容）。
            self.md.clear();
        }
    }

    /// 3s 轻量轮询。
    fn maybe_refresh(&mut self) {
        if self.last_refresh.elapsed() >= REFRESH {
            self.reload();
        }
    }

    // ===================== 动作（全部走 core） =====================

    pub fn create_req(&mut self) {
        let title = self.input_title.trim().to_string();
        if title.is_empty() {
            self.set_msg(Tone::Warning, "标题不能为空");
            return;
        }
        match requirement::create(&self.root, None, &title) {
            Ok(r) => {
                self.dialog = Dialog::None;
                self.input_title.clear();
                self.reload();
                self.set_msg(Tone::Success, format!("已创建 {}", r.id));
            }
            Err(e) => self.set_msg(Tone::Danger, format!("创建失败：{}", e)),
        }
    }

    pub fn approve(&mut self) {
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.set_msg(Tone::Warning, "没有可审核的需求");
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "审核人不能为空");
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 界面进程内签发凭据：以"人类点击本界面"为在场证明，凭据全程只在进程内存里，
        // 不经 stdout / 环境变量（见 core auth::ui_issue_credential）。L0 为无操作。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::review(&self.root, &id, step, &reviewer, true, "", strict);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.reload();
                // 自动前进到第一个未通过的段：批准完阶段 1 后界面应立即可审阶段 2，
                // 否则光标停在已通过的旧段，后续段永远无法进入审核。
                self.jump_to_reviewable();
                self.set_msg(
                    Tone::Success,
                    format!("已批准 {} / {}", id, requirement::step_label(step)),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("批准失败：{}", e)),
        }
    }

    /// 界面进程内签发凭据（见 [`req_guard_core::auth::ui_issue_credential`]）。
    ///
    /// 返回 `false` 表示签发失败（`message` 已写明原因，调用方应放弃本次动作）；
    /// L0 下是无操作且返回 `true`——未开加固的项目照旧可审批。
    ///
    /// ★ 这是方案 2 解决"GUI 拿不到令牌"的关键：凭据由界面进程**自己签发并持有**，
    /// 不经 CLI stdout，也不需要人类先 `export REQ_GUARD_TOKEN`。
    fn prepare_credential(&mut self, scope: &str) -> bool {
        match req_guard_core::auth::ui_issue_credential(&self.root, scope) {
            Ok(_) => true,
            Err(e) => {
                self.set_msg(Tone::Danger, format!("签发界面凭据失败：{}", e));
                false
            }
        }
    }

    /// 把 `step` 光标移到第一个未通过的段（全通过时保持不动）。
    fn jump_to_reviewable(&mut self) {
        if let Some(r) = self.current() {
            if let Some(i) = r.steps.iter().position(|s| !s.state.is_approved()) {
                self.step = i;
            }
        }
    }

    pub fn reject(&mut self) {
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.set_msg(Tone::Warning, "没有可审核的需求");
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        let reason = self.input_reason.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "审核人不能为空");
            return;
        }
        if reason.is_empty() {
            self.set_msg(Tone::Warning, "打回必须填写原因");
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 同 approve：界面进程内签发凭据（人类点击 = 在场证明）。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::review(&self.root, &id, step, &reviewer, false, &reason, strict);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(_) => {
                // 打回原因同步落成阻塞性评论，保证 AI 必然看到"为什么被打回"。
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
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.input_reason.clear();
                self.reload();
                self.set_msg(
                    Tone::Success,
                    format!("已打回 {} / {}", id, requirement::step_label(step)),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("打回失败：{}", e)),
        }
    }

    /// 修订当前段（REQ-004 G1，`requirement::amend`）。
    ///
    /// 与 [`Self::reject`] 机械同构（回退待审 + 清 `sum=` + 必须重审），
    /// 区别只在状态标签与台账事件名：段标 `amended`、记 `AMEND`。
    /// 界面把它单列，是因为"方向没错、只是漏个约束"用 reject 表达会让台账里
    /// "否决"与"改稿"混成一坨，事后答不出"哪几段反复返工"。
    pub fn amend(&mut self) {
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.set_msg(Tone::Warning, "没有可修订的需求");
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "修订提请人不能为空");
            return;
        }
        let reason = self.input_reason.trim().to_string();
        if reason.is_empty() {
            self.set_msg(Tone::Warning, "修订必须说明改什么、为什么改");
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 与批准 / 打回同一套"界面进程内签发凭据"：scope 仍是 `<需求>:<段>`
        // （core 的 amend 走 ensure_human + ScopeCheck::Exact）。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::amend(&self.root, &id, step, &reviewer, &reason, strict);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.input_reason.clear();
                self.reload();
                self.set_msg(
                    Tone::Warning,
                    format!(
                        "已请求修订 {} / {}（{}）——摘要已清空，段已回到待审，请改完重新批准",
                        id,
                        requirement::step_label(step),
                        reviewer
                    ),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("修订失败：{}", e)),
        }
    }

    /// 绑定内容摘要（`requirement::seal`）：把已批准段的 `sum=` 绑到**当前正文**。
    ///
    /// 两个用途，界面上不区分（core 会按"还有段可绑 / 都绑过了"给出不同要求）：
    /// - 存量清单首次启用内容冻结（REQ-002 上线前批准的段）；
    /// - 正文已改、但确实无需重审时显式重新绑定——此时**必须给理由**，记 `RESEAL`。
    ///
    /// 人类专属：core 的 `ensure_human` 会拒 AI；界面照旧走进程内凭据。
    pub fn seal(&mut self) {
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可绑定的需求");
            return;
        };
        let id = r.id.clone();
        let reason = self.input_reason.trim().to_string();
        if !self.prepare_credential("seal") {
            return;
        }
        let outcome = requirement::seal(&self.root, &id, &reason);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(bound) => {
                self.dialog = Dialog::None;
                self.input_reason.clear();
                self.reload();
                let sums: Vec<String> = bound
                    .iter()
                    .map(|(label, sum)| format!("{}={}…", label, &sum[..8.min(sum.len())]))
                    .collect();
                self.set_msg(
                    Tone::Success,
                    format!(
                        "已绑定 {} 段内容摘要：{}",
                        bound.len(),
                        if sums.is_empty() {
                            "（无）".to_string()
                        } else {
                            sums.join("，")
                        }
                    ),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("绑定失败：{}", e)),
        }
    }

    pub fn do_bypass(&mut self) {
        let reason = self.input_reason.trim().to_string();
        if reason.is_empty() {
            self.set_msg(Tone::Warning, "应急绕过必须填写原因");
            return;
        }
        if !self.prepare_credential("bypass") {
            return;
        }
        let outcome = gate::bypass(&self.root, &reason, "gui", 60);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reason.clear();
                self.set_msg(Tone::Success, "已开启应急绕过 60 分钟（已记审计）");
            }
            Err(e) => self.set_msg(Tone::Danger, format!("绕过失败：{}", e)),
        }
    }

    pub fn run_check(&mut self) {
        match gate::gate_check(&self.root) {
            Ok(v) => {
                self.check_pass = v.is_pass();
                // 放行也可能是"靠绕过"：此时 summary 是警告文案，必须一起显示，
                // 否则弹窗里只剩脚本明细、看不出这是非常规放行。
                let mut lines = v.detail().to_vec();
                if lines.is_empty() || v.bypassed() {
                    lines.insert(0, v.summary().to_string());
                }
                self.check_detail = lines;
                self.dialog = Dialog::CheckResult;
            }
            Err(e) => self.set_msg(Tone::Danger, format!("检查失败：{}", e)),
        }
    }

    pub fn open_audit(&mut self) {
        self.audit = gate::audit_tail(&self.root, 200).unwrap_or_default();
        self.dialog = Dialog::Audit;
    }

    /// 打开评论面板（读当前需求的评论）。
    ///
    /// 与 `open_audit` 同一套路：先经 core 读盘再展示——界面不自己解析评论文件，
    /// 否则"两个界面看到的评论不一样"这种事迟早发生。
    pub fn open_comments(&mut self) {
        self.reload_comments();
        self.dialog = Dialog::Comments;
    }

    /// 重新读评论，并把选区夹回合法范围（新增/关闭后调用）。
    pub fn reload_comments(&mut self) {
        self.comments = match self.current() {
            Some(r) => comment::list(&self.root, &r.id).unwrap_or_default(),
            None => Vec::new(),
        };
        if self.comment_sel >= self.comments.len() {
            self.comment_sel = self.comments.len().saturating_sub(1);
        }
    }

    /// 新增一条评论（不改任何步骤状态——"审核人只评论、不改正文"的原则）。
    ///
    /// 锚定当前选中段：`step` 为空的总评在界面上无从表达，而"这句话在说哪一段"
    /// 恰恰是审核意见最要紧的信息（core 的 CLI 侧另支持 `--quote` 做行号锚定）。
    pub fn add_comment(&mut self) {
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可评论的需求");
            return;
        };
        let id = r.id.clone();
        let author = self.input_reviewer.trim().to_string();
        if author.is_empty() {
            self.set_msg(Tone::Warning, "评论作者不能为空");
            return;
        }
        let text = self.input_comment.trim().to_string();
        if text.is_empty() {
            self.set_msg(Tone::Warning, "评论内容不能为空");
            return;
        }
        let step = self.current_step();
        let blocking = self.input_blocking;
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
                self.input_comment.clear();
                self.input_reviewer.clear();
                self.input_blocking = false;
                self.dialog = Dialog::Comments;
                // 阻塞评论会改变门禁裁决 → 连同需求状态一起刷新（计数与未解锁标记）。
                self.reload();
                self.reload_comments();
                self.comment_sel = self
                    .comments
                    .iter()
                    .position(|x| x.id == c.id)
                    .unwrap_or(self.comment_sel);
                self.set_msg(
                    Tone::Success,
                    format!(
                        "已添加评论 {}{}",
                        c.id,
                        if blocking { "（阻塞）" } else { "" }
                    ),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("添加评论失败：{}", e)),
        }
    }

    /// 关闭（resolve）选中的评论。
    ///
    /// 与批准/打回同样走"界面进程内签发凭据"：`comment::resolve` 内含
    /// `auth::ensure_human(.., ScopeCheck::Exact("resolve:<需求ID>"))`（审批类动作），
    /// L3 下必须是范围票据，签发范围必须逐字对上。
    pub fn resolve_comment(&mut self) {
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可操作的需求");
            return;
        };
        let id = r.id.clone();
        let Some(c) = self.comments.get(self.comment_sel).cloned() else {
            self.set_msg(Tone::Warning, "请先选中一条评论");
            return;
        };
        if c.state == comment::CommentState::Resolved {
            self.set_msg(Tone::Warning, format!("{} 已是关闭状态", c.id));
            return;
        }
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "关闭人不能为空（关闭权归审核人）");
            return;
        }
        if !self.prepare_credential(&format!("resolve:{}", id)) {
            return;
        }
        let outcome = comment::resolve(&self.root, &id, &c.id, &reviewer);
        req_guard_core::auth::clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::Comments;
                self.input_reviewer.clear();
                self.reload();
                self.reload_comments();
                self.set_msg(Tone::Success, format!("已关闭 {}（解除阻塞）", c.id));
            }
            Err(e) => self.set_msg(Tone::Danger, format!("关闭失败：{}", e)),
        }
    }

    /// 正文被 AI 改过之后重算行号锚点：命中的刷新 `line`，找不到原文的标 `stale`。
    pub fn refresh_comment_anchors(&mut self) {
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可操作的需求");
            return;
        };
        let id = r.id.clone();
        match comment::refresh_anchors(&self.root, &id) {
            Ok(stale) => {
                self.reload_comments();
                // 全部命中算成功（绿），有失效算提醒（琥珀）——两种结果给两种颜色，
                // 免得"有 3 条失效"和"全部命中"看起来是同一件事。
                self.set_msg(
                    if stale == 0 {
                        Tone::Success
                    } else {
                        Tone::Warning
                    },
                    if stale == 0 {
                        "行号锚点已重算：全部命中".to_string()
                    } else {
                        format!("行号锚点已重算：{} 条找不到原文（已标 stale）", stale)
                    },
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("重算锚点失败：{}", e)),
        }
    }

    /// 切换项目根目录（rfd 文件对话框）。
    pub fn pick_root(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("选择项目根目录")
            .pick_folder()
        {
            self.root = dir;
            self.selected = 0;
            self.step = 0;
            self.comments.clear();
            self.comment_sel = 0;
            self.reload();
            self.set_msg(Tone::Success, format!("已切换到 {}", self.root.display()));
        }
    }
}

impl eframe::App for App {
    /// egui 0.36 起，`App` 拆成 `logic`（无 UI 的逻辑）与 `ui`（渲染）两个回调：
    /// 定时刷新与重绘请求放在 `logic`，避免每帧重复读盘。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.maybe_refresh();
        // 触发 3s 轮询的重绘（无操作时也能看到别人改的状态）
        ctx.request_repaint_after(REFRESH);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // egui 0.36：面板从根 Ui 分配空间（顺序即层级，CentralPanel 必须最后）。
        render_top(ui, self);
        render_bottom(ui, self);
        render_left(ui, self);
        render_center(ui, self);
        // 弹窗（Window）不属于面板体系，仍需 Context。
        let ctx = ui.ctx().clone();
        render_dialogs(&ctx, self);
    }
}

// ===================== 渲染 =====================

fn render_top(parent: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("top").show(parent, |ui| {
        ui.horizontal(|ui| {
            ui.heading("req-guard 门禁管理台");
            ui.separator();
            ui.label("项目:");
            ui.monospace(app.root.display().to_string());
            if ui.button("切换目录").clicked() {
                app.pick_root();
            }
            ui.separator();
            // 正文视图切换：默认渲染（可读性优先），原文视图给"逐字核对 / 整段复制"用。
            ui.label("正文:");
            if ui
                .selectable_label(app.render_md, "渲染")
                .on_hover_text("按 Markdown 渲染（标题/任务勾选框/表格/代码块）")
                .clicked()
            {
                app.render_md = true;
            }
            if ui
                .selectable_label(!app.render_md, "原文")
                .on_hover_text("等宽显示 Markdown 源，便于逐字核对与复制")
                .clicked()
            {
                app.render_md = false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 颜色走语义色板（`palette::Tone`），图标是颜色之外的第二条线索。
                let tone = match app.current() {
                    None => Tone::Muted,
                    Some(r) if r.is_blocked() => Tone::Danger,
                    Some(_) => Tone::Success,
                };
                let text = match app.current() {
                    None => "无需求",
                    Some(r) if r.is_blocked() => "未解锁",
                    Some(_) => "已解锁",
                };
                ui.label(tone.rich(ui, text));
            });
        });
    });
}

fn render_bottom(parent: &mut egui::Ui, app: &mut App) {
    egui::Panel::bottom("bottom").show(parent, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("创建需求").clicked() {
                app.dialog = Dialog::NewReq;
                app.input_title.clear();
            }
            if ui.button("刷新").clicked() {
                app.reload();
                app.set_msg(Tone::Success, "已刷新");
            }
            if ui.button("执行门禁检查").clicked() {
                app.run_check();
            }
            if ui.button("应急绕过").clicked() {
                app.dialog = Dialog::Bypass;
                app.input_reason.clear();
            }
            if ui.button("审计日志").clicked() {
                app.open_audit();
            }
            // 评论入口放在操作条上：审核人"只评论不改正文"，这是他最高频的动作之一。
            {
                let (open, blocking) = match app.current() {
                    Some(r) => (r.open_comments, r.blocking_comments),
                    None => (0, 0),
                };
                let label = if blocking > 0 {
                    format!("评论 {open}（阻塞 {blocking}）")
                } else {
                    format!("评论 {open}")
                };
                if ui
                    .add_enabled(
                        app.current().is_some(),
                        egui::Button::new(if blocking > 0 {
                            // 有阻塞评论 → 红色（文案里已经写了"阻塞"，颜色只是加强）。
                            egui::RichText::new(label).color(Tone::Danger.color(ui))
                        } else {
                            egui::RichText::new(label)
                        }),
                    )
                    .on_hover_text("查看 / 新增 / 关闭（resolve）审核评论")
                    .clicked()
                {
                    app.open_comments();
                }
            }
            // 内容摘要绑定：存量清单首次启用冻结、正文改过又确认无需重审时用。
            // 与评论入口一样常驻操作条——它是"人类专属兜底动作"，藏起来就等于不存在。
            {
                let pending = app
                    .current()
                    .map(|r| r.steps.iter().filter(|s| s.seal.needs_action()).count())
                    .unwrap_or(0);
                let label = if pending > 0 {
                    format!("绑定内容摘要（{pending} 段待处理）")
                } else {
                    "绑定内容摘要".to_string()
                };
                if ui
                    .add_enabled(
                        app.current().is_some(),
                        egui::Button::new(if pending > 0 {
                            egui::RichText::new(label).color(Tone::Warning.color(ui))
                        } else {
                            egui::RichText::new(label)
                        }),
                    )
                    .on_hover_text(
                        "把已批准段的内容摘要绑定到当前正文（存量清单补绑定 / 改稿后重新绑定）",
                    )
                    .clicked()
                {
                    app.input_reason.clear();
                    app.dialog = Dialog::Seal;
                }
            }
            // 提示的颜色由 `msg_tone` 决定：失败红 / 成功绿 / 提示琥珀，
            // 并带一个同语义的图标，色弱用户不靠颜色也分得出级别。
            let tone = app.msg_tone;
            if let Some(m) = &app.message {
                let rt = tone.rich(ui, m.clone());
                ui.separator();
                ui.label(rt);
            }
        });
    });
}

fn render_left(parent: &mut egui::Ui, app: &mut App) {
    egui::Panel::left("list")
        .resizable(true)
        .default_size(240.0)
        .show(parent, |ui| {
            ui.label(egui::RichText::new("需求列表").strong());
            ui.separator();
            if app.reqs.is_empty() {
                ui.label("（暂无需求，点「创建需求」）");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, r) in app.reqs.clone().iter().enumerate() {
                    // 图标也跟着状态换：原来两种状态都是「●」，色弱用户只能靠颜色分辨。
                    let (glyph, tone) = if r.is_blocked() {
                        ("⛔", Tone::Danger)
                    } else {
                        ("✓", Tone::Success)
                    };
                    let line = egui::RichText::new(format!("{} {} {}", glyph, r.id, r.title))
                        .color(tone.color(ui));
                    if ui.selectable_label(i == app.selected, line).clicked() {
                        app.selected = i;
                        app.step = 0;
                        app.load_body();
                    }
                }
            });
        });
}

fn render_center(parent: &mut egui::Ui, app: &mut App) {
    egui::CentralPanel::default_margins().show(parent, |ui| {
        let Some(req) = app.current().cloned() else {
            ui.centered_and_justified(|ui| {
                ui.label("还没有需求清单。点底部「创建需求」开始。");
            });
            return;
        };

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} {}", req.id, req.title)).strong());
            ui.separator();
            let (text, tone) = if req.is_blocked() {
                ("未解锁 — AI 不得编写/修改源码", Tone::Danger)
            } else {
                ("已解锁 — AI 可以开始编写代码", Tone::Success)
            };
            ui.label(tone.rich(ui, text));
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "进度 {}/3 已通过 · 评论 {} 条（阻塞 {}）",
                req.approved_count(),
                req.open_comments,
                req.blocking_comments
            ));
            if req.open_comments > 0 && ui.link("查看评论").clicked() {
                app.open_comments();
            }
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, s) in req.steps.iter().enumerate() {
                let (mark, tone) = match s.state {
                    StepState::Approved => ("✓", Tone::Success),
                    StepState::Rejected => ("✗", Tone::Danger),
                    // 修订与打回都要重新批准，色调一致；标记用 `~` 示"这是改稿不是否决"
                    // （与 TUI 的 `[~]` 同一套语义，别让两个界面看起来不一样）。
                    StepState::Amended => ("~", Tone::Danger),
                    StepState::Pending => ("○", Tone::Muted),
                };
                let who = s
                    .reviewer
                    .clone()
                    .map(|v| format!("  审核人 {} ", v))
                    .unwrap_or_default();
                // 冻结异常（未绑定 / 被改动 / 无法校验）直接写进段标题——
                // 审核人扫一眼三段就该知道哪段不能放行，而不是要点开才知道。
                let seal_note = match seal_badge(s.seal) {
                    Some((m, _)) => format!("  {} {}", m, s.seal.hint()),
                    None => String::new(),
                };
                let header = egui::RichText::new(format!(
                    "[{}] {}. {}  {}{}{}",
                    mark,
                    i + 1,
                    s.label,
                    s.state.label(),
                    who,
                    seal_note
                ))
                .color(tone.color(ui));

                // open(Some(..)) 会每帧强制开合状态：折叠交互本身展不开非选中段，
                // 所以这里把"点击标题"接管为"选中该段"，选中段下一帧即被展开。
                let resp = egui::CollapsingHeader::new(header)
                    .open(Some(i == app.step))
                    .show(ui, |ui| {
                        // 只显示**这一段**（需求分解 / 技术方案 / 测试计划），而不是整篇清单：
                        // 展开哪一段就看到哪一段，审核人不必自己在全文里找对应节，避免看错段点错批准。
                        // 切段规则在 core（与 TUI 同一条规则），定位失败时回退整篇。
                        let text = requirement::section_of(&app.body, i);
                        if app.render_md {
                            markdown::show(ui, app.md.get(i, &text));
                        } else {
                            add_raw_text(ui, &text);
                        }
                    });
                if resp.header_response.clicked() {
                    app.step = i;
                }

                // 当前段的冻结异常 → 直接给出「绑定摘要」入口：
                // 这条动作的正当理由就写在标题上，摆在旁边免得审核人再去想"该怎么办"。
                if i == app.step
                    && s.seal.needs_action()
                    && ui
                        .button("绑定本段内容摘要")
                        .on_hover_text(s.seal.hint())
                        .clicked()
                {
                    app.input_reason.clear();
                    app.dialog = Dialog::Seal;
                }

                // 当前段的审核按钮（已通过的段无需再审，不显示，避免误导）
                if i == app.step && s.state != StepState::Approved {
                    let can = req.can_review(s.key);
                    if ui
                        .add_enabled(can, egui::Button::new("批准"))
                        .on_hover_text(if can {
                            "批准当前段（需要审核人姓名）"
                        } else {
                            "顺序不满足：请先批准更早的步骤"
                        })
                        .clicked()
                    {
                        app.dialog = Dialog::Approve;
                        app.input_reviewer.clear();
                    }
                    if ui
                        .add_enabled(can, egui::Button::new("打回"))
                        .on_hover_text("打回当前段（审核人 + 原因必填）")
                        .clicked()
                    {
                        app.dialog = Dialog::Reject;
                        app.input_reviewer.clear();
                        app.input_reason.clear();
                    }
                }

                // 「修订」对**已通过**的段同样有意义：正文冻结之后，"方向没错、只是漏个约束"
                // 这种反馈最常发生在刚批准的段上，而已通过段上恰恰没有"打回"按钮。
                if i == app.step
                    && s.state == StepState::Approved
                    && ui
                        .button("请求修订")
                        .on_hover_text("方向没错、只是要改：回退待审 + 清空摘要，仍需重新批准")
                        .clicked()
                {
                    app.dialog = Dialog::Amend;
                    app.input_reviewer.clear();
                    app.input_reason.clear();
                }
                ui.separator();
            }
        });
    });
}

/// 段状态的可读标记（`Seal` 弹窗里逐段列状态时复用，避免两处各写一套符号）。
fn state_mark(state: StepState) -> &'static str {
    match state {
        StepState::Approved => "已通过",
        StepState::Rejected => "已打回",
        StepState::Amended => "待修订",
        StepState::Pending => "待审核",
    }
}

/// 内容冻结徽标：`(标记, 色调)`。`Frozen` / `NotApplicable` **不给徽标**——
/// 正常状态没必要在每段标题上再挂一个符号，界面噪声会淹没真正要处理的事。
fn seal_badge(seal: requirement::SealState) -> Option<(&'static str, Tone)> {
    match seal {
        requirement::SealState::Frozen | requirement::SealState::NotApplicable => None,
        requirement::SealState::NotSealed => Some(("○未绑定", Tone::Warning)),
        requirement::SealState::Changed => Some(("⚠已改动", Tone::Danger)),
        requirement::SealState::Unverifiable => Some(("✗无法校验", Tone::Danger)),
    }
}

/// 原文视图：只读、可滚动、**可框选 / 可复制**，等宽逐字呈现 Markdown 源。
///
/// ## 为什么不再用 `TextEdit::multiline(..).interactive(false)`
///
/// `interactive(false)` 会把 sense 降成 `Sense::hover()`（egui 0.36 源码
/// `text_edit/builder.rs` 里 `let sense = if interactive { .. } else { Sense::hover() }`）：
/// 鼠标按下 / 拖拽不参与命中测试 → **选不中文字**；不参与事件分发 → **收不到
/// `Event::Copy`，Ctrl+C 无效**；没有 response 也就挂不上右键菜单。
/// 而"原文"视图存在的唯一理由就是**逐字核对与整段复制**，等于整个功能被废掉。
///
/// ## 现在的做法：把 `&str` 当 `TextBuffer` 喂进去
///
/// egui 给 `&str` 实现了 `TextBuffer`，其中 `is_mutable()` 返回 `false`、
/// `insert_text` / `delete_char_range` 都是**空实现**。于是：
/// - 交互全开：能点选、能拖选、能 Ctrl+C、能挂右键菜单；
/// - 内容改不动：键盘输入与粘贴被空实现吃掉（不靠"下一帧覆盖回去"，所以不会闪）；
/// - 看着仍是只读：`is_mutable() == false` 时不画文本光标（源码同文件的
///   `if text.is_mutable() && interactive { ..绘制光标.. }`）。
///
/// 正文真值仍然只有 `app.body` 一份，界面改不动它——与"清单正文由 AI / 编辑器维护，
/// 界面只做审核决策"的原则一致。
fn add_raw_text(ui: &mut egui::Ui, text: &str) {
    // 「一键复制全文」：原文视图最高频的动作就是整段带走，给个显式按钮比让人拖选更快；
    // 顺手写清可复制的方式，免得再有人以为这里只能看。
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("复制全文")
            .on_hover_text("把本段 Markdown 源整段复制到剪贴板")
            .clicked()
        {
            ui.ctx().copy_text(text.to_string());
        }
        ui.label(
            egui::RichText::new("可选中文字；Ctrl+C 或右键菜单复制")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            read_only_text(
                ui,
                "raw_md",
                [ui.available_width(), 200.0],
                egui::TextStyle::Monospace,
                text,
            );
        });
}

/// 只读但**可框选、可复制**的文本框（[`add_raw_text`] 与评论正文共用）。
///
/// 关键在 `let mut buf: &str = text;`——传进去的是不可变 `TextBuffer`，
/// 交互开着而内容改不动，详见 [`add_raw_text`] 的说明。
fn read_only_text(
    ui: &mut egui::Ui,
    id_salt: &str,
    size: [f32; 2],
    font: egui::TextStyle,
    text: &str,
) -> egui::Response {
    let mut buf: &str = text;
    let resp = ui.add_sized(
        size,
        egui::TextEdit::multiline(&mut buf)
            .font(font)
            .id_salt(id_salt),
    );
    // 右键菜单：复制选中 / 全选 / 复制全文。
    // 菜单要读 TextEdit 的选区状态，所以挂在 response 上（id 从 `resp.id` 取，
    // 不用自己另造一个，免得两处 id 不同步）。
    let id = resp.id;
    resp.context_menu(|ui| {
        if ui.button("复制选中").clicked() {
            if let Some(sel) = selected_text(ui.ctx(), id, text) {
                ui.ctx().copy_text(sel);
            }
            ui.close();
        }
        if ui.button("全选").clicked() {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                )));
            egui::TextEdit::store_state(ui.ctx(), id, state);
            ui.close();
        }
        if ui.button("复制全文").clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
    });
    resp
}

/// 取只读文本框里当前选中的文字（没选就返回 `None`）。
fn selected_text(ctx: &egui::Context, id: egui::Id, text: &str) -> Option<String> {
    let state = egui::TextEdit::load_state(ctx, id)?;
    let range = state.cursor.char_range()?;
    if range.is_empty() {
        return None;
    }
    Some(range.slice_str(text).to_string())
}

/// 评论面板：列表 + 选中详情 + 动作条（新增 / 关闭 / 重算锚点 / 刷新）。
///
/// 全部经 `comment::*` 读写，界面只做展示与收集输入——与 CLI、TUI 同一套语义。
///
/// ⚠ `ScrollArea` 都给了显式 `id_salt`、每条评论包在 `push_id` 里：
/// egui 的控件 id 由父 id 派生，同一层里出现两个同名 `ScrollArea`、或循环里反复建
/// `TextEdit`，就会撞成同一个 id（表现为 "Second use of widget ID" + 框线画飞）。
fn render_comments(app: &mut App, ui: &mut egui::Ui) {
    let open = app
        .comments
        .iter()
        .filter(|c| c.state == comment::CommentState::Open)
        .count();
    let blocking = app.comments.iter().filter(|c| c.is_blocking_open()).count();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "共 {} 条 · 未解决 {} · 阻塞 {}",
            app.comments.len(),
            open,
            blocking
        ));
        if blocking > 0 {
            ui.label(Tone::Danger.rich(ui, "有未关闭的阻塞评论 → AI 不得编码"));
        }
    });
    ui.separator();

    if app.comments.is_empty() {
        ui.label("（还没有评论。审核人只评论、不改正文；打回会自动留一条阻塞评论。）");
    } else {
        egui::ScrollArea::vertical()
            .id_salt("comment_list")
            .max_height(220.0)
            .show(ui, |ui| {
                for (i, c) in app.comments.clone().iter().enumerate() {
                    let selected = i == app.comment_sel;
                    ui.push_id(i, |ui| {
                        // 未解决用琥珀（提示：还差一步），已解决用绿；图标也不同。
                        let (mark, tone) = match c.state {
                            comment::CommentState::Open => (Tone::Warning.glyph(), Tone::Warning),
                            comment::CommentState::Resolved => {
                                (Tone::Success.glyph(), Tone::Success)
                            }
                        };
                        let mut head = format!(
                            "{} [{}] {}{} · {} · {}",
                            mark,
                            c.id,
                            match c.step.as_deref() {
                                Some(s) => requirement::step_label(s),
                                None => "总评",
                            },
                            if c.blocking { " · 阻塞" } else { "" },
                            c.author,
                            c.ts
                        );
                        if let Some(n) = c.line {
                            head.push_str(&format!(" · L{}", n));
                        }
                        if c.stale {
                            head.push_str(" · ⚠锚点失效");
                        }
                        if let Some(r) = &c.reply {
                            head.push_str(&format!(" · 回复 {}", r));
                        }
                        let mut rt = egui::RichText::new(head).color(tone.color(ui));
                        if selected {
                            rt = rt.strong();
                        }
                        if ui.selectable_label(selected, rt).clicked() {
                            app.comment_sel = i;
                        }
                        // 只展开选中项的正文：列表一屏放得下，读起来像"目录 + 详情"。
                        if selected {
                            if let Some(q) = &c.quote {
                                ui.label(egui::RichText::new(format!("引用：{}", q)).italics());
                            }
                            let body = c.body.clone();
                            // 高度按正文长度估（约 50 字一行），否则短评论也占一大块空白。
                            let rows = (body.chars().count() / 50 + 1).clamp(1, 8) as f32;
                            egui::ScrollArea::vertical()
                                .id_salt("comment_body")
                                .max_height(24.0 * rows + 8.0)
                                .show(ui, |ui| {
                                    // 同样是"只读但可复制"：审核意见经常要被摘进回信/PR 里。
                                    read_only_text(
                                        ui,
                                        "comment_body_text",
                                        [ui.available_width(), 20.0 * rows],
                                        egui::TextStyle::Body,
                                        &body,
                                    );
                                });
                        }
                    });
                }
            });
    }

    ui.separator();
    // 动作条用 `horizontal_wrapped`：窗口窄时按钮换行续排，
    // 而不是被藏进横向滚动条里（"关闭评论"这种按钮被藏起来等于没有）。
    ui.horizontal_wrapped(|ui| {
        if ui.button("新增普通评论").clicked() {
            app.input_blocking = false;
            app.input_comment.clear();
            app.input_reviewer.clear();
            app.dialog = Dialog::NewComment;
        }
        if ui.button("新增阻塞评论").clicked() {
            app.input_blocking = true;
            app.input_comment.clear();
            app.input_reviewer.clear();
            app.dialog = Dialog::NewComment;
        }
        if ui
            .add_enabled(
                app.comments
                    .get(app.comment_sel)
                    .is_some_and(|c| c.state == comment::CommentState::Open),
                egui::Button::new("关闭选中（resolve）"),
            )
            .on_hover_text("关闭权归审核人：AI 只能回复、不能关闭（关闭会解除阻塞）")
            .clicked()
        {
            app.input_reviewer.clear();
            app.dialog = Dialog::ResolveComment;
        }
        if ui.button("重算行号锚点").clicked() {
            app.refresh_comment_anchors();
        }
        if ui.button("刷新").clicked() {
            app.reload_comments();
        }
        if ui.button("关闭面板").clicked() {
            app.dialog = Dialog::None;
        }
    });
}

fn render_dialogs(ctx: &egui::Context, app: &mut App) {
    let mut open = app.dialog != Dialog::None;
    if !open {
        return;
    }

    // 标题允许动态（评论面板要带上需求 ID），故用 String 而非 &'static str。
    let (title, body): (String, fn(&mut App, &mut egui::Ui)) = match app.dialog {
        Dialog::NewReq => ("创建需求".into(), |app, ui| {
            ui.label("标题：");
            ui.text_edit_singleline(&mut app.input_title);
            if ui.button("创建").clicked() {
                app.create_req();
            }
        }),
        Dialog::Approve => ("批准当前段".into(), |app, ui| {
            ui.label("审核人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            if ui.button("确认批准").clicked() {
                app.approve();
            }
        }),
        Dialog::Reject => ("打回当前段".into(), |app, ui| {
            ui.label("审核人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            ui.label("原因（必填，将同时写入阻塞性评论）：");
            ui.text_edit_singleline(&mut app.input_reason);
            if ui.button("确认打回").clicked() {
                app.reject();
            }
        }),
        Dialog::Bypass => ("应急绕过".into(), |app, ui| {
            ui.label("原因（必填，用于审计追溯）：");
            ui.text_edit_singleline(&mut app.input_reason);
            ui.label("默认时效 60 分钟，到期自动恢复硬拦截。");
            if ui.button("开启绕过").clicked() {
                app.do_bypass();
            }
        }),
        Dialog::Audit => (
            "审计日志（最近 200 行，倒序）".into(),
            |app, ui| {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        if app.audit.is_empty() {
                            ui.label("（暂无审计记录）");
                        } else {
                            // 按 core 判定的性质上色（分类只有一份，见 gate::audit_kind）。
                            // 拦截与绕过必须一眼可辨：把 BLOCK 显示成 PASS 就是"看着在放行"。
                            for line in app.audit.iter().rev() {
                                let color = match req_guard_core::gate::audit_kind(line) {
                                    req_guard_core::gate::AuditKind::Block => {
                                        egui::Color32::from_rgb(200, 80, 80)
                                    }
                                    req_guard_core::gate::AuditKind::Bypass => {
                                        egui::Color32::from_rgb(210, 170, 60)
                                    }
                                    req_guard_core::gate::AuditKind::Note => {
                                        egui::Color32::from_rgb(130, 130, 130)
                                    }
                                    req_guard_core::gate::AuditKind::Pass => {
                                        egui::Color32::from_rgb(90, 180, 110)
                                    }
                                    req_guard_core::gate::AuditKind::Event => egui::Color32::GRAY,
                                };
                                ui.monospace(egui::RichText::new(line).color(color));
                            }
                        }
                    });
            },
        ),
        Dialog::CheckResult => ("门禁检查结果".into(), |app, ui| {
            // 图标（✅ / ⛔）本身就区分得开，这里只统一颜色取值，不再额外叠一层图标。
            let (text, tone) = if app.check_pass {
                ("放行 ✅", Tone::Success)
            } else {
                ("拦截 ⛔", Tone::Danger)
            };
            ui.label(egui::RichText::new(text).color(tone.color(ui)));
            ui.separator();
            for line in app.check_detail.iter() {
                ui.label(line);
            }
        }),
        Dialog::Comments => (
            format!(
                "审核评论 · {}",
                app.current().map(|r| r.id.as_str()).unwrap_or("（无需求）")
            ),
            |app, ui| render_comments(app, ui),
        ),
        Dialog::NewComment => (
            format!(
                "新增{}评论 · 锚定第 {} 段",
                if app.input_blocking {
                    "阻塞"
                } else {
                    "普通"
                },
                app.current_step()
                    .map(requirement::step_label)
                    .unwrap_or("—")
            ),
            |app, ui| {
                ui.label("作者（必填，填「ai」表示 AI 回复——AI 不得新开评论）：");
                ui.text_edit_singleline(&mut app.input_reviewer);
                ui.label("内容（必填）：");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 120.0],
                            egui::TextEdit::multiline(&mut app.input_comment)
                                .desired_width(f32::MAX)
                                .hint_text("例：回滚方案需补充 DB 迁移回退"),
                        );
                    });
                ui.checkbox(
                    &mut app.input_blocking,
                    "阻塞性：未关闭（resolve）即拦截 AI 编码",
                );
                ui.horizontal(|ui| {
                    if ui.button("提交").clicked() {
                        app.add_comment();
                    }
                    if ui.button("取消").clicked() {
                        app.dialog = Dialog::Comments;
                    }
                });
            },
        ),
        Dialog::ResolveComment => (
            format!(
                "关闭评论 · {}",
                app.comments
                    .get(app.comment_sel)
                    .map(|c| c.id.clone())
                    .unwrap_or_else(|| "（未选中）".into())
            ),
            |app, ui| {
                let body = app
                    .comments
                    .get(app.comment_sel)
                    .map(|c| c.body.clone())
                    .unwrap_or_default();
                ui.label("将要关闭：");
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(body).italics());
                    });
                ui.label("关闭人（必填；只能填人类姓名，填 ai 会被 core 拒绝）：");
                ui.text_edit_singleline(&mut app.input_reviewer);
                ui.label("关闭会解除阻塞并入审计/台账，不可撤销（确需重开请新增一条评论）。");
                ui.horizontal(|ui| {
                    if ui.button("确认关闭").clicked() {
                        app.resolve_comment();
                    }
                    if ui.button("取消").clicked() {
                        app.dialog = Dialog::Comments;
                    }
                });
            },
        ),
        Dialog::Amend => (
            format!(
                "修订当前段 · {}",
                app.current_step()
                    .map(requirement::step_label)
                    .unwrap_or("—")
            ),
            |app, ui| {
                ui.label("提请人（必填）：");
                ui.text_edit_singleline(&mut app.input_reviewer);
                ui.label("改稿说明（必填）：");
                ui.label(
                    egui::RichText::new("说明写进清单与台账（AMEND），并作为阻塞性评论留给 AI。")
                        .italics(),
                );
                egui::ScrollArea::vertical()
                    .id_salt("amend_reason")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 100.0],
                            egui::TextEdit::multiline(&mut app.input_reason)
                                .desired_width(f32::MAX)
                                .hint_text("例：回滚方案缺 DB 迁移回退，方向没问题，请补该步骤"),
                        );
                    });
                ui.label("修订 = 回退待审 + 清空内容摘要，**仍需重新批准**；不豁免重审。");
                ui.horizontal(|ui| {
                    if ui.button("提交修订").clicked() {
                        app.amend();
                    }
                    if ui.button("取消").clicked() {
                        app.dialog = Dialog::None;
                    }
                });
            },
        ),
        Dialog::Seal => {
            (
                format!(
                    "绑定内容摘要 · {}",
                    app.current().map(|r| r.id.as_str()).unwrap_or("（无需求）")
                ),
                |app, ui| {
                    // 把当前各段的冻结状态摊开：审核人得先看清"哪几段可绑、哪几段已绑"，
                    // 否则点下去只会撞上 core 的报错文案。
                    if let Some(r) = app.current() {
                        for s in &r.steps {
                            let text =
                                format!("{} {}  {}", s.label, state_mark(s.state), s.seal.hint());
                            match seal_badge(s.seal) {
                                Some((_, tone)) => {
                                    ui.colored_label(tone.color(ui), text);
                                }
                                None => {
                                    ui.label(text);
                                }
                            }
                        }
                    }
                    ui.separator();
                    ui.label("原因（首次补绑定可留空）：");
                    ui.label(
                        egui::RichText::new(
                            "已全部绑定过再执行 = 承认内容改动无需重审，必须写明原因（记 RESEAL）。",
                        )
                        .italics(),
                    );
                    ui.text_edit_singleline(&mut app.input_reason);
                    ui.horizontal(|ui| {
                        if ui.button("确认绑定").clicked() {
                            app.seal();
                        }
                        if ui.button("取消").clicked() {
                            app.dialog = Dialog::None;
                        }
                    });
                },
            )
        }
        Dialog::None => return,
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .open(&mut open)
        .show(ctx, |ui| body(app, ui));

    if !open {
        app.dialog = Dialog::None;
    }
}

// ===================== 状态机单测（不需要窗口） =====================

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "req-guard-gui-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("创建临时目录");
        p
    }

    /// 填三段实质正文并全部批准（三段都要有实质内容才批得过）。
    ///
    /// core 的 `testutil` 是 `#[cfg(test)]`，gui 用不了，故就地填——
    /// 比把测试专用助手提成公开 API 划算（与 tui 的同名夹具同理由）。
    fn approved_doc(root: &Path) {
        requirement::create(root, None, "登录改造").expect("创建需求");
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let mut c = std::fs::read_to_string(&p).expect("清单应可读");
        for (heading, line) in [
            ("## 1. 需求分解", "- 背景与问题：GUI 状态机夹具。"),
            ("## 2. 技术方案", "- 总体思路：走 amend / seal。"),
            ("## 3. 测试计划", "- 验收门槛：三段全通过。"),
        ] {
            let needle = format!("{heading}\n");
            assert!(c.contains(&needle), "模板结构变了：{heading}");
            c = c.replacen(&needle, &format!("{needle}{line}\n"), 1);
        }
        std::fs::write(&p, c).expect("写入应成功");
        for step in ["decomposition", "solution", "testplan"] {
            requirement::review(root, "REQ-001", step, "寇工", true, "", false)
                .expect("三段都应批得过");
        }
    }

    /// 把所有标记行的 `sum=` 抹成 `-`，模拟"REQ-002 之前批准的存量清单"。
    fn strip_sums(root: &Path) {
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        let out: String = c
            .lines()
            .map(|l| match l.find(" sum=") {
                Some(i) if l.starts_with("<!-- GATE:STEP") => format!("{} sum=-", &l[..i]),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&p, out).expect("写入应成功");
    }

    fn seal_of(root: &Path, step: &str) -> requirement::SealState {
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        requirement::seal_state(&c, step)
    }

    /// 建一条评论（走 core，界面只负责读）。
    fn add_comment(root: &Path, blocking: bool, text: &str) {
        comment::add(
            root,
            "REQ-001",
            comment::NewComment {
                step: Some("solution"),
                author: "寇工",
                text,
                quote: None,
                blocking,
                reply: None,
            },
        )
        .expect("添加评论");
    }

    #[test]
    fn amend_回退待审且清空内容摘要() {
        let root = temp_root("amend");
        approved_doc(&root);
        assert_eq!(
            seal_of(&root, "decomposition"),
            requirement::SealState::Frozen
        );
        let mut app = App::new(&root);

        // 说明必填：没有说明就分不清"方向没错的改稿"与"这段被否决"
        app.input_reviewer = "寇工".into();
        app.input_reason = "   ".into();
        app.amend();
        assert_eq!(
            app.current().unwrap().steps[0].state,
            StepState::Approved,
            "说明为空时不应改动状态"
        );

        app.input_reason = "回滚方案缺 DB 迁移回退，方向没问题".into();
        app.amend();
        let r = app.current().expect("应有需求");
        assert_eq!(
            r.steps[0].state,
            StepState::Amended,
            "amend 应把段置为 amended（待修订）"
        );
        assert_eq!(
            r.steps[0].seal,
            requirement::SealState::NotApplicable,
            "amend 清空 sum= → 不再有已批准正文要保护"
        );
        assert!(!r.unlocked, "amend 不豁免重审：三段不再全通过 → 重新锁回");
        assert_eq!(app.dialog, Dialog::None, "成功后回到主界面");
        assert!(
            app.message.as_deref().unwrap_or("").contains("重新批准"),
            "提示要写明「仍需重审」：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn amend_已通过段也可请求修订() {
        // "方向没错、只是漏个约束"最常发生在刚批准的段上，而那一段没有「打回」按钮——
        // 若 amend 也只在未通过段可用，这条反馈路径在界面上就是死的。
        let root = temp_root("amend-approved");
        approved_doc(&root);
        let mut app = App::new(&root);
        app.input_reviewer = "寇工".into();
        app.input_reason = "补充回滚步骤".into();
        app.amend();
        assert_eq!(app.current().unwrap().steps[0].state, StepState::Amended);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seal_为存量清单补绑定后三段皆冻结() {
        let root = temp_root("seal-fresh");
        approved_doc(&root);
        strip_sums(&root);
        let mut app = App::new(&root);
        assert_eq!(
            app.current().unwrap().steps[0].seal,
            requirement::SealState::NotSealed
        );

        app.input_reason = "   ".into();
        app.seal();
        for step in ["decomposition", "solution", "testplan"] {
            assert_eq!(
                seal_of(&root, step),
                requirement::SealState::Frozen,
                "{step} 首次补绑定后应真正冻结"
            );
        }
        assert_eq!(app.dialog, Dialog::None);
        assert!(
            app.message.as_deref().unwrap_or("").contains("已绑定"),
            "应报出绑了哪几段：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seal_重复绑定须给理由() {
        let root = temp_root("seal-reseal");
        approved_doc(&root);
        strip_sums(&root);
        let mut app = App::new(&root);
        app.seal();

        // 已全部绑定过 → 空理由应被 core 挡下（正常路径是 amend → 改 → approve）
        app.input_reason = "".into();
        app.seal();
        assert!(
            app.message.as_deref().unwrap_or("").contains("reason"),
            "应提示需给理由：{:?}",
            app.message
        );

        app.input_reason = "改动仅为错别字".into();
        app.seal();
        assert!(
            app.message.as_deref().unwrap_or("").contains("已绑定"),
            "给了理由应放行（记 RESEAL）：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seal_代劳审批的绕道被core挡住() {
        // amend 清了 sum= 之后 seal 不能把未批准的段绑上，否则"打回 → 改 → seal"
        // 就是一条跳过重审的通道（REQ-002 的核心防线，界面不能自己开口子）。
        let root = temp_root("seal-no-approve");
        approved_doc(&root);
        let mut app = App::new(&root);
        app.step = 1;
        app.input_reviewer = "寇工".into();
        app.input_reason = "补声明".into();
        app.amend();
        app.input_reason = "".into();
        app.seal();
        assert_eq!(
            seal_of(&root, "solution"),
            requirement::SealState::NotApplicable,
            "未批准的段不应被 seal 绑定"
        );
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("未处于 approved"),
            "应把 core 的拒绝理由原样带出来：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 打开面板读出评论与阻塞计数() {
        let root = temp_root("open");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, true, "回滚方案需补充 DB 迁移回退");
        let mut app = App::new(&root);

        app.open_comments();
        assert_eq!(app.dialog, Dialog::Comments);
        assert_eq!(app.comments.len(), 1);
        assert!(app.comments[0].is_blocking_open());
        assert_eq!(app.comment_sel, 0, "默认选中第一条");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 新增评论经core落盘且锚定当前段() {
        let root = temp_root("add");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);

        // 作者 / 内容任一为空都应被拒，且不留下半条评论
        app.input_reviewer = "  ".into();
        app.input_comment = "缺少失败锁定".into();
        app.add_comment();
        assert!(app.comments.is_empty(), "作者为空不应落盘");

        app.input_reviewer = "寇工".into();
        app.input_comment = "  ".into();
        app.add_comment();
        assert!(app.comments.is_empty(), "内容为空不应落盘");

        app.input_comment = "缺少失败锁定".into();
        app.input_blocking = true;
        app.add_comment();

        let list = comment::list(&root, "REQ-001").expect("读评论");
        assert_eq!(list.len(), 1);
        assert!(list[0].is_blocking_open(), "勾了阻塞就应阻塞");
        assert_eq!(list[0].author, "寇工");
        assert_eq!(
            list[0].step.as_deref(),
            Some("decomposition"),
            "锚定当前选中段"
        );
        // 面板回到列表视图，且需求计数已刷新（阻塞评论会改变门禁裁决）
        assert_eq!(app.dialog, Dialog::Comments);
        assert_eq!(app.comments.len(), 1);
        assert!(app.current().expect("应有需求").blocking_comments == 1);
        assert!(app.input_comment.is_empty(), "提交后清空输入");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 关闭评论需审核人且改为resolved() {
        let root = temp_root("resolve");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, true, "回滚方案需补充 DB 迁移回退");
        let mut app = App::new(&root);
        app.open_comments();

        // 关闭人为空：应拒绝，且评论仍是 open
        app.resolve_comment();
        assert_eq!(
            comment::list(&root, "REQ-001").unwrap()[0].state,
            comment::CommentState::Open,
            "关闭人为空不应关闭"
        );

        app.input_reviewer = "寇工".into();
        app.resolve_comment();
        let list = comment::list(&root, "REQ-001").unwrap();
        assert_eq!(list[0].state, comment::CommentState::Resolved);
        assert!(!list[0].is_blocking_open(), "关闭后不再阻塞");
        assert_eq!(app.comments[0].state, comment::CommentState::Resolved);
        assert_eq!(
            app.current().expect("应有需求").blocking_comments,
            0,
            "阻塞计数应随关闭归零"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 重复关闭已关闭评论会被挡下() {
        let root = temp_root("resolve-twice");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, false, "普通意见");
        let mut app = App::new(&root);
        app.open_comments();
        app.input_reviewer = "寇工".into();
        app.resolve_comment();
        // 面板已刷新成 resolved，再关一次应被挡（而不是把审计写重复）
        app.input_reviewer = "寇工".into();
        app.resolve_comment();
        assert!(
            app.message.as_deref().unwrap_or("").contains("已是关闭"),
            "应提示已关闭：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 重算锚点把找不到原文的标记为stale() {
        let root = temp_root("anchors");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        comment::add(
            &root,
            "REQ-001",
            comment::NewComment {
                step: None,
                author: "寇工",
                text: "带锚点的意见",
                quote: Some("这段引用在正文中并不存在"),
                blocking: false,
                reply: None,
            },
        )
        .expect("添加评论");
        let mut app = App::new(&root);
        app.open_comments();

        app.refresh_comment_anchors();
        assert!(app.comments[0].stale, "找不到原文应标 stale");
        assert!(
            app.message.as_deref().unwrap_or("").contains("stale"),
            "应说明有几条失效：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 切换目录清空旧需求的评论() {
        let root = temp_root("switch");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, false, "意见");
        let mut app = App::new(&root);
        app.open_comments();
        assert_eq!(app.comments.len(), 1);

        let other = temp_root("switch-other");
        app.root = other.clone();
        app.comments.clear();
        app.reload_comments();
        assert!(app.comments.is_empty(), "换项目后不应残留上一个需求的评论");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
    }

    // ===================== 原文视图：只读，但能选中 / 能复制 =====================

    /// 只读的**机制**来自 egui 给 `&str` 的 `TextBuffer` 实现：`is_mutable()` 为 false，
    /// 且写入动作（插入 / 删除 / 清空 / 整体替换）全是空实现。
    ///
    /// 这条依赖必须钉死——哪天 egui 把 `&str` 改成可写，原文视图会**悄悄**变成能改，
    /// 那时这个测试会先红，而不是等审核人发现清单被改了。
    #[test]
    fn 只读靠不可变的文本缓冲区实现() {
        use egui::TextBuffer;

        let src = "## 1. 需求分解\n";
        let mut buf: &str = src;
        assert!(!TextBuffer::is_mutable(&buf), "&str 必须是只读 buffer");

        TextBuffer::insert_text(&mut buf, "注入", egui::text::CharIndex(0));
        TextBuffer::delete_char_range(&mut buf, egui::text::CharIndex(0)..egui::text::CharIndex(2));
        TextBuffer::clear(&mut buf);
        TextBuffer::replace_with(&mut buf, "篡改");
        assert_eq!(buf, src, "只读 buffer 的写入动作必须全部是空实现");
    }

    /// 测试用的原文（两行，够拖出一段选区）。
    const RAW_SRC: &str = "## 1. 需求分解\n- [ ] 背景与问题\n";

    /// 左键按下 / 松开事件。
    fn click_at(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn raw_screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(420.0, 260.0))
    }

    /// 跑一帧「原文文本框」，返回本帧输出与控件 `Response`。
    ///
    /// 离屏跑帧没有渲染器来消费纹理增量，必须手动 clear（否则 epaint 在 Drop 时 panic）。
    fn raw_frame(
        ctx: &egui::Context,
        screen: egui::Rect,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, egui::Response) {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let mut resp = None;
        let mut out = ctx.run_ui(input, |ui| {
            resp = Some(read_only_text(
                ui,
                "raw_md_test",
                [400.0, 200.0],
                egui::TextStyle::Monospace,
                RAW_SRC,
            ));
        });
        out.textures_delta.clear();
        (out, resp.expect("read_only_text 应返回 response"))
    }

    /// 从输出里捞所有写进剪贴板的内容。
    fn copied_texts(out: &egui::FullOutput) -> Vec<String> {
        out.platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// 只读文本框必须**参与指针交互**。
    ///
    /// `interactive(false)` 的老写法会把 sense 降成 `Sense::hover()`：点不中、拖不动、
    /// 也拿不到焦点——这正是原来"选不中、复制不了"的根因。这里把这两条钉住。
    #[test]
    fn 只读文本框参与指针交互() {
        let ctx = egui::Context::default();
        let screen = raw_screen();
        let (_, r) = raw_frame(&ctx, screen, vec![]);
        let id = r.id;
        let p = egui::pos2(8.0, 6.0);

        let (_, r) = raw_frame(&ctx, screen, vec![egui::Event::PointerMoved(p)]);
        assert!(r.hovered(), "指针移到正文上应算 hover");

        // 按下即拿焦点：旧写法（sense = hover）这一步必然失败。
        let _ = raw_frame(&ctx, screen, vec![click_at(p, true)]);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(id),
            "在只读正文上按下应让文本框获得焦点"
        );

        // 按住并移动 → 进入拖拽态（鼠标框选的前提）。
        let _ = raw_frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(80.0, 6.0))],
        );
        let (_, r) = raw_frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(200.0, 6.0))],
        );
        assert!(r.dragged(), "按住并移动后应进入拖拽态，否则框选无从谈起");
    }

    /// 「选中 → 复制」两个出口：右键菜单的「复制选中」与 Ctrl+C。
    ///
    /// 选区直接写进 TextEdit 状态（菜单里的「全选」也是这么写），再分别验两个出口。
    /// 不模拟鼠标拖选的原因见 [`只读文本框参与指针交互`]：离屏帧没有真实窗口几何，
    /// `Galley::cursor_from_pos` 一律返回第 0 个字符（可编辑的 TextEdit 同样如此，已实测），
    /// 选区内容只能在状态层面验。
    #[test]
    fn 选中内容可经右键菜单与快捷键复制() {
        let ctx = egui::Context::default();
        let screen = raw_screen();
        let p = egui::pos2(8.0, 6.0);
        let _ = raw_frame(&ctx, screen, vec![]);
        let _ = raw_frame(&ctx, screen, vec![egui::Event::PointerMoved(p)]);
        let (_, r) = raw_frame(&ctx, screen, vec![click_at(p, true)]);
        let id = r.id;

        let want = "## 1. 需求分解";
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(want.chars().count()),
            )));
        egui::TextEdit::store_state(&ctx, id, state);

        // 出口一：右键菜单「复制选中」走的就是它。
        assert_eq!(
            selected_text(&ctx, id, RAW_SRC).as_deref(),
            Some(want),
            "「复制选中」应取出当前选区"
        );

        // 出口二：Ctrl+C。真实窗口里 egui-winit 就是把 Ctrl+C 翻成 `Event::Copy`。
        //
        // ⚠ 这里只断言"选区的文字确实进了剪贴板"，不比对完整内容：
        // 离屏帧里 galley 的几何是退化的（量出来 `Galley::rect` 是 0×0），
        // `clamp_cursor` 会把选区夹短（实测 0..9 被夹成 0..2，且**换成可编辑的
        // `String` buffer 也一样**，属测试环境限制而非本改动引入）。选区的取值本身
        // 由上面 `selected_text` 那条断言负责——它不走 galley。
        let (out, _) = raw_frame(&ctx, screen, vec![egui::Event::Copy]);
        let copied = copied_texts(&out);
        assert_eq!(
            copied.len(),
            1,
            "Ctrl+C 应产生一条复制命令；实际输出：{:?}",
            out.platform_output.commands
        );
        assert!(!copied[0].is_empty(), "Ctrl+C 复制出来的内容不应为空");
        assert!(
            RAW_SRC.starts_with(copied[0].as_str()),
            "复制出来的应是原文的一段，实际：{:?}",
            copied[0]
        );
    }

    /// 「一键复制全文」按钮：点一下就整段进剪贴板（不依赖选中状态）。
    #[test]
    fn 原文视图的一键复制全文按钮可用() {
        let ctx = egui::Context::default();
        let screen = raw_screen();
        let run = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| add_raw_text(ui, RAW_SRC));
            out.textures_delta.clear();
            out
        };

        // 按钮在 `add_raw_text` 顶部那一行的最左端。
        let btn = egui::pos2(12.0, 8.0);
        let _ = run(vec![]);
        let _ = run(vec![egui::Event::PointerMoved(btn), click_at(btn, true)]);
        let out = run(vec![egui::Event::PointerMoved(btn), click_at(btn, false)]);

        let copied = copied_texts(&out);
        assert_eq!(
            copied,
            vec![RAW_SRC.to_string()],
            "点「复制全文」应把整段原文放进剪贴板；实际输出：{:?}",
            out.platform_output.commands
        );
    }
}

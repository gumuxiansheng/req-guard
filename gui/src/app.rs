//! GUI 状态机与渲染。
//!
//! 所有动作都落到 `req-guard-core`：审核走 `requirement::review`、新建走 `requirement::create`、
//! 绕过走 `gate::bypass`、判定走 `gate::gate_check`——与 CLI / TUI 完全等价。

use eframe::egui;
use req_guard_core::status::{self, ReqStatus, StepState};
use req_guard_core::{comment, gate, requirement};

use crate::markdown;
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
    /// 最近一次门禁检查结果。
    pub check_pass: bool,
    pub check_detail: Vec<String>,
    pub audit: Vec<String>,
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
            check_pass: false,
            check_detail: Vec::new(),
            audit: Vec::new(),
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

    /// 重新读取需求与正文。
    pub fn reload(&mut self) {
        match status::req_list(&self.root) {
            Ok(list) => {
                self.reqs = list;
                if self.selected >= self.reqs.len() {
                    self.selected = self.reqs.len().saturating_sub(1);
                }
            }
            Err(e) => self.message = Some(format!("读取需求失败：{}", e)),
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
            self.message = Some("标题不能为空".into());
            return;
        }
        match requirement::create(&self.root, None, &title) {
            Ok(r) => {
                self.dialog = Dialog::None;
                self.input_title.clear();
                self.reload();
                self.message = Some(format!("已创建 {}", r.id));
            }
            Err(e) => self.message = Some(format!("创建失败：{}", e)),
        }
    }

    pub fn approve(&mut self) {
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.message = Some("没有可审核的需求".into());
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.message = Some("审核人不能为空".into());
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
                self.message = Some(format!("已批准 {} / {}", id, requirement::step_label(step)));
            }
            Err(e) => self.message = Some(format!("批准失败：{}", e)),
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
                self.message = Some(format!("签发界面凭据失败：{}", e));
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
                self.message = Some("没有可审核的需求".into());
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        let reason = self.input_reason.trim().to_string();
        if reviewer.is_empty() {
            self.message = Some("审核人不能为空".into());
            return;
        }
        if reason.is_empty() {
            self.message = Some("打回必须填写原因".into());
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
                self.message = Some(format!("已打回 {} / {}", id, requirement::step_label(step)));
            }
            Err(e) => self.message = Some(format!("打回失败：{}", e)),
        }
    }

    pub fn do_bypass(&mut self) {
        let reason = self.input_reason.trim().to_string();
        if reason.is_empty() {
            self.message = Some("应急绕过必须填写原因".into());
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
                self.message = Some("已开启应急绕过 60 分钟（已记审计）".into());
            }
            Err(e) => self.message = Some(format!("绕过失败：{}", e)),
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
            Err(e) => self.message = Some(format!("检查失败：{}", e)),
        }
    }

    pub fn open_audit(&mut self) {
        self.audit = gate::audit_tail(&self.root, 200).unwrap_or_default();
        self.dialog = Dialog::Audit;
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
            self.reload();
            self.message = Some(format!("已切换到 {}", self.root.display()));
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
                let (text, color) = match app.current() {
                    None => ("无需求", egui::Color32::GRAY),
                    Some(r) if r.is_blocked() => ("未解锁", egui::Color32::RED),
                    Some(_) => ("已解锁", egui::Color32::GREEN),
                };
                ui.colored_label(color, text);
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
                app.message = Some("已刷新".into());
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
            if let Some(m) = &app.message {
                ui.separator();
                ui.colored_label(egui::Color32::YELLOW, m);
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
                    let color = if r.is_blocked() {
                        egui::Color32::RED
                    } else {
                        egui::Color32::GREEN
                    };
                    let line = egui::RichText::new(format!("● {} {}", r.id, r.title)).color(color);
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
            let (text, color) = if req.is_blocked() {
                ("未解锁 — AI 不得编写/修改源码", egui::Color32::RED)
            } else {
                ("已解锁 — AI 可以开始编写代码", egui::Color32::GREEN)
            };
            ui.colored_label(color, text);
        });
        ui.label(format!(
            "进度 {}/3 已通过 · 评论 {} 条（阻塞 {}）",
            req.approved_count(),
            req.open_comments,
            req.blocking_comments
        ));
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, s) in req.steps.iter().enumerate() {
                let (mark, color) = match s.state {
                    StepState::Approved => ("✓", egui::Color32::GREEN),
                    StepState::Rejected => ("✗", egui::Color32::RED),
                    StepState::Pending => ("○", egui::Color32::GRAY),
                };
                let who = s
                    .reviewer
                    .clone()
                    .map(|v| format!("  审核人 {} ", v))
                    .unwrap_or_default();
                let header = egui::RichText::new(format!(
                    "[{}] {}. {}  {}{}",
                    mark,
                    i + 1,
                    s.label,
                    s.state.label(),
                    who
                ))
                .color(color);

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
                            let mut raw = text;
                            add_raw_text(ui, &mut raw);
                        }
                    });
                if resp.header_response.clicked() {
                    app.step = i;
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
                ui.separator();
            }
        });
    });
}

/// 原文视图：只读、可滚动、可复制，等宽逐字呈现 Markdown 源（**不可编辑**：
/// 清单正文由 AI/编辑器维护，界面只做审核决策）。
fn add_raw_text(ui: &mut egui::Ui, text: &mut String) {
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            ui.add_sized(
                [ui.available_width(), 200.0],
                egui::TextEdit::multiline(text)
                    .font(egui::TextStyle::Monospace)
                    .interactive(false),
            );
        });
}

fn render_dialogs(ctx: &egui::Context, app: &mut App) {
    let mut open = app.dialog != Dialog::None;
    if !open {
        return;
    }

    let (title, body): (&str, fn(&mut App, &mut egui::Ui)) = match app.dialog {
        Dialog::NewReq => ("创建需求", |app, ui| {
            ui.label("标题：");
            ui.text_edit_singleline(&mut app.input_title);
            if ui.button("创建").clicked() {
                app.create_req();
            }
        }),
        Dialog::Approve => ("批准当前段", |app, ui| {
            ui.label("审核人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            if ui.button("确认批准").clicked() {
                app.approve();
            }
        }),
        Dialog::Reject => ("打回当前段", |app, ui| {
            ui.label("审核人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            ui.label("原因（必填，将同时写入阻塞性评论）：");
            ui.text_edit_singleline(&mut app.input_reason);
            if ui.button("确认打回").clicked() {
                app.reject();
            }
        }),
        Dialog::Bypass => ("应急绕过", |app, ui| {
            ui.label("原因（必填，用于审计追溯）：");
            ui.text_edit_singleline(&mut app.input_reason);
            ui.label("默认时效 60 分钟，到期自动恢复硬拦截。");
            if ui.button("开启绕过").clicked() {
                app.do_bypass();
            }
        }),
        Dialog::Audit => ("审计日志（最近 200 行，倒序）", |app, ui| {
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    if app.audit.is_empty() {
                        ui.label("（暂无审计记录）");
                    } else {
                        for line in app.audit.iter().rev() {
                            ui.monospace(line);
                        }
                    }
                });
        }),
        Dialog::CheckResult => ("门禁检查结果", |app, ui| {
            let (text, color) = if app.check_pass {
                ("放行 ✅", egui::Color32::GREEN)
            } else {
                ("拦截 ⛔", egui::Color32::RED)
            };
            ui.colored_label(color, egui::RichText::new(text).strong());
            ui.separator();
            for line in app.check_detail.iter() {
                ui.label(line);
            }
        }),
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

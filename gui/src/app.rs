//! GUI 状态机与渲染。
//!
//! 所有动作都落到 `req-guard-core`：审核走 `requirement::review`、新建走 `requirement::create`、
//! 绕过走 `gate::bypass`、判定走 `gate::gate_check`——与 CLI / TUI 完全等价。

use eframe::egui;
use req_guard_core::status::{self, ReqStatus, StepState};
use req_guard_core::{comment, gate, requirement};

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
    /// 当前需求正文（只读展示）。
    pub body: String,
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
        self.body = match self.current() {
            Some(r) => {
                std::fs::read_to_string(&r.path).unwrap_or_else(|e| format!("读取正文失败：{}", e))
            }
            None => String::new(),
        };
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
        match requirement::review(&self.root, &id, step, &reviewer, true, "", strict) {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.reload();
                self.message = Some(format!("已批准 {} / {}", id, requirement::step_label(step)));
            }
            Err(e) => self.message = Some(format!("批准失败：{}", e)),
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
        match requirement::review(&self.root, &id, step, &reviewer, false, &reason, strict) {
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
        match gate::bypass(&self.root, &reason, "gui", 60) {
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
                self.check_detail = v.detail().to_vec();
                if self.check_detail.is_empty() {
                    self.check_detail.push(v.summary().to_string());
                }
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

                egui::CollapsingHeader::new(header)
                    .open(Some(i == app.step))
                    .show(ui, |ui| {
                        // 正文只读：清单正文由 AI/编辑器维护，界面只做审核决策。
                        let mut text = app.body.clone();
                        add_sized_text_edit(ui, &mut text);
                    });

                // 当前段的审核按钮
                if i == app.step {
                    let can = req.can_review(s.key);
                    ui.horizontal(|ui| {
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
                    });
                }
                ui.separator();
            }
        });
    });
}

/// 只读正文框：可滚动、可复制，但**不可编辑**（清单正文由 AI/编辑器维护）。
fn add_sized_text_edit(ui: &mut egui::Ui, text: &mut String) {
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

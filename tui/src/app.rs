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
        }
    }
}

/// 覆盖层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Audit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Requirements,
    Steps,
    Body,
}

/// TUI 全局状态。
pub struct App {
    pub focus: Focus,
    document: Vec<String>,
    pub root: PathBuf,
    pub reqs: Vec<ReqStatus>,
    /// 选中的需求下标。
    pub selected: usize,
    /// 选中的步骤下标（0..3）。
    pub step: usize,
    /// 当前需求正文（只读展示）。
    pub body: Vec<String>,
    pub body_scroll: u16,
    pub overlay: Overlay,
    /// 审计日志行（打开 `L` 时加载）。
    pub audit: Vec<String>,
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
            focus: Focus::Requirements,
            document: Vec::new(),
            root: root.to_path_buf(),
            reqs: Vec::new(),
            selected: 0,
            step: 0,
            body: Vec::new(),
            body_scroll: 0,
            overlay: Overlay::None,
            audit: Vec::new(),
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
        self.last_refresh = Instant::now();
    }

    /// 加载当前需求正文（只读）。切段时直接在本段起始处打开正文。
    pub fn load_body(&mut self) {
        self.body.clear();
        self.body_scroll = 0;
        let Some(r) = self.current() else { return };
        match std::fs::read_to_string(&r.path) {
            Ok(text) => {
                self.document = text.lines().map(|l| l.to_string()).collect();
                self.body = section_lines(&self.document, self.step);
            }
            Err(e) => {
                self.document = vec![format!("读取正文失败：{}", e)];
                self.body = self.document.clone();
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
        if self.overlay != Overlay::None {
            // 任意键关闭覆盖层。
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
                Focus::Body => {
                    self.body_scroll = self.body_scroll.saturating_sub(1);
                    self.clamp_scroll();
                }
                Focus::Steps => {}
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Requirements => self.move_selection(1),
                Focus::Body => {
                    self.body_scroll = self.body_scroll.saturating_add(1);
                    self.clamp_scroll();
                }
                Focus::Steps => {}
            },
            KeyCode::Left | KeyCode::Char('h') => self.move_step(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.move_step(1),
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
            _ => {}
        }
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
            Prompt::BypassReason => {
                if value.is_empty() {
                    self.message = Some("应急绕过必须填写原因".into());
                    return;
                }
                let actor = "tui";
                match gate::bypass(&self.root, &value, actor, 60) {
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

        match requirement::review(&self.root, &id, step, &reviewer, !reject, &reason, strict) {
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

/// 按步骤下标截取对应段落（`## 1. 需求分解` / `## 2. 技术方案` / `## 3. 测试计划`）。
/// 找不到标题时回退到整篇文档，保证老格式清单也能显示。
fn section_lines(document: &[String], step: usize) -> Vec<String> {
    let Some(start) = document
        .iter()
        .position(|l| l.trim_start().starts_with(&format!("## {}", step + 1)))
    else {
        return document.to_vec();
    };
    let end = document[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with("## "))
        .map_or(document.len(), |i| start + 1 + i);
    document[start..end].to_vec()
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

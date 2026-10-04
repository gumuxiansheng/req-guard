//! TUI 渲染层：只负责把 `App` 的状态画出来，不做任何判定。

use crate::app::{App, Focus, Overlay, Prompt};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use req_guard_core::comment::{Comment, CommentState};
use req_guard_core::requirement;
use req_guard_core::status::{ReqStatus, StepState};

/// 渲染一帧。
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(4),
        ])
        .split(area);

    render_header(f, app, rows[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(rows[1]);

    render_list(f, app, cols[0]);
    render_detail(f, app, cols[1]);
    render_footer(f, app, rows[2]);

    match app.overlay {
        Overlay::Help => render_help(f, area),
        Overlay::Audit => render_audit(f, app, area),
        Overlay::Comments => render_comments(f, app, area),
        Overlay::None => {}
    }
    if let Some(prompt) = app.prompt {
        render_prompt(f, app, prompt, area);
    }
}

/// 面板外框：聚焦时用青色边框 + 标题前缀 `▶`。
///
/// 必须有可见的焦点指示——否则用户按 `↑↓` 却不知道"现在动的是谁"，
/// 只能靠试错（正是"上下键没反应"的观感来源之一）。
fn pane(title: &str, focused: bool) -> Block<'static> {
    let t = if focused {
        format!(" ▶{}", title)
    } else {
        format!("  {}", title)
    };
    Block::default()
        .borders(Borders::ALL)
        .title(t)
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        })
}

/// 面板是否处于聚焦状态。
fn focused(app: &App, f: Focus) -> bool {
    app.focus == f
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let (flag, color) = match app.current() {
        None => ("无需求", Color::Gray),
        Some(r) if r.is_blocked() => ("未解锁", Color::Red),
        Some(_) => ("已解锁", Color::Green),
    };
    let line = Line::from(vec![
        Span::styled(
            " req-guard 门禁管理台 ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("· 项目: "),
        Span::styled(
            app.root.display().to_string(),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" · "),
        Span::styled(flag, Style::default().fg(color)),
    ]);
    f.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .reqs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let dot = if r.is_blocked() { "●" } else { "○" };
            let style = if i == app.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(format!("{} {} {}", dot, r.id, r.title))).style(style)
        })
        .collect();

    let list = List::new(items).block(pane(
        &format!(" 需求列表（{}） ", app.reqs.len()),
        focused(app, Focus::Requirements),
    ));
    f.render_widget(list, area);
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.current() else {
        let hint = Paragraph::new(
            "还没有需求清单。\n\n按 n 新建一条；或先在命令行执行：\n\n  req-guard create -t \"<需求标题>\"",
        )
        .block(Block::default().borders(Borders::ALL).title(" 详情 "));
        f.render_widget(hint, area);
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(area);

    render_req_title(f, r, rows[0]);
    render_steps(f, app, r, rows[1]);
    render_body(f, app, r, rows[2]);
}

fn render_req_title(f: &mut Frame, r: &ReqStatus, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            format!("{} {}", r.id, r.title),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("    状态: "),
        Span::styled(
            r.state.label().to_string(),
            Style::default().fg(color_of_req(r)),
        ),
    ]);
    f.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_steps(f: &mut Frame, app: &App, r: &ReqStatus, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    for (i, s) in r.steps.iter().enumerate() {
        let (mark, color) = match s.state {
            StepState::Approved => ("[✓]", Color::Green),
            StepState::Rejected => ("[✗]", Color::Red),
            // 修订与驳回都需要重新批准，颜色一致；标记不同以示"这是改稿不是否决"
            StepState::Amended => ("[~]", Color::Red),
            StepState::Pending => ("[ ]", Color::Gray),
        };
        let cursor = if i == app.step { "▶ " } else { "  " };
        let mut spans = vec![
            Span::styled(cursor.to_string(), Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{} {}. {}  ", mark, i + 1, s.label),
                Style::default().fg(color),
            ),
            Span::styled(s.state.label().to_string(), Style::default().fg(color)),
        ];
        if let Some(who) = &s.reviewer {
            spans.push(Span::raw(format!("  审核人: {}", who)));
        }
        if i == app.step {
            for sp in spans.iter_mut() {
                *sp = sp.clone().style(sp.style.add_modifier(Modifier::BOLD));
            }
        }
        lines.push(Line::from(spans));
    }

    let block = pane(
        &format!(" 三段审核（{}/3 已通过） ", r.approved_count()),
        focused(app, Focus::Steps),
    );
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_body(f: &mut Frame, app: &App, r: &ReqStatus, area: Rect) {
    let lines: Vec<Line> = app.body.iter().map(|l| Line::from(l.clone())).collect();
    // 标题点明"这是第几段"，与三段列表的选中项一一对应，防止看错段。
    let title = if r.open_comments > 0 {
        format!(
            " 正文（只读）· {}. {} · 评论 {} 条（阻塞 {}） ",
            app.step + 1,
            r.steps.get(app.step).map(|s| s.label).unwrap_or("未知段"),
            r.open_comments,
            r.blocking_comments
        )
    } else {
        format!(
            " 正文（只读）· {}. {} ",
            app.step + 1,
            r.steps.get(app.step).map(|s| s.label).unwrap_or("未知段")
        )
    };
    let p = Paragraph::new(lines)
        .block(pane(&title, focused(app, Focus::Body)))
        .wrap(Wrap { trim: false })
        .scroll((app.body_scroll, 0));
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.message {
        Some(m) => Line::from(Span::styled(m.clone(), Style::default().fg(Color::Yellow))),
        None => Line::from(
            " a批准 r打回 m评论 n新建 g检查 b绕过 L审计 R刷新 ↑↓选择 ←→/Tab换焦点 ?帮助 q退出 ",
        ),
    };
    // 折行显示：终端窄时也不至于把「q退出」这类关键提示裁掉。
    f.render_widget(
        Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let popup = centered(area, 70, 16);
    f.render_widget(Clear, popup);
    let text = vec![
        Line::from(""),
        Line::from("  键位（↑↓ 作用于带 ▶ 的面板）"),
        Line::from("    ↑/k  ↓/j          在聚焦面板内选择：需求 / 三段 / 正文滚动"),
        Line::from("    ←/h  →/l  Tab     切换焦点：需求列表 → 三段 → 正文（Shift+Tab 反向）"),
        Line::from("    PageUp/PageDown   滚动正文（无需先切焦点）"),
        Line::from(""),
        Line::from("  操作"),
        Line::from("    a  批准当前段（输入审核人）"),
        Line::from("    r  打回当前段（审核人 + 原因必填）"),
        Line::from("    m  评论面板：查看 / 新增（n 普通 · N 阻塞）/ 关闭 x / 重算锚点 A"),
        Line::from("    n  新建需求（输入标题）"),
        Line::from("    g  执行门禁检查（与 CLI req-guard check 等价）"),
        Line::from("    b  应急绕过（原因必填，默认 60 分钟，写审计）"),
        Line::from("    L  查看审计日志"),
        Line::from("    R  刷新          ?  本帮助          q/Esc 退出"),
        Line::from(""),
        Line::from("  任意键关闭本帮助"),
    ];
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" 帮助 ")),
        popup,
    );
}

fn render_audit(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 84, 20);
    f.render_widget(Clear, popup);
    let lines: Vec<Line> = if app.audit.is_empty() {
        vec![Line::from("（暂无审计记录）")]
    } else {
        app.audit
            .iter()
            .rev()
            .map(|l| Line::from(l.clone()))
            .collect()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 审计日志（最近 200 行，倒序）— 任意键关闭 ");
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// 评论面板：唯一**可交互**的覆盖层（其余覆盖层任意键即关）。
///
/// 每条评论固定占 2 行（1 行头 + 1 行正文，**不折行**、超长截断）：
/// 这样"选中项 → 滚动位置"是确定算术（见 app 里的 `comment_top_line`），
/// 而折行会让行数依赖终端宽度，滚动就再也算不准。
fn render_comments(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 88, 24);
    f.render_widget(Clear, popup);

    let open = app
        .comments
        .iter()
        .filter(|c| c.state == CommentState::Open)
        .count();
    let blocking = app.comments.iter().filter(|c| c.is_blocking_open()).count();

    let mut lines: Vec<Line> = Vec::new();
    if blocking > 0 {
        lines.push(Line::from(Span::styled(
            " ⛔ 有未关闭的阻塞评论 —— AI 不得编写代码（门禁会拦）",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));

    if app.comments.is_empty() {
        lines.push(Line::from(
            " （还没有评论。审核人只评论、不改正文；打回会自动留一条阻塞评论。）",
        ));
    }
    for (i, c) in app.comments.iter().enumerate() {
        lines.push(comment_header(app, i, c));
        lines.push(comment_body(c));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " n 新增普通评论  N 新增阻塞评论  x 关闭选中  A 重算行号锚点  ↑↓/jk 选择  Esc/m/q 关闭 ",
        Style::default().fg(Color::Cyan),
    )));

    // 计数放在**标题**里而不是正文首行：面板会滚动，首行迟早被顶出去，
    // 而"还有几条没解决、几条阻塞"是审核人最需要一直看得见的信息。
    let sel = if app.comments.is_empty() {
        String::new()
    } else {
        format!(" · 选中 {}/{}", app.comment_sel + 1, app.comments.len())
    };
    let block = Block::default().borders(Borders::ALL).title(format!(
        " 审核评论 · {} · 共 {} 未解决 {} 阻塞 {}{} ",
        app.current().map(|r| r.id.as_str()).unwrap_or("（无需求）"),
        app.comments.len(),
        open,
        blocking,
        sel
    ));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.comment_scroll, 0)),
        popup,
    );
}

/// 一条评论的头：ID / 段 / 状态 / 阻塞 / 作者 / 时间 / 行号。
fn comment_header(app: &App, i: usize, c: &Comment) -> Line<'static> {
    let (state_mark, state_color) = match c.state {
        CommentState::Open => ("● open", Color::Yellow),
        CommentState::Resolved => ("✓ resolved", Color::Green),
    };
    let mut spans = vec![
        Span::raw(if i == app.comment_sel { "▶ " } else { "  " }),
        Span::styled(
            format!("[{}] ", c.id),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(state_mark.to_string(), Style::default().fg(state_color)),
        Span::raw(format!(
            " · {}",
            c.step.as_deref().map_or("总评", requirement::step_label)
        )),
    ];
    if c.blocking {
        spans.push(Span::styled(
            " · 阻塞".to_string(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(format!(" · {} · {}", c.author, c.ts)));
    if let Some(n) = c.line {
        spans.push(Span::raw(format!(" · L{}", n)));
    }
    if c.stale {
        spans.push(Span::styled(
            " · ⚠ 锚点失效".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(r) = &c.reply {
        spans.push(Span::raw(format!(" · 回复 {}", r)));
    }
    Line::from(spans)
}

/// 一条评论的正文（压成单行 + 截断，保证"一条评论 = 2 行"这个不变式）。
fn comment_body(c: &Comment) -> Line<'static> {
    let text = c.body.replace('\n', " ");
    let mut body = text.trim().to_string();
    if body.chars().count() > 110 {
        body = body.chars().take(110).collect::<String>() + "…";
    }
    let mut spans = vec![Span::raw("    ")];
    if let Some(q) = &c.quote {
        spans.push(Span::styled(
            format!("“{}” ", q),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::raw(body));
    Line::from(spans)
}

fn render_prompt(f: &mut Frame, app: &App, prompt: Prompt, area: Rect) {
    let popup = centered(area, 64, 5);
    f.render_widget(Clear, popup);
    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("> "),
            Span::styled(app.input.clone(), Style::default().fg(Color::Cyan)),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", prompt.title()));
    f.render_widget(Paragraph::new(text).block(block), popup);
}

/// 计算居中弹窗区域。
fn centered(area: Rect, percent_x: u16, height: u16) -> Rect {
    let height = height.min(area.height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);
    let percent_y = percent_x.min(100);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn color_of_req(r: &ReqStatus) -> Color {
    if r.is_blocked() {
        Color::Red
    } else {
        Color::Green
    }
}

// ===================== 渲染冒烟测试（TestBackend，无需真实终端） =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Focus};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "req-guard-tui-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("创建临时目录");
        p
    }

    /// 把终端缓冲摊平成文本，便于断言。
    ///
    /// 注意：宽字符（中文等）在缓冲里占两个 cell，后一个 cell 是空格占位符。
    /// 必须按显示宽度跳格，否则会得到"登 录 改 造"这种被拆开的文本，断言必然误判。
    fn screen(terminal: &Terminal<TestBackend>) -> String {
        use unicode_width::UnicodeWidthStr;

        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                out.push_str(sym);
                x += UnicodeWidthStr::width(sym).max(1) as u16;
            }
            out.push('\n');
        }
        if std::env::var("TUI_DEBUG").is_ok() {
            eprintln!("{}", out);
        }
        out
    }

    fn draw(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("测试终端");
        terminal.draw(|f| render(f, app)).expect("渲染一帧");
        terminal
    }

    #[test]
    fn 空项目渲染引导提示() {
        let root = temp_dir("empty");
        let app = App::new(&root);
        let text = screen(&draw(&app, 100, 30));
        assert!(text.contains("req-guard 门禁管理台"), "应显示管理台标题");
        assert!(text.contains("需求列表"), "列表标题是恒定骨架");
        assert!(text.contains("还没有需求清单"), "空项目要给出下一步指引");
        assert!(
            text.contains("q退出"),
            "底部键位提示常驻（窄终端折行也不能丢）"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 有需求时渲染三段与状态() {
        let root = temp_dir("has-req");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let app = App::new(&root);
        let text = screen(&draw(&app, 120, 34));
        assert!(text.contains("REQ-001"));
        assert!(text.contains("登录改造"));
        assert!(text.contains("需求分解"), "三段名称应逐一列出");
        assert!(text.contains("技术方案"));
        assert!(text.contains("测试计划"));
        assert!(text.contains("未解锁"), "三段未过时应显示未解锁");
        assert!(text.contains("0/3 已通过"), "进度应显示 0/3");
        // 焦点指示：默认聚焦三段，其面板标题带 ▶（否则用户按 ↑↓ 不知道在动谁）
        assert!(
            text.contains("▶ 三段审核"),
            "聚焦面板须有可见标记：\n{}",
            text
        );
        // 正文面板标题点明当前段，且正文只含该段内容
        assert!(
            text.contains("正文（只读）· 1. 需求分解"),
            "正文标题应标明段号"
        );
        assert!(
            !text.contains("总体思路"),
            "第一段视图里不应出现第二段的正文：\n{}",
            text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 换段后正文面板只显示该段() {
        let root = temp_dir("section-view");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let text = screen(&draw(&app, 120, 34));
        assert!(
            text.contains("正文（只读）· 2. 技术方案"),
            "标题应切到第二段"
        );
        assert!(text.contains("总体思路"), "应显示第二段正文");
        assert!(
            !text.contains("验收标准"),
            "不应把第一段正文一起显示出来：\n{}",
            text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 三段通过后显示已解锁() {
        let root = temp_dir("unlocked");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        // 三段都要有实质正文才批得过（REQ-003 的 EmptySection 挂在 approve 上）。
        // core 的 `testutil` 是 `#[cfg(test)]`，TUI 用不了，故就地填 —— 夹具重复
        // 三行比把测试专用助手暴露成公开 API 划算。
        {
            let p = req_guard_core::requirement::find(&root, "REQ-001")
                .expect("清单应存在")
                .path;
            let mut c = std::fs::read_to_string(&p).expect("清单应可读");
            for (heading, line) in [
                ("## 1. 需求分解", "- 背景与问题：TUI 渲染夹具。"),
                ("## 2. 技术方案", "- 总体思路：渲染三段状态。"),
                ("## 3. 测试计划", "- 验收门槛：屏幕出现「已解锁」。"),
            ] {
                let needle = format!("{heading}\n");
                assert!(c.contains(&needle), "模板结构变了：{heading}");
                c = c.replacen(&needle, &format!("{needle}{line}\n"), 1);
            }
            std::fs::write(&p, c).expect("写入应成功");
        }
        for step in ["decomposition", "solution", "testplan"] {
            req_guard_core::requirement::review(&root, "REQ-001", step, "寇工", true, "", true)
                .expect("审核");
        }
        let app = App::new(&root);
        let text = screen(&draw(&app, 120, 34));
        assert!(text.contains("已解锁"));
        assert!(text.contains("3/3 已通过"));
        assert!(text.contains("寇工"), "应显示审核人");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 帮助与审计覆盖层可渲染() {
        let root = temp_dir("overlay");
        let mut app = App::new(&root);
        app.overlay = Overlay::Help;
        let help = screen(&draw(&app, 110, 30));
        assert!(help.contains("键位"));
        assert!(help.contains("执行门禁检查"));

        app.overlay = Overlay::Audit;
        app.audit = vec!["2026-09-11 10:00:00 PASS REQ-001.md".to_string()];
        let audit = screen(&draw(&app, 110, 30));
        assert!(audit.contains("审计日志"));
        assert!(audit.contains("PASS REQ-001.md"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 输入弹窗渲染提示与内容() {
        let root = temp_dir("prompt");
        let mut app = App::new(&root);
        app.prompt = Some(Prompt::NewTitle);
        app.input = "用户登录改造".to_string();
        let text = screen(&draw(&app, 110, 30));
        assert!(text.contains("新建需求"), "弹窗标题应说明在收集什么");
        assert!(text.contains("用户登录改造"), "输入内容应回显");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 切换步骤后正文定位到对应段落() {
        let root = temp_dir("section-jump");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        // 默认聚焦三段：↑↓ 直接换段（这正是历史缺陷：焦点从未被赋值，
        // 于是 ↑↓ 只动需求列表、换段只能靠 ←→）
        assert_eq!(app.focus, Focus::Steps, "进来就该聚焦三段");
        assert!(
            app.body.iter().any(|l| l.contains("需求分解")),
            "初始应显示第一段"
        );
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.step, 1, "↓ 应切到第二段");
        assert!(
            app.body.iter().any(|l| l.contains("技术方案")),
            "切段后应定位到该段正文"
        );
        assert!(
            !app.body.iter().any(|l| l.contains("测试计划")),
            "第二段不应混入第三段内容"
        );
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.step, 0, "↑ 应回到第一段");
        assert!(
            app.body.iter().any(|l| l.contains("需求分解")),
            "← 应回到第一段"
        );
        // 边界：第一段再按 ↑ 不越界、也不跳到需求列表
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!((app.step, app.selected), (0, 0), "段首 ↑ 应停在原地");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 左右键与tab只切换焦点_不再直接切段() {
        let root = temp_dir("focus-cycle");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        assert_eq!(app.focus, Focus::Steps);

        // → 到正文：只换焦点，段不变
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!((app.focus, app.step), (Focus::Body, 0));
        // ↓ 在正文聚焦时滚动，而不是换段
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!((app.step, app.body_scroll), (0, 1), "正文聚焦时 ↓ 滚动正文");
        // → 回绕到需求列表；此时 ↓ 才动需求列表（只有一条，故停在原地）
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Requirements);
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!((app.selected, app.step), (0, 0), "单条需求时不动");
        // Tab 正向、Shift+Tab（BackTab）反向
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Steps);
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.focus, Focus::Requirements, "Shift+Tab 应反向回绕");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 多需求时上下键切换需求() {
        let root = temp_dir("req-nav");
        req_guard_core::requirement::create(&root, None, "第一条").expect("创建需求");
        req_guard_core::requirement::create(&root, None, "第二条").expect("创建需求");
        let mut app = App::new(&root);
        // 需求列表按文件名/id 排序，找到"第二条"的位置再断言跳转结果
        let second = app
            .reqs
            .iter()
            .position(|r| r.title == "第二条")
            .expect("应包含第二条");
        app.focus = Focus::Requirements;
        for _ in 0..second {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert_eq!(app.selected, second, "↓ 应逐条下移");
        assert_eq!(app.step, 0, "换需求后段游标归零");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 刷新保留选区与滚动位置() {
        let root = temp_dir("refresh-keep");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.body_scroll = 2;
        app.reload();
        assert_eq!(app.step, 1, "自动刷新不应重置当前段");
        assert_eq!(app.body_scroll, 2, "自动刷新不应重置滚动位置");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 逐字符输入（模拟真人打字）。
    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn press(app: &mut App, c: char) {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    fn enter(app: &mut App) {
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    /// 建一条评论（走 core，界面只负责读）。
    fn add_comment(root: &std::path::Path, blocking: bool, text: &str) {
        req_guard_core::comment::add(
            root,
            "REQ-001",
            req_guard_core::comment::NewComment {
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
    fn 评论面板列出评论内容与阻塞标记() {
        let root = temp_dir("comments-view");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, true, "回滚方案需补充 DB 迁移回退");
        let mut app = App::new(&root);

        press(&mut app, 'm');
        assert_eq!(app.overlay, Overlay::Comments, "m 应打开评论面板");
        assert_eq!(app.comments.len(), 1);

        let text = screen(&draw(&app, 120, 34));
        assert!(text.contains("审核评论"), "面板标题：\n{}", text);
        assert!(text.contains("C001"), "应显示评论 ID");
        assert!(
            text.contains("回滚方案需补充 DB 迁移回退"),
            "应显示评论正文：\n{}",
            text
        );
        assert!(text.contains("阻塞"), "阻塞评论要有醒目标记");
        assert!(
            text.contains("AI 不得编写代码"),
            "有未关闭阻塞评论时要写明后果：\n{}",
            text
        );
        assert!(text.contains("技术方案"), "应显示锚定的段名：\n{}", text);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 面板内新增评论走core并刷新列表() {
        let root = temp_dir("comments-add");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        press(&mut app, 'm');

        // N = 新增阻塞评论；随后两个弹窗：作者 → 内容
        press(&mut app, 'N');
        type_text(&mut app, "寇工");
        enter(&mut app);
        type_text(&mut app, "缺少失败锁定");
        enter(&mut app);

        let list = req_guard_core::comment::list(&root, "REQ-001").expect("读评论");
        assert_eq!(list.len(), 1, "评论应落盘");
        assert!(list[0].is_blocking_open(), "N 加的是阻塞评论");
        assert_eq!(list[0].author, "寇工");
        assert_eq!(
            list[0].step.as_deref(),
            Some("decomposition"),
            "评论锚定**当前选中段**（此时是第 1 段）"
        );
        assert_eq!(app.comments.len(), 1, "面板列表应已刷新");
        assert!(
            app.message.as_deref().unwrap_or("").contains("已添加"),
            "应给出结果提示：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 面板内关闭评论需审核人且改为resolved() {
        let root = temp_dir("comments-resolve");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, true, "回滚方案需补充 DB 迁移回退");
        let mut app = App::new(&root);
        press(&mut app, 'm');
        assert_eq!(app.comments.len(), 1);

        // x 打开"关闭人"弹窗；审核人不能为空
        press(&mut app, 'x');
        enter(&mut app);
        assert!(
            req_guard_core::comment::list(&root, "REQ-001").unwrap()[0].state
                == req_guard_core::comment::CommentState::Open,
            "关闭人为空时不应关闭"
        );

        type_text(&mut app, "寇工");
        enter(&mut app);
        let list = req_guard_core::comment::list(&root, "REQ-001").expect("读评论");
        assert_eq!(
            list[0].state,
            req_guard_core::comment::CommentState::Resolved,
            "审核人署名后应关闭"
        );
        assert!(
            !list[0].is_blocking_open(),
            "关闭后不再阻塞（门禁据此放行）"
        );
        let text = screen(&draw(&app, 120, 34));
        assert!(
            text.contains("resolved"),
            "面板应显示已关闭状态：\n{}",
            text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 面板内重算锚点标记失效行号() {
        let root = temp_dir("comments-anchor");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        // quote 指向正文中并不存在的片段 → 应被标 stale
        req_guard_core::comment::add(
            &root,
            "REQ-001",
            req_guard_core::comment::NewComment {
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
        press(&mut app, 'm');
        press(&mut app, 'A');
        let list = req_guard_core::comment::list(&root, "REQ-001").unwrap();
        assert!(list[0].stale, "找不到原文的引用应标 stale");
        assert!(
            app.message.as_deref().unwrap_or("").contains("stale"),
            "应提示有几条失效：{:?}",
            app.message
        );
        let text = screen(&draw(&app, 120, 34));
        assert!(text.contains("锚点失效"), "面板应标出失效：\n{}", text);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 面板按键不会误触发全局动作() {
        let root = temp_dir("comments-keys");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, false, "普通意见");
        let mut app = App::new(&root);
        press(&mut app, 'm');
        // 面板开着时 a/r/n/g 不应生效（否则一个字母就误批准/误打回）
        press(&mut app, 'a');
        assert_eq!(app.step, 0);
        assert!(app.prompt.is_none(), "面板内的 a 不该触发批准弹窗");
        // Esc 关闭面板，且不退出程序
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.should_quit, "Esc 只关面板，不退出");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 上下键在正文聚焦时滚动() {
        let root = temp_dir("body-scroll");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        app.focus = Focus::Body;
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.body_scroll, 1, "正文聚焦时 ↓ 应滚动正文");
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.body_scroll, 0, "↑ 应回滚");
        let _ = std::fs::remove_dir_all(&root);
    }
}

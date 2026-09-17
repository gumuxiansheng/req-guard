//! TUI 渲染层：只负责把 `App` 的状态画出来，不做任何判定。

use crate::app::{App, Overlay, Prompt};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
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
        Overlay::None => {}
    }
    if let Some(prompt) = app.prompt {
        render_prompt(f, app, prompt, area);
    }
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

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 需求列表（{}） ", app.reqs.len())),
    );
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

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 三段审核（{}/3 已通过） ", r.approved_count()));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_body(f: &mut Frame, app: &App, r: &ReqStatus, area: Rect) {
    let lines: Vec<Line> = app.body.iter().map(|l| Line::from(l.clone())).collect();
    let title = if r.open_comments > 0 {
        format!(
            " 正文（只读）· 评论 {} 条（阻塞 {}） ",
            r.open_comments, r.blocking_comments
        )
    } else {
        " 正文（只读） ".to_string()
    };
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.body_scroll, 0));
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.message {
        Some(m) => Line::from(Span::styled(m.clone(), Style::default().fg(Color::Yellow))),
        None => Line::from(" a批准 r打回 n新建 g检查 b绕过 L审计 R刷新 ↑↓选择 ←→切段 ?帮助 q退出 "),
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
        Line::from("  键位"),
        Line::from("    ↑/k  ↓/j          选择需求"),
        Line::from("    ←/h  →/l  Tab     切换三段（正文随之切换）"),
        Line::from("    PageUp/PageDown   滚动正文"),
        Line::from(""),
        Line::from("  操作"),
        Line::from("    a  批准当前段（输入审核人）"),
        Line::from("    r  打回当前段（审核人 + 原因必填）"),
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 三段通过后显示已解锁() {
        let root = temp_dir("unlocked");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
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
        assert!(
            app.body.iter().any(|l| l.contains("需求分解")),
            "初始应显示第一段"
        );
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(
            app.body.iter().any(|l| l.contains("技术方案")),
            "→ 应切到第二段并定位正文"
        );
        assert!(
            !app.body.iter().any(|l| l.contains("测试计划")),
            "第二段不应混入第三段内容"
        );
        app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(
            app.body.iter().any(|l| l.contains("需求分解")),
            "← 应回到第一段"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 刷新保留选区与滚动位置() {
        let root = temp_dir("refresh-keep");
        req_guard_core::requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.body_scroll = 2;
        app.reload();
        assert_eq!(app.step, 1, "自动刷新不应重置当前段");
        assert_eq!(app.body_scroll, 2, "自动刷新不应重置滚动位置");
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

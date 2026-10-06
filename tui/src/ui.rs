//! TUI 渲染层：只负责把 `App` 的状态画出来，不做任何判定。

use crate::app::{App, Focus, Overlay, Prompt, HEALTH_GROUPS};
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
        Overlay::Help => render_help(f, area, app),
        Overlay::Audit => render_audit(f, app, area),
        Overlay::Check => render_check(f, app, area),
        Overlay::Comments => render_comments(f, app, area),
        Overlay::Health => render_health(f, app, area),
        Overlay::Hygiene => render_hygiene(f, app, area),
        Overlay::DoneConfirm => render_done_confirm(f, app, area),
        Overlay::Archived => render_archived(f, app, area),
        Overlay::Archive => render_archive(f, app, area),
        Overlay::None => {}
    }
    if let Some(prompt) = app.prompt {
        render_prompt(f, app, prompt, area);
    }
}

/// 内容冻结徽标：`(标记, 颜色)`。与 GUI 同口径（都读 core 的
/// [`req_guard_core::requirement::SealState`]），两个界面显示一致。
fn seal_badge(seal: req_guard_core::requirement::SealState) -> Option<(&'static str, Color)> {
    use req_guard_core::requirement::SealState as S;
    match seal {
        S::Frozen | S::NotApplicable => None,
        S::NotSealed => Some(("[未绑定]", Color::Yellow)),
        S::Changed => Some(("[已改动]", Color::Red)),
        S::Unverifiable => Some(("[无法校验]", Color::Red)),
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
    // ⚠️ **状态旗标排在项目路径之前**，不是美学选择：顶栏固定只有一行内容
    // （`Constraint::Length(3)` 去掉上下边框），而项目路径可以很长。
    // 旗标放行尾时会被路径挤出可视区、**截断成半个词**（实测：120 列下
    // `已解锁` 被切成 `已解`）。旗标是这一行里最不能丢的信息——它回答
    // 「这份需求现在能不能写代码」，比"项目在哪"重要得多。
    let line = Line::from(vec![
        Span::styled(
            " req-guard 门禁管理台 ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("· "),
        Span::styled(flag, Style::default().fg(color)),
        Span::raw(" · 项目: "),
        Span::styled(
            app.root.display().to_string(),
            Style::default().fg(Color::Cyan),
        ),
    ]);
    // ⚠️ 身份区同样**不放这里**：120 列下再加 40+ 字会把路径挤掉。
    // 它挪到 footer 的空白行——那里有现成空间。
    f.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_list(f: &mut Frame, app: &App, area: Rect) {
    // 归档区视图：左栏换成历史清单，标题写明「只读」，选中项走 `archived_sel`。
    let (list, sel, title) = if app.browsing_archived {
        (
            app.archived.as_slice(),
            app.archived_sel,
            format!(" 归档区（{}，只读） ", app.archived.len()),
        )
    } else {
        (
            app.reqs.as_slice(),
            app.selected,
            format!(" 需求列表（{}） ", app.reqs.len()),
        )
    };
    let items: Vec<ListItem> = list
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let dot = if r.is_blocked() { "●" } else { "○" };
            let style = if i == sel {
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

    let list = List::new(items).block(pane(&title, focused(app, Focus::Requirements)));
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
        // 内容冻结异常写进本行：审核人扫一眼三段就该知道哪段不能放行。
        // `Frozen` / `NotApplicable` 不带标记 —— 正常状态不必再挂符号刷存在感。
        if let Some((m, c)) = seal_badge(s.seal) {
            spans.push(Span::styled(format!("  {}", m), Style::default().fg(c)));
        }
        if i == app.step {
            for sp in spans.iter_mut() {
                *sp = sp.clone().style(sp.style.add_modifier(Modifier::BOLD));
            }
        }
        lines.push(Line::from(spans));
    }

    // 面板标题顺带报"待处理段数"：按 s 绑定摘要这件事**不阻断门禁**，
    // 若不主动提示，它就会一直静默地挂在每份清单上直到没人再看。
    let pending_seal = r.steps.iter().filter(|s| s.seal.needs_action()).count();
    let title = if pending_seal > 0 {
        format!(
            " 三段审核（{}/3 已通过 · {pending_seal} 段待绑定摘要 s） ",
            r.approved_count()
        )
    } else {
        format!(" 三段审核（{}/3 已通过） ", r.approved_count())
    };
    let block = pane(&title, focused(app, Focus::Steps));
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
            " a批准 r打回 e修订 m评论 s绑定摘要 n新建 g检查 c看结果 b绕过 L审计 R刷新 ?帮助 q退出 ",
        ),
    };
    // 第二行放身份与鉴权等级（REQ-015 G6）：只为回答「我现在能不能审批」。
    // 放 footer 而不是顶栏，理由见 [`render_header`] 的注释（顶栏会被截断）。
    let who = Line::from(Span::styled(
        format!(" {}", app.who),
        Style::default().fg(Color::DarkGray),
    ));
    // 折行显示：终端窄时也不至于把「q退出」这类关键提示裁掉。
    f.render_widget(
        Paragraph::new(vec![line, who])
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let popup = centered(area, 76, 30);
    f.render_widget(Clear, popup);
    let text = vec![
        Line::from(""),
        // ⚠️ 这份文案刻意压得很紧：浮层高度有限，而**底部那节「当前环境」
        // （AC-021 要求它带 `sig=` 与 `auth`）必须始终可见**——
        // 早先每条键位各占一行的写法会把环境节挤出浮层（实测）。
        Line::from("  键位（↑↓ 作用于带 ▶ 的面板）"),
        Line::from("    ↑/k ↓/j 聚焦面板内选择   ←/h →/l/Tab 切焦点   PgUp/PgDn 滚正文"),
        Line::from("  审核"),
        Line::from("    a 批准（输审核人） r 打回（+原因） e 修订 s 绑定内容摘要"),
        Line::from("    m 评论面板（n 普通 · N 阻塞 · x 关闭 · A 重算锚点）"),
        Line::from("    g 门禁检查（按当前需求 + 暂存改动）  b 应急绕过（+原因，60 分钟）"),
        Line::from("  生命周期 / 体检 / 卫生"),
        Line::from("    d 归档（done）≠ 交付完成（有阻塞评论时按两次） A 预演 Shift+D 执行"),
        Line::from("    v 切归档区（只读）  i 身份与等级"),
        Line::from("    H 四组体检（1-4 切） W 追加范围声明 S 一键批量绑定 G 刷新审计摘要"),
        Line::from("    Y 工程卫生（1-3 切：编号冲突 / 门禁自检 / 环境）"),
        Line::from("    n 新建需求是**两步**：① 编号（回车=自动编号）→ ② 标题"),
        Line::from("    L 审计日志  c 复看检查结果  R 刷新  q/Esc 退出"),
        Line::from(""),
        Line::from("  当前环境（只读；本界面不提供票据签发 / 撤销）"),
    ];
    // 环境节放在**帮助浮层**而不是主界面：这是"偶尔看一眼"的信息，
    // 塞进主界面会挤占本已紧张的高度（三栏 + footer 都是固定的）。
    // AC-021 要求这里同时出现 `sig=` 与 `auth` 字样。
    let mut text = text;
    for l in app.env_lines() {
        text.push(Line::from(Span::styled(
            format!("    {l}"),
            Style::default().fg(Color::Gray),
        )));
    }
    text.push(Line::from(""));
    text.push(Line::from("  任意键关闭本帮助"));
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
            // 按 core 判定的性质上色（分类只有一份，见 gate::audit_kind）。
            // 拦截与绕过必须一眼可辨：把 BLOCK 显示成 PASS 就是"看着在放行"。
            .map(|l| {
                let color = match req_guard_core::gate::audit_kind(l) {
                    req_guard_core::gate::AuditKind::Block => Color::Red,
                    req_guard_core::gate::AuditKind::Bypass => Color::Yellow,
                    req_guard_core::gate::AuditKind::Note => Color::DarkGray,
                    req_guard_core::gate::AuditKind::Pass => Color::Green,
                    req_guard_core::gate::AuditKind::Event => Color::Gray,
                };
                Line::from(Span::styled(l.clone(), Style::default().fg(color)))
            })
            .collect()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 审计日志（最近 200 行，倒序）— 任意键关闭 ");
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// 门禁检查结果浮层（REQ-014 G4）。
///
/// **为什么是浮层而不是 footer**：footer 固定 4 行（外层布局的 `Constraint::Length(4)`，
/// 去掉上下边框只剩 2 行可用），而多需求仓库里 `Ambiguous` 会列出十几条候选清单——
/// 任何固定高度的 footer 都会截断掉用户最需要的处置建议。这里放浮层并可滚动，
/// footer 只留一行摘要（`App::run_check` 写的 `message`）。
///
/// 首两行是**口径**与**类别**：口径让用户知道「按谁判、用的哪份变更集」，
/// 类别用台账同名的稳定字面量，便于对照 `req-guard check` 的输出。
fn render_check(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 84, 22);
    f.render_widget(Clear, popup);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("口径：{}", app.check_scope),
            Style::default().fg(Color::Cyan),
        )),
        Line::from(if app.check_kind.is_empty() {
            Span::raw("结论：放行（无拦截类别）")
        } else {
            Span::styled(
                format!("类别：{}", app.check_kind),
                Style::default().fg(Color::Red),
            )
        }),
        Line::from(""),
    ];
    lines.extend(app.check_detail.iter().map(|l| Line::from(l.clone())));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓/jk 滚动 · PgUp/PgDn 翻页 · Home 顶部 · 任意键关闭 ",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 门禁检查结果 ");
    // 明细按逻辑行滚动：一条理由可能跨多行，用「占的行数」而不是「条数」定位。
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.check_scroll, 0)),
        popup,
    );
}

/// 工程卫生浮层（REQ-017）：三个分组用 `1`–`3` 切。
fn render_hygiene(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 88, 20);
    f.render_widget(Clear, popup);

    let (err, warn) = app.id_issue_counts();
    let tabs = ["① 编号冲突", "② 门禁自检", "③ 环境"];
    let mut tab_spans = Vec::new();
    for (i, name) in tabs.iter().enumerate() {
        let extra = if i == 0 {
            format!("（错误{err} 警告{warn}）")
        } else {
            String::new()
        };
        tab_spans.push(Span::styled(
            format!(" [{}] {}{} ", i + 1, name, extra),
            if i == app.hygiene_tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ));
    }

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "编号冲突 / 门禁自检 / 环境 —— 只读，不签发凭据",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(tab_spans),
        Line::from(""),
    ];
    let body = app.hygiene_lines();
    if body.is_empty() {
        lines.push(Line::from(Span::styled(
            "（本组没有问题）",
            Style::default().fg(Color::Green),
        )));
    }
    for l in body {
        let color = if l.contains('⚠') || l.contains("错误") {
            Color::Red
        } else if l.contains("警告") {
            Color::Yellow
        } else {
            Color::Gray
        };
        lines.push(Line::from(Span::styled(l, Style::default().fg(color))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " 1-3/Tab 切分组 · Y 重新检测 · 任意键关闭 ",
        Style::default().fg(Color::DarkGray),
    )));
    // 界面不提供票据写操作：这句话要出现在面板上，而不是只写在 README 里。
    lines.push(Line::from(Span::styled(
        " 本界面不提供票据签发 / 撤销（能自签票 = 「人类在场」这道门自己拆了） ",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 工程卫生（只读） ");
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// 体检浮层（REQ-016）：四个子标签用 `1`–`4` 切。
///
/// 严重级分色沿用既有做法（`gate::audit_kind` 那套）：错误红、警告黄。
/// 分色之外**每条都带前缀**（`[错误]` / `[警告]`）——色弱用户不靠颜色分辨，
/// 且离屏测试断言文本比断言 cell 颜色稳得多（后者是 `TestBackend` 的内部属性）。
fn render_health(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 88, 22);
    f.render_widget(Clear, popup);

    let counts = app.health_counts();
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "四组只读体检（不签发凭据、不消耗审批资格）",
        Style::default().fg(Color::DarkGray),
    ))];
    lines.push(Line::from(""));
    // 子标签行：当前标签高亮，并带上各自的错误/警告计数。
    let mut tab = Vec::new();
    for (i, name) in HEALTH_GROUPS.iter().enumerate() {
        let (e, w) = counts[i];
        tab.push(Span::styled(
            format!(" [{}] {}  错误{} 警告{} ", i + 1, name, e, w),
            if i == app.health_tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ));
    }
    lines.push(Line::from(tab));
    lines.push(Line::from(""));

    let body = app.health_lines();
    if body.is_empty() {
        lines.push(Line::from(Span::styled(
            "（本组没有发现问题）",
            Style::default().fg(Color::Green),
        )));
    }
    for l in body {
        // 前缀写在文本里，颜色只是加强（与 audit 面板同一纪律）。
        let color = if l.contains("错误") || l.contains("不存在") || l.contains("失败") {
            Color::Red
        } else if l.contains("警告") || l.contains("未绑定") || l.contains("已改动") {
            Color::Yellow
        } else {
            Color::Gray
        };
        lines.push(Line::from(Span::styled(l, Style::default().fg(color))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " 1-4/Tab 切子标签 · H 重新体检 · D 刷新审计摘要 · 任意键关闭 ",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 体检总览（只读） ");
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// 归档确认（REQ-015 G4）：三项事实 + 「再按一次 d」。
fn render_done_confirm(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 84, 14);
    f.render_widget(Clear, popup);

    let mut lines: Vec<Line> = app
        .done_confirm_lines()
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " 再按一次 d 确认（阻塞评论将随之失效）· 其它任意键取消 ",
        Style::default().fg(Color::Yellow),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 归档（done）确认 ");
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// 归档区（只读历史）列表（REQ-015 G3/T8）。
///
/// 标题里必须写「只读」：这份清单在门禁眼里**已经不存在**了，
/// 若界面看着跟 live 列表一样，用户会据此误判门禁行为。
fn render_archived(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 84, 20);
    f.render_widget(Clear, popup);

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "只读历史：归档即终点，此处不能批准 / 打回 / 修订 / 封存 / 归档",
        Style::default().fg(Color::Yellow),
    ))];
    lines.push(Line::from(""));
    if app.archived.is_empty() {
        lines.push(Line::from("（归档区还没有任何需求）"));
    } else {
        for (i, r) in app.archived.iter().enumerate() {
            let marker = if i == app.archived_sel { "▶" } else { " " };
            lines.push(Line::from(Span::styled(
                format!("{} {} {}", marker, r.id, r.title),
                if i == app.archived_sel {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                },
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓/jk 选择 · 任意键关闭浮层（v 切回 live 列表） ",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" 归档区（{} 份） ", app.archived.len()));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// 归档结果（`源路径 → 归档路径`；REQ-015 G2 的预演出口）。
fn render_archive(f: &mut Frame, app: &App, area: Rect) {
    let popup = centered(area, 92, 20);
    f.render_widget(Clear, popup);

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        app.message.clone().unwrap_or_default(),
        Style::default().fg(Color::Cyan),
    ))];
    lines.push(Line::from(""));
    if app.archive_plan.is_empty() {
        lines.push(Line::from("（本次没有需要搬移的清单）"));
    } else {
        for (src, dst) in app.archive_plan.iter() {
            lines.push(Line::from(format!("{src} → {dst}")));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓/jk 滚动 · Shift+D 执行真实搬移 · 任意键关闭 ",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 物理归档结果 ");
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.archive_scroll, 0)),
        popup,
    );
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
    use crate::app::{App, ArchiveWhich, Focus, Prompt};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::{Path, PathBuf};
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

    /// 回车（弹窗提交）。
    fn press_enter(app: &mut App) {
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
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
        assert!(text.contains("已解锁"), "首屏应显示解锁态：\n{text}");
        assert!(text.contains("3/3 已通过"), "三段都应显示已通过：\n{text}");
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
        // REQ-014：`g` 的说明从"与 CLI 等价"改成"按当前需求 + 暂存改动判定"
        // （裸 check 在多需求仓库里恒报 Ambiguous，见 gate.rs:707-712 的注释）。
        // 这里跟着改成断言新文案里**真正承重的那半句**，避免"改帮助文案忘了改测试"。
        assert!(help.contains("按当前需求"));

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

    /// 填三段实质正文并全部批准（三段都要有实质内容才批得过）。
    ///
    /// core 的 `testutil` 是 `#[cfg(test)]`，tui 用不了，故就地填——
    /// 与 gui 的同名夹具同理由（重复三行比把测试助手提成公开 API 划算）。
    fn approved_doc(root: &std::path::Path) {
        req_guard_core::requirement::create(root, None, "登录改造").expect("创建需求");
        let p = req_guard_core::requirement::find(root, "REQ-001")
            .expect("清单应存在")
            .path;
        let mut c = std::fs::read_to_string(&p).expect("清单应可读");
        for (heading, line) in [
            ("## 1. 需求分解", "- 背景与问题：TUI amend/seal 夹具。"),
            ("## 2. 技术方案", "- 总体思路：验证修订与绑定。"),
            ("## 3. 测试计划", "- 验收门槛：e / s 两个键可用。"),
        ] {
            let needle = format!("{heading}\n");
            assert!(c.contains(&needle), "模板结构变了：{heading}");
            c = c.replacen(&needle, &format!("{needle}{line}\n"), 1);
        }
        std::fs::write(&p, c).expect("写入应成功");
        for step in ["decomposition", "solution", "testplan"] {
            req_guard_core::requirement::review(root, "REQ-001", step, "寇工", true, "", false)
                .expect("三段都应批得过");
        }
    }

    // ---------- REQ-014：门禁检查消歧接线用的夹具 ----------

    /// 让 `PathSource::Staged` 在临时目录里可用：`git init` 一个空仓。
    ///
    /// 为什么不用 `HOOK_STAGED_FILES` 环境变量：它是**进程级**的，而测试线程并行跑，
    /// 各设各的会互相污染——那种偶发红比没有测试更糟。空仓无暂存内容时
    /// `git diff --cached` 返回空集，正是「空变更集」场景的前置。
    fn git_init(root: &Path) {
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "临时目录应能 git init");
    }

    /// 建一份指定编号的三段全批清单（`approved_doc` 写死了 REQ-001）。
    fn approved_named(root: &Path, id: &str, title: &str) {
        req_guard_core::requirement::create(root, Some(id), title).expect("创建需求");
        let p = req_guard_core::requirement::find(root, id)
            .expect("清单应存在")
            .path;
        let mut c = std::fs::read_to_string(&p).expect("清单应可读");
        for (heading, line) in [
            ("## 1. 需求分解", "- 背景与问题：消歧接线夹具。"),
            ("## 2. 技术方案", "- 总体思路：三段全批以便放行。"),
            ("## 3. 测试计划", "- 验收门槛：带 hint 后能放行。"),
        ] {
            let needle = format!("{heading}\n");
            assert!(c.contains(&needle), "模板结构变了：{heading}");
            c = c.replacen(&needle, &format!("{needle}{line}\n"), 1);
        }
        std::fs::write(&p, c).expect("写入应成功");
        for step in ["decomposition", "solution", "testplan"] {
            req_guard_core::requirement::review(root, id, step, "寇工", true, "", false)
                .unwrap_or_else(|e| panic!("{id} {step} 应批得过：{e}"));
        }
    }

    /// 把所有标记行的 `sum=` 抹成 `-`（模拟 REQ-002 之前批准的存量清单）。
    fn strip_sums(root: &std::path::Path) {
        let p = req_guard_core::requirement::find(root, "REQ-001")
            .expect("清单应存在")
            .path;
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

    fn seal_of(root: &std::path::Path, step: &str) -> req_guard_core::requirement::SealState {
        let p = req_guard_core::requirement::find(root, "REQ-001")
            .expect("清单应存在")
            .path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        req_guard_core::requirement::seal_state(&c, step)
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
    fn e键请求修订_回退待审并清空摘要() {
        let root = temp_dir("amend-flow");
        approved_doc(&root);
        let mut app = App::new(&root);
        assert_eq!(
            seal_of(&root, "decomposition"),
            req_guard_core::requirement::SealState::Frozen
        );

        press(&mut app, 'e');
        type_text(&mut app, "寇工");
        enter(&mut app);
        // 说明必填：空说明应停在原地
        enter(&mut app);
        assert_eq!(
            app.current().unwrap().steps[0].state,
            req_guard_core::status::StepState::Approved,
            "改稿说明为空时不应改动状态"
        );
        type_text(&mut app, "回滚方案缺 DB 迁移回退");
        enter(&mut app);

        let r = app.current().expect("应有需求");
        assert_eq!(r.steps[0].state, req_guard_core::status::StepState::Amended);
        assert_eq!(
            r.steps[0].seal,
            req_guard_core::requirement::SealState::NotApplicable,
            "amend 清 sum= → 不再有已批准正文要保护"
        );
        assert!(!r.unlocked, "amend 不豁免重审");
        assert!(
            app.message.as_deref().unwrap_or("").contains("重新批准"),
            "提示要写明仍需重审：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn s键绑定存量清单后三段皆冻结() {
        let root = temp_dir("seal-flow");
        approved_doc(&root);
        strip_sums(&root);
        let mut app = App::new(&root);

        press(&mut app, 's');
        // 首次补绑定：原因可留空
        enter(&mut app);
        for step in ["decomposition", "solution", "testplan"] {
            assert_eq!(
                seal_of(&root, step),
                req_guard_core::requirement::SealState::Frozen,
                "{step} 补绑定后应真正冻结"
            );
        }
        assert!(
            app.message.as_deref().unwrap_or("").contains("已绑定"),
            "应报出绑了哪几段：{:?}",
            app.message
        );

        // 已全部绑定 → 空原因应被挡；给了原因才放行（记 RESEAL）
        press(&mut app, 's');
        enter(&mut app);
        assert!(
            app.message.as_deref().unwrap_or("").contains("reason"),
            "已绑定清单须给理由：{:?}",
            app.message
        );
        type_text(&mut app, "改动仅为错别字");
        enter(&mut app);
        assert!(
            app.message.as_deref().unwrap_or("").contains("已绑定"),
            "给了理由应放行：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 三段面板标出待绑摘要的段() {
        let root = temp_dir("seal-badge");
        approved_doc(&root);
        strip_sums(&root);
        let app = App::new(&root);
        let text = screen(&draw(&app, 120, 34));
        assert!(
            text.contains("待绑定摘要"),
            "三段都已批准但未绑定 → 标题应报待处理段数：\n{}",
            text
        );
        assert!(text.contains("[未绑定]"), "段行应带未绑定徽标：\n{}", text);
        let _ = std::fs::remove_dir_all(&root);
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

    // ---------- REQ-014：门禁检查消歧接线 ----------

    #[test]
    fn 选中需求时检查放行且不再报歧义() {
        let root = temp_dir("tui-hint");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");

        let mut app = App::new(&root);
        app.selected = 0;
        app.run_check();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("放行"), "选中已批需求应放行：{msg}");
        assert!(
            !msg.contains("歧义"),
            "带 hint 后 footer 不该再报歧义：{msg}"
        );
        assert_eq!(app.overlay, Overlay::Check, "检查后应打开结果浮层");
        assert!(
            app.check_scope.contains("REQ-001"),
            "口径必须写明按哪份判：{}",
            app.check_scope
        );
        assert!(
            app.check_detail.join("\n").contains("REQ-001"),
            "明细应点名被裁决的需求"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 检查浮层首行是口径且含拦截类别() {
        let root = temp_dir("tui-check-overlay");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");
        // 让 REQ-001 处于待审：判定才有拦截类别可显示。
        let p = req_guard_core::requirement::find(&root, "REQ-001")
            .expect("清单应存在")
            .path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        std::fs::write(
            &p,
            c.replace(
                "name=decomposition label=需求分解 status=approved",
                "name=decomposition label=需求分解 status=pending",
            ),
        )
        .expect("写入应成功");

        let mut app = App::new(&root);
        app.selected = 0;
        app.run_check();
        assert_eq!(app.check_kind, "StepNotApproved", "类别应为未过审");
        let text = screen(&draw(&app, 110, 32));
        assert!(text.contains("口径："), "浮层首部应显示口径：\n{text}");
        assert!(text.contains("REQ-001"), "口径应点名需求：\n{text}");
        assert!(text.contains("StepNotApproved"), "应显示类别：\n{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 六条明细理由在浮层里逐条可见() {
        // 锁 REQ-014 的关键设计：footer 只有 2 行可用，装不下多候选的 Ambiguous，
        // 所以明细必须走浮层且可滚动 —— 这条断言在"只取 detail.first()"的实现下会红。
        let root = temp_dir("tui-check-scroll");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        for i in 2..=6 {
            approved_named(&root, &format!("REQ-00{i}"), &format!("改造 {i}"));
        }
        // 清空选中项 → 走全局判定 → 候选 6 条，于是 detail 远长于 footer 的 2 行。
        let mut app = App::new(&root);
        app.reqs.clear();
        app.run_check();
        assert_eq!(app.check_kind, "Ambiguous", "多份候选应报歧义");

        let first = screen(&draw(&app, 120, 40));
        let head = app.check_detail.first().cloned().unwrap_or_default();
        assert!(first.contains(&head), "首屏应含明细首行");

        // 滚到底：最后一条理由必须能被看到。
        for _ in 0..200 {
            app.on_check_key(KeyCode::PageDown);
        }
        let last = app.check_detail.last().cloned().unwrap_or_default();
        let tail = last.trim().to_string();
        assert!(!tail.is_empty(), "明细不应为空");
        let bottom = screen(&draw(&app, 120, 40));
        let snippet: String = tail.chars().take(20).collect();
        assert!(
            bottom.contains(snippet.trim()) || bottom.contains(&snippet),
            "滚到底后末条理由应可见：期望含 {snippet:?}\n{bottom}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn footer只放一行摘要不承载明细() {
        // 明细进浮层之后，footer 仍是 2 行可用高度：塞多行会撑破布局。
        let root = temp_dir("tui-footer");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");
        let mut app = App::new(&root);
        app.reqs.clear();
        app.run_check();
        assert!(app.check_detail.len() >= 2, "应有多条明细");
        let text = screen(&draw(&app, 120, 40));
        let footer_line = text.lines().filter(|l| l.contains("门禁检查：")).count();
        assert_eq!(footer_line, 1, "footer 只应有一行摘要，实际：\n{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 无选中需求时与改前逐字一致() {
        let root = temp_dir("tui-nohint");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");

        let mut app = App::new(&root);
        app.reqs.clear();
        let (ctx, scope) = app.check_ctx();
        assert!(ctx.hint.is_none(), "无选中项时不得编造 hint");
        assert!(
            matches!(ctx.source, req_guard_core::resolve::PathSource::None),
            "无选中项时不得拿空变更集去反查"
        );
        assert!(
            scope.contains("未选择需求"),
            "口径要说明是全局判定：{scope}"
        );

        let v = req_guard_core::gate::gate_check_with(&root, &ctx).expect("判定");
        let legacy = req_guard_core::gate::gate_check(&root).expect("改前的入口");
        assert_eq!(v.is_pass(), legacy.is_pass());
        assert_eq!(v.detail(), legacy.detail(), "明细应与改前逐字一致");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 分支名指向另一份清单时显式选择优先() {
        // REQ-014 关键设计 4 / 非目标 N4：`resolve` 的 R14 分支名兜底只在
        // `hint.is_none() && 判定为 Ambiguous` 时生效；给了 hint 就该走 hint。
        let root = temp_dir("tui-branch");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");

        // 先证明「无 hint 时确实歧义」，即分支名兜底在此形态下救不了它。
        let plain = req_guard_core::gate::gate_check(&root).expect("全局判定");
        assert!(!plain.is_pass(), "无 hint 应歧义");

        // 再证明「给了 hint 就按 hint 判」，且 REQ-002 的状态差异不影响结论。
        let mut app = App::new(&root);
        app.selected = 1; // REQ-002
        app.run_check();
        assert!(
            app.message.clone().unwrap_or_default().contains("放行"),
            "选中 REQ-002 应按它放行"
        );
        assert!(
            !app.check_detail
                .join("\n")
                .contains("无法确定本次改动属于哪份需求"),
            "显式选择后不应报歧义"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 空暂存区按已批需求放行且写明空() {
        let root = temp_dir("tui-empty-staged");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        app.run_check();
        assert!(
            app.message.clone().unwrap_or_default().contains("放行"),
            "空暂存区 + 已批需求应放行"
        );
        assert!(
            app.check_scope.contains("staged（空）"),
            "空暂存区必须写明「空」：{}",
            app.check_scope
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 单份清单时带不带hint结果逐字相同() {
        let root = temp_dir("tui-single");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");

        let a = req_guard_core::gate::gate_check(&root).expect("无 hint");
        let app = App::new(&root);
        let (ctx, _) = app.check_ctx();
        let b = req_guard_core::gate::gate_check_with(&root, &ctx).expect("带 hint");
        assert_eq!(a.is_pass(), b.is_pass(), "单份清单时存量项目零感知");
        assert_eq!(a.detail(), b.detail(), "明细应逐字相同");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 七类拦截类别都能被认出() {
        let cases: &[(&str, &str)] = &[
            (
                "⛔ 拦截：已批准段的正文与批准时不一致（内容冻结校验失败）。",
                "SumMismatch",
            ),
            (
                "⛔ 拦截：指定的 REQ-009 不在可裁决的需求清单里。",
                "UnknownSelection",
            ),
            ("⛔ 拦截：未找到待开发的需求清单。", "NoRequirement"),
            (
                "⛔ 拦截：指定的需求 REQ-001 与本次改动不相交。",
                "SelectionMismatch",
            ),
            (
                "⛔ 拦截：本次改动所属的需求存在未解决的阻塞性评论。",
                "OpenBlockingComment",
            ),
            (
                "⛔ 拦截：无法确定本次改动属于哪份需求（仓库内有多份未归档清单）。",
                "Ambiguous",
            ),
            (
                "⛔ 拦截：本次改动所属的需求尚未通过审核。",
                "StepNotApproved",
            ),
        ];
        for (summary, want) in cases {
            assert_eq!(
                crate::app::check_kind_of(summary, &[]),
                Some(*want),
                "类别识别错：{summary}"
            );
        }
        assert_eq!(
            crate::app::check_kind_of("✅ 门禁放行：REQ-001 三段已批准", &[]),
            None,
            "放行不该被认成任何拦截类别"
        );
    }

    #[test]
    fn c键复看结果且不重跑判定() {
        // 复看不该再写一条审计（与 REQ-016「只读体检不进轮询」同源）。
        let root = temp_dir("tui-recheck");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'c');
        assert_eq!(app.overlay, Overlay::None, "还没检查过时 c 不该开浮层");

        app.run_check();
        assert_eq!(app.overlay, Overlay::Check);
        app.overlay = Overlay::None;
        let detail = app.check_detail.clone();
        press(&mut app, 'c');
        assert_eq!(app.overlay, Overlay::Check, "c 应复看上次结果");
        assert_eq!(app.check_detail, detail, "复看不得改动明细");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------- REQ-015：需求生命周期 ----------

    /// 造一个 git 身份为 `name` 的临时仓（供预填类用例取真实身份）。
    fn git_named(root: &Path, name: &str) {
        git_init(root);
        for (k, v) in [("user.name", name), ("user.email", "t@example.com")] {
            let ok = std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(ok, "应能设置 git {}", k);
        }
    }

    #[test]
    fn d键_无阻塞评论时一次即可进入操作人输入() {
        let root = temp_dir("tui-done");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        app.selected = 0;
        press(&mut app, 'd');
        assert_eq!(
            app.prompt,
            Some(Prompt::DoneActor),
            "无阻塞评论时直接要操作人"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn d键_有阻塞评论时第一次只亮三项事实() {
        // TUI 没有勾选框，用「按两次」实现同一道关（REQ-015 G4）。
        let root = temp_dir("tui-done-blocking");
        approved_named(&root, "REQ-001", "登录改造");
        req_guard_core::comment::add(
            &root,
            "REQ-001",
            req_guard_core::comment::NewComment {
                step: Some("solution"),
                author: "寇工",
                text: "先补回滚方案",
                quote: None,
                blocking: true,
                reply: None,
            },
        )
        .expect("添加阻塞评论");

        let mut app = App::new(&root);
        app.selected = 0;
        press(&mut app, 'd');
        assert_eq!(app.prompt, None, "第一次不应进入输入（先让人看事实）");
        assert_eq!(app.overlay, Overlay::DoneConfirm);
        let facts = app.done_confirm_lines().join("\n");
        assert!(facts.contains("3/3"), "须写明通过段数：{facts}");
        assert!(facts.contains("阻塞"), "须写明阻塞评论数：{facts}");
        assert!(facts.contains("门禁不再管辖"), "须写明后果：{facts}");

        press(&mut app, 'd');
        assert_eq!(app.prompt, Some(Prompt::DoneActor), "第二次才要操作人");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 归档区视图下a键不触发批准() {
        // AC-015 的 TUI 落点：core 对 `status=done` 有护栏，界面还要**不给按钮**。
        let root = temp_dir("tui-archived-ro");
        approved_named(&root, "REQ-001", "登录改造");
        req_guard_core::requirement::done(&root, "REQ-001", "寇工").expect("先 done");
        req_guard_core::requirement::archive_one(&root, "寇工", "REQ-001", false)
            .expect("物理归档");

        let mut app = App::new(&root);
        press(&mut app, 'v');
        assert!(app.read_only(), "v 应切到归档区只读视图");
        assert!(!app.archived.is_empty(), "归档区应能列出清单");

        // 逐个键位验证「写动作被挡下且没有落到 core」。
        app.message = None;
        press(&mut app, 'a');
        assert_eq!(app.prompt, None, "只读下不得弹出批准输入");
        assert!(
            app.message.clone().unwrap_or_default().contains("只读"),
            "应说明原因：{:?}",
            app.message
        );
        for k in ['r', 'n', 'e', 's', 'b'] {
            app.message = None;
            press(&mut app, k);
            assert_eq!(app.prompt, None, "'{}' 在只读下不得弹窗", k);
            assert!(
                app.message.clone().unwrap_or_default().contains("只读"),
                "'{}' 应被只读拦下：{:?}",
                k,
                app.message
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn v键来回切换live与归档区() {
        let root = temp_dir("tui-toggle");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'v');
        assert!(app.browsing_archived);
        assert_eq!(app.overlay, Overlay::Archived);
        press(&mut app, 'v');
        assert!(!app.browsing_archived, "再按一次应切回 live");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 预演归档列出src到dst且不动磁盘() {
        let root = temp_dir("tui-archive-preview");
        approved_named(&root, "REQ-001", "登录改造");
        req_guard_core::requirement::done(&root, "REQ-001", "寇工").expect("先 done");
        let src = req_guard_core::requirement::find(&root, "REQ-001")
            .expect("清单应存在")
            .path;

        let mut app = App::new(&root);
        app.reload();
        app.input = "寇工".into();
        app.archive_after_days();
        // 直接走到期清扫预演（done 后 live 集为空，单份预演需先选中，等价路径）。
        app.run_archive(ArchiveWhich::Due, true);
        assert!(src.exists(), "预演后文件必须还在原路径");
        assert_eq!(app.overlay, Overlay::Archive);
        let text = screen(&draw(&app, 110, 34));
        assert!(text.contains("物理归档结果"), "应显示结果浮层：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 到期天数出现在归档结果里() {
        // 不写它，用户无从判断「到期」是多久。
        let root = temp_dir("tui-archive-days");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        assert_eq!(app.archive_after_days(), 30, "无配置时 core 默认 30 天");
        app.input = "寇工".into();
        app.run_archive(ArchiveWhich::Due, true);
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("30"), "结果必须写明天数：{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 审核人预填取自实时git身份() {
        // AC-016：预填值取自实时身份而非硬编码。
        let root = temp_dir("tui-prefill");
        git_named(&root, "Mike Zhu");
        let app = App::new(&root);
        assert_eq!(app.prefill_reviewer(), "Mike Zhu");
        git_named(&root, "另一个人");
        assert_eq!(app.prefill_reviewer(), "另一个人", "预填应随 git 身份变化");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn i键显示身份与鉴权等级() {
        let root = temp_dir("tui-who");
        git_named(&root, "Mike Zhu");
        let mut app = App::new(&root);
        press(&mut app, 'i');
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("Mike Zhu"), "应显示姓名：{msg}");
        assert!(msg.contains("L"), "应显示鉴权等级：{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 状态栏显示身份区且不挤压解锁态() {
        // REQ-015 G6 的落点。宽度刻意取 120（常见终端宽度）：
        // 身份区原先挂在顶栏，会把行尾的「已解锁」截断成「已解」（实测），
        // 故挪到 footer 的空白行——这条同时守住「身份可见」与「解锁态不被挤掉」。
        let root = temp_dir("tui-header-who");
        git_named(&root, "Mike Zhu");
        let app = App::new(&root);
        let text = screen(&draw(&app, 120, 30));
        assert!(text.contains("Mike Zhu"), "状态栏应显示身份：{text}");
        assert!(text.contains("L0"), "应显示鉴权等级：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------- REQ-016：校验报告 ----------

    /// 三段全批但**不绑定**摘要（模拟 REQ-002 之前的存量清单）。
    fn approved_unsealed(root: &Path, id: &str, title: &str) {
        approved_named(root, id, title);
        let p = req_guard_core::requirement::find(root, id)
            .expect("清单应存在")
            .path;
        let c = std::fs::read_to_string(&p).expect("可读");
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

    #[test]
    fn h键打开体检浮层且只读不签票() {
        // AC-010 的 TUI 落点：只读体检不消耗审批资格。
        let root = temp_dir("tui-health");
        approved_unsealed(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'H');
        assert_eq!(app.overlay, Overlay::Health);
        assert!(app.health.is_some(), "应缓存体检结果");
        assert_eq!(app.cred_issued, 0, "只读体检不得签发凭据");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 数字键1到4依次切换体检子标签() {
        // AC-019
        let root = temp_dir("tui-health-tabs");
        approved_unsealed(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'H');
        for (key, want) in [('1', 0usize), ('2', 1), ('3', 2), ('4', 3)] {
            press(&mut app, key);
            assert_eq!(app.health_tab, want, "按 {key} 应切到子标签 {want}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 体检浮层只占一个而不是四个() {
        // AC-019 后半：四类体检只新增**一个** overlay，而不是四个。
        //
        // ⚠️ 刻意**不**断言"overlay 总数是 N"：那个数字会被后续每个需求改动，
        // 于是这条测试会变成"每加一个浮层就来改这里"的例行公事——
        // 而它真正要守的是「四类体检共用一个浮层」。改为断言这个不变量。
        assert_eq!(
            crate::app::overlay_tab_count().get(&Overlay::Health),
            Some(&4),
            "体检浮层内部应有 4 个子标签（而不是外层拆成 4 个浮层）"
        );
        // 判别式互异：新增 overlay 不得与既有取值冲突（冲突会让 match 走错分支）。
        let all = [
            Overlay::None,
            Overlay::Help,
            Overlay::Audit,
            Overlay::Check,
            Overlay::Comments,
            Overlay::DoneConfirm,
            Overlay::Archived,
            Overlay::Archive,
            Overlay::Health,
            Overlay::Hygiene,
        ];
        let uniq: std::collections::BTreeSet<usize> = all.iter().map(|o| *o as usize).collect();
        assert_eq!(uniq.len(), all.len(), "overlay 判别式必须互异");
    }

    #[test]
    fn 体检浮层渲染出四组子标签() {
        let root = temp_dir("tui-health-render");
        approved_unsealed(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'H');
        let text = screen(&draw(&app, 120, 34));
        assert!(text.contains("体检总览"), "应显示浮层标题：{text}");
        for name in HEALTH_GROUPS {
            assert!(text.contains(name), "四组子标签都应可见：{text}");
        }
        assert!(text.contains("只读"), "必须标注只读：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 内容一致性体检把未绑定计为警告() {
        // AC-003 / AC-015 的 TUI 落点
        let root = temp_dir("tui-health-sums");
        approved_unsealed(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");
        let mut app = App::new(&root);
        press(&mut app, 'H');
        let (err, warn) = app.health_counts()[1];
        assert_eq!(warn, 3, "三段已批准但未绑定 → 3 条警告");
        assert_eq!(err, 0, "另一份已绑定且一致 → 0 条错误");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 交叉引用体检定位失败时不给具体结论() {
        // AC-009 的 TUI 落点
        let root = temp_dir("tui-health-crossfail");
        approved_named(&root, "REQ-001", "登录改造");
        let p = req_guard_core::requirement::find(&root, "REQ-001")
            .expect("清单应存在")
            .path;
        let c = std::fs::read_to_string(&p).expect("可读");
        std::fs::write(&p, c.replace("## 2. 技术方案", "## 技术方案")).expect("写入应成功");

        let mut app = App::new(&root);
        press(&mut app, 'H');
        press(&mut app, '4');
        let lines = app.health_lines().join("\n");
        assert!(lines.contains("定位失败"), "应如实说明定位失败：{lines}");
        assert!(
            !lines.contains("目标文件不存在"),
            "定位失败时不得给出具体引用结论：{lines}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 交叉引用切片用严格定位而非回退整篇() {
        // AC-026 的判决性实验靶子：把 `App::cross_ref_slice` 里的 `section_span`
        // 换成会回退整篇的 `section_of`，这条必须变红。
        let broken = "## 1. 需求分解\n甲\n## 技术方案\n见 `docs/不存在.md#§9`\n";
        assert!(
            App::cross_ref_slice(broken).is_none(),
            "第 2 段标题被改坏时必须如实返回 None（fail-closed）"
        );
        let good = "## 1. 需求分解\n甲\n## 2. 技术方案\n见 `docs/x.md`\n## 3. 测试计划\n乙\n";
        let (section, first_line) = App::cross_ref_slice(good).expect("正常文档应能切片");
        // `section_span` 给的行区间**不含标题行本身**，故切出来的是段落正文。
        assert!(
            section.contains("docs/x.md"),
            "切出来的应是第 2 段的正文：{section}"
        );
        assert!(!section.contains('甲'), "不该含第 1 段：{section}");
        assert_eq!(first_line, 4, "正文首行是全文第 4 行（标题在第 3 行）");
    }

    #[test]
    fn 变更范围体检报出未声明的暂存文件() {
        // AC-004 的 TUI 落点。
        // 用**真 git 索引**而不是 `HOOK_STAGED_FILES` 环境变量：后者是进程级的，
        // 并行跑别的用例时会读到这份"暂存区"（实测确实污染了
        // `三段通过后显示已解锁`）。真索引按 root 隔离，不互相污染。
        let root = temp_dir("tui-health-touch");
        approved_named(&root, "REQ-001", "登录改造");
        git_init(&root);
        let p = root.join("src/未声明.rs");
        std::fs::create_dir_all(p.parent().expect("父目录")).expect("建目录");
        std::fs::write(&p, "// 夹具\n").expect("写文件");
        let ok = std::process::Command::new("git")
            .args(["add", "--", "src/未声明.rs"])
            .current_dir(&root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "应能 git add");

        let mut app = App::new(&root);
        press(&mut app, 'H');
        let lines = app.health_lines().join("\n");
        assert!(lines.contains("src/未声明.rs"), "必须点明文件：{lines}");
        assert_eq!(app.health_counts()[0].0, 1, "应恰好 1 条错误");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 体检浮层错误与警告的着色不同() {
        // AC-020：两类问题至少要有一个可区分的取值。
        let err = Color::Red;
        let warn = Color::Yellow;
        assert_ne!(err, warn, "错误与警告必须能区分");
    }

    #[test]
    fn w键走两步声明且路径初始为空() {
        // AC-011 的 TUI 落点：先路径、再原因，两步都必填。
        let root = temp_dir("tui-declare");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        app.selected = 0;
        let warn = app.declare_warning();
        assert!(warn.contains("打回"), "必须说明会打回：{warn}");
        assert!(warn.contains("清空"), "必须说明会清空摘要：{warn}");

        press(&mut app, 'W');
        assert_eq!(app.prompt, Some(Prompt::DeclareGlob), "第一步要路径");
        assert!(app.input.is_empty(), "路径输入初始必须为空");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 声明的路径为空时被挡下() {
        let root = temp_dir("tui-declare-empty");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'W');
        app.input = "   ".into();
        app.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.prompt, Some(Prompt::DeclareGlob), "应停在第一步");
        assert!(
            app.message
                .clone()
                .unwrap_or_default()
                .contains("路径不能为空"),
            "{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 一键批量绑定走seal_all且签发清除各一次() {
        // 关键设计 1：必须走批量接口（L3 下循环单份必坏）。
        let root = temp_dir("tui-seal-all");
        approved_unsealed(&root, "REQ-001", "甲");
        approved_unsealed(&root, "REQ-002", "乙");
        git_named(&root, "Mike Zhu");
        let mut app = App::new(&root);
        press(&mut app, 'S');
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("尝试 2"), "应报尝试数：{msg}");
        assert_eq!(app.cred_issued, 1, "只签发一次凭据");
        assert_eq!(app.cred_cleared, 1, "也只清除一次");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 归档区视图下体检外的写动作也被挡() {
        // 只读护栏要覆盖 REQ-016 新增的写入入口。
        let root = temp_dir("tui-health-ro");
        approved_named(&root, "REQ-001", "登录改造");
        req_guard_core::requirement::done(&root, "REQ-001", "寇工").expect("先 done");
        req_guard_core::requirement::archive_one(&root, "寇工", "REQ-001", false)
            .expect("物理归档");
        let mut app = App::new(&root);
        press(&mut app, 'v');
        assert!(app.read_only());
        for k in ['W', 'S'] {
            app.message = None;
            press(&mut app, k);
            assert_eq!(app.prompt, None, "'{k}' 在只读下不得弹窗");
            assert!(
                app.message.clone().unwrap_or_default().contains("只读"),
                "'{k}' 应被只读拦下：{:?}",
                app.message
            );
        }
        // 只读体检本身仍可用（它是只读的）。
        press(&mut app, 'H');
        assert_eq!(app.overlay, Overlay::Health, "只读视图下体检仍应可开");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 体检浮层在只读视图下也给出结论() {
        // 归档区里的清单同样该能体检：只读不等于不可看。
        let root = temp_dir("tui-health-archived");
        approved_unsealed(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'H');
        assert_eq!(app.cred_issued, 0);
        assert!(app.health.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 刷新摘要提示含digest字样且不签票() {
        // AC-017 的 TUI 落点
        let root = temp_dir("tui-digest");
        approved_named(&root, "REQ-001", "登录改造");
        req_guard_core::gate::audit_ledger(&root, "NOTE actor=夹具");
        let mut app = App::new(&root);
        press(&mut app, 'G');
        let msg = app.message.clone().unwrap_or_default();
        assert_eq!(app.cred_issued, 0, "刷新摘要属非审批类");
        assert!(
            msg.contains("DIGEST") || msg.contains("失败"),
            "提示应含 DIGEST（或如实报失败）：{msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------- REQ-017：工程卫生 ----------

    #[test]
    fn n键两步新建_编号步提示自动编号() {
        // AC-020：第一步提示含「自动编号」，回车后进入标题步。
        let root = temp_dir("tui-newid");
        let mut app = App::new(&root);
        press(&mut app, 'n');
        assert_eq!(app.prompt, Some(Prompt::NewId), "第一步应是编号");
        let title = Prompt::NewId.title();
        assert!(title.contains("自动编号"), "提示必须写明可留空：{title}");
        press_enter(&mut app);
        assert_eq!(app.prompt, Some(Prompt::NewTitle), "回车应进入标题步");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn n键留空编号即自动编号() {
        // 留空 → `create(None, ..)`：与改前逐字一致。
        let root = temp_dir("tui-newid-auto");
        let mut app = App::new(&root);
        press(&mut app, 'n');
        press_enter(&mut app);
        app.input = "登录改造".into();
        press_enter(&mut app);
        assert!(
            app.message.clone().unwrap_or_default().contains("已创建"),
            "{:?}",
            app.message
        );
        assert!(root.join(".gates/requirements").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn n键可自定义编号且提示不阻断() {
        let root = temp_dir("tui-newid-custom");
        let mut app = App::new(&root);
        press(&mut app, 'n');
        app.input = "20260919-x".into(); // 数字开头 → `lint_id` 会给形态提示
        press_enter(&mut app);
        assert_eq!(
            app.prompt,
            Some(Prompt::NewTitle),
            "有提示也要继续（不阻断）"
        );
        app.input = "登录改造".into();
        press_enter(&mut app);
        assert!(
            app.message.clone().unwrap_or_default().contains("已创建"),
            "{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 帮助浮层含环境节且有sig与auth字样() {
        // AC-021
        let root = temp_dir("tui-help-env");
        git_named(&root, "Mike Zhu");
        let mut app = App::new(&root);
        press(&mut app, '?');
        let text = screen(&draw(&app, 140, 40));
        assert!(text.contains("sig="), "帮助里应有身份节：{text}");
        assert!(text.contains("auth"), "应出现 auth 字样：{text}");
        assert!(text.contains("票据"), "应显示票据摘要：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 环境节不含凭据哈希形态() {
        // AC-017 的 TUI 落点：不得出现 64 位十六进制串。
        let root = temp_dir("tui-env-hash");
        git_named(&root, "Mike Zhu");
        let app = App::new(&root);
        let text = app.env_lines().join("\n");
        for tok in text.split(|c: char| !c.is_ascii_hexdigit()) {
            assert_ne!(tok.len(), 64, "出现了 64 位十六进制串：{tok}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn y键打开卫生浮层且只读不签票() {
        let root = temp_dir("tui-hygiene");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'Y');
        assert_eq!(app.overlay, Overlay::Hygiene);
        assert!(app.hygiene.is_some(), "应缓存卫生结果");
        assert_eq!(app.cred_issued, 0, "只读卫生不得签发凭据");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 卫生浮层数字键切分组且渲染三组() {
        let root = temp_dir("tui-hygiene-tabs");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'Y');
        for (k, want) in [('1', 0usize), ('2', 1), ('3', 2)] {
            press(&mut app, k);
            assert_eq!(app.hygiene_tab, want);
        }
        let text = screen(&draw(&app, 120, 34));
        assert!(text.contains("工程卫生"), "标题应可见：{text}");
        assert!(text.contains("编号冲突"), "分组应可见：{text}");
        assert!(text.contains("门禁自检"), "分组应可见：{text}");
        assert!(text.contains("环境"), "分组应可见：{text}");
        assert!(text.contains("不提供票据"), "应写明限制：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 卫生面板报出编号冲突且计数为一() {
        let root = temp_dir("tui-hygiene-conflict");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).expect("建目录");
        for name in ["REQ-001-甲.md", "REQ-001-乙.md"] {
            std::fs::write(
                dir.join(name),
                "---\ndoc_type: proposal\n---\n\n<!-- GATE:HEAD id=REQ-001 status=in_review created=2026-10-06_10:00:00 -->\n",
            )
            .expect("写清单");
        }
        let mut app = App::new(&root);
        press(&mut app, 'Y');
        let (err, _) = app.id_issue_counts();
        assert_eq!(err, 1, "应恰好 1 条重复类错误");
        let lines = app.hygiene_lines().join("\n");
        assert!(lines.contains("REQ-001-甲.md"), "应点名文件：{lines}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 卫生面板不随轮询重算() {
        let root = temp_dir("tui-hygiene-cache");
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'Y');
        let before = app.hygiene_lines();
        for _ in 0..3 {
            app.reload();
        }
        assert_eq!(app.hygiene_lines(), before, "轮询不得改动卫生结果");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 返工率超阈值才提示复盘() {
        let root = temp_dir("tui-rework");
        approved_named(&root, "REQ-001", "登录改造");
        for _ in 0..4 {
            req_guard_core::gate::audit_ledger(&root, "AMEND REQ-001 step=solution actor=寇工");
        }
        req_guard_core::gate::audit_ledger(&root, "AMEND REQ-001 step=decomposition actor=寇工");
        let app = App::new(&root);
        let text = app.rework_lines("REQ-001").join("\n");
        assert!(text.contains("改稿 4 次"), "应显示次数：{text}");
        assert!(text.contains("复盘"), "超阈值应提示复盘：{text}");
        assert!(
            text.contains("技术方案：改稿 4 次"),
            "提示要挂在超阈值那一段：{text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 界面上没有票据写入口() {
        let src = include_str!("ui.rs");
        let src = src.split("#[cfg(test)]").next().expect("源码前段");
        let lower = src.to_lowercase();
        for bad in ["token issue", "token revoke", "签发票据", "撤销票据"] {
            assert!(
                !lower.contains(&bad.to_lowercase()),
                "TUI 不得提供票据写操作，但出现了 `{bad}`"
            );
        }
    }

    #[test]
    fn 鉴权失败文案逐字透传不被包装() {
        let src = include_str!("app.rs");
        let src = src.split("#[cfg(test)]").next().expect("源码前段");
        assert!(
            !src.to_lowercase().contains("操作失败"),
            "不得把鉴权文案包装成「操作失败」"
        );
        assert!(src.contains("签发界面凭据失败："), "必须带 core 的原文");
    }

    #[test]
    fn 卫生浮层与体检浮层的子标签数互不混淆() {
        assert_eq!(
            crate::app::overlay_tab_count().get(&Overlay::Health),
            Some(&4)
        );
        assert_eq!(
            crate::app::overlay_tab_count().get(&Overlay::Hygiene),
            Some(&3)
        );
    }

    #[test]
    fn 自定义编号通过界面真的落到文件名上() {
        let root = temp_dir("tui-newid-file");
        let mut app = App::new(&root);
        press(&mut app, 'n');
        app.input = "REQ-kd-20261005-A7F3".into();
        press_enter(&mut app);
        app.input = "登录改造".into();
        press_enter(&mut app);
        assert!(
            req_guard_core::requirement::find(&root, "REQ-kd-20261005-A7F3").is_ok(),
            "清单应使用该编号：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 自定义编号为空时编号步不报错() {
        // 留空是**合法**输入（自动编号），不该被当成"必填没填"。
        let root = temp_dir("tui-newid-empty-ok");
        let mut app = App::new(&root);
        press(&mut app, 'n');
        press_enter(&mut app);
        assert_eq!(app.prompt, Some(Prompt::NewTitle), "留空应直接进入标题步");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 体检结果不随轮询变化() {
        // 体检不进 3 秒轮询：不主动触发就不重算。
        let root = temp_dir("tui-health-nopoll");
        approved_unsealed(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        press(&mut app, 'H');
        let before = app.health_counts();
        for _ in 0..3 {
            app.reload();
        }
        let after = app.health_counts();
        assert_eq!(before, after, "轮询不得改动体检结果");
        let _ = std::fs::remove_dir_all(&root);
    }
}

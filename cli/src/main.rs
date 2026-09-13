//! req-guard CLI 入口：参数解析 + 文本输出。
//!
//! 业务逻辑（清单生命周期、审核评论、门禁判定）全部在 `req-guard-core`；
//! 本 crate 只做参数翻译与文本渲染，**不做任何判定**——判定唯一真相在 core。

mod cli;
mod render;

use cli::Action;
use req_guard_core::comment;
use req_guard_core::error::{GateError, Result};
use req_guard_core::gate;
use req_guard_core::requirement;
use req_guard_core::status;
use std::path::Path;

fn main() {
    let parsed = match cli::parse() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&parsed.args) {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}

fn run(a: &cli::Args) -> Result<()> {
    let root = &a.root;
    // 方案 B：审批命令显式给 --token 时注入环境，供 core 的 ensure_human 校验。
    // （无 --token 则走 REQ_GUARD_TOKEN 环境变量；未启用令牌则回退方案 A）
    if let Some(t) = a.token.as_deref() {
        if !t.trim().is_empty() {
            std::env::set_var(req_guard_core::token::TOKEN_ENV, t);
        }
    }
    match a.action {
        Action::Init | Action::Install => {
            // --verify：只校验不写入（CI 步骤；§4.5 消除 L1 静默缺口）
            if a.verify {
                let problems = gate::verify_install(root);
                if problems.is_empty() {
                    println!("✅ 门禁校验通过：核心资产 / AI 工具 hook / pre-commit 全部就位");
                    return Ok(());
                }
                for p in &problems {
                    eprintln!("✗ {}", p);
                }
                eprintln!(
                    "共 {} 项未通过；修复后重跑 req-guard install --verify",
                    problems.len()
                );
                std::process::exit(1);
            }
            let tools = if a.tools.is_empty() {
                gate::default_tools()
            } else {
                a.tools.clone()
            };
            let created = gate::install(root, &tools, true)?;
            println!("✅ AI 需求门禁已安装：{}", root.display());
            for f in &created {
                println!("   - {}", f.display());
            }
            println!("   拦截：AI 工具 Write/Edit（PreToolUse）+ git pre-commit（fail-closed）");
            println!(
                "   CI  ：enforce.ci 默认 true——流水线须调用 req-guard check 并设为必需状态检查"
            );
        }
        Action::Create => {
            let title = a.title.as_deref().unwrap_or("");
            let r = requirement::create(root, a.id.as_deref(), title)?;
            println!("✅ 已创建需求清单：{} {}", r.id, r.title);
            println!("   文件 : {}", r.path.display());
            println!("   下一步：由 AI 填写三段正文，再由审核人逐段批准：");
            println!(
                "           req-guard approve {} --step decomposition --reviewer <姓名>",
                r.id
            );
        }
        Action::Approve | Action::Reject => {
            let pass = matches!(a.action, Action::Approve);
            let id = a.id.as_deref().unwrap_or("");
            let step = a.step.as_deref().unwrap_or("");
            let reviewer = resolve_identity(a.reviewer.as_deref(), "审核人", "--reviewer")?;
            // 是否强制审核顺序，由 .gates/req-guard.yaml 的 strict_order 决定（缺失时 fail-closed = true）。
            let strict = gate::strict_order(root);
            let r = requirement::review(
                root,
                id,
                step,
                &reviewer,
                pass,
                a.reason.as_deref().unwrap_or(""),
                strict,
            )?;
            println!(
                "✅ 已{}：{} / {}（{}，审核人 {}）",
                if pass { "批准" } else { "打回" },
                r.id,
                step,
                requirement::step_label(step),
                reviewer
            );
            // 附加评论（--comment）
            if let Some(text) = a.text.as_deref() {
                let c = comment::add(
                    root,
                    id,
                    comment::NewComment {
                        step: Some(step),
                        author: &reviewer,
                        text,
                        quote: a.quote.as_deref(),
                        blocking: a.blocking,
                        reply: a.reply.as_deref(),
                    },
                )?;
                println!("   已附加评论 {}", c.id);
            }
            render::print_status(root, Some(&r.id))?;
            render::print_comment_summary(&status::req_get(root, &r.id)?);
        }
        Action::Comment => {
            let id = a.id.as_deref().unwrap_or("");
            let author = resolve_identity(a.author.as_deref(), "评论作者", "--author")?;
            let text = a.text.as_deref().unwrap_or("");
            let c = comment::add(
                root,
                id,
                comment::NewComment {
                    step: a.step.as_deref(),
                    author: &author,
                    text,
                    quote: a.quote.as_deref(),
                    blocking: a.blocking,
                    reply: a.reply.as_deref(),
                },
            )?;
            match &c.reply {
                Some(parent) => println!(
                    "✅ 已添加评论 {}（需求 {}，作者 {}，回复 {}）",
                    c.id, id, author, parent
                ),
                None => println!("✅ 已添加评论 {}（需求 {}，作者 {}）", c.id, id, author),
            }
            if a.blocking {
                println!("   ⚠️ 阻塞性评论：未 resolve 前将拦截 AI 编码");
            }
            if let Some(line) = c.line {
                println!("   锚点 : 第 {} 行", line);
            } else if c.quote.is_some() {
                println!("   ⚠️ 锚点 : 未能在正文中定位到引用，已降级为步骤级");
            }
            render::print_comment_summary(&status::req_get(root, id)?);
        }
        Action::Resolve => {
            let id = a.id.as_deref().unwrap_or("");
            let cid = a.comment_id.as_deref().unwrap_or("");
            let author = resolve_identity(a.author.as_deref(), "审核人", "--author")?;
            comment::resolve(root, id, cid, &author)?;
            println!("✅ 已关闭评论 {}（需求 {}，审核人 {}）", cid, id, author);
            render::print_comment_summary(&status::req_get(root, id)?);
        }
        Action::Status => {
            let id = a.id.as_deref();
            render::print_status(root, id)?;
            if let Some(i) = id {
                render::print_comment_summary(&status::req_get(root, i)?);
            }
        }
        Action::List => render::print_status(root, None)?,
        Action::Comments => {
            let id = a.id.as_deref().unwrap_or("");
            if a.refresh_anchors {
                let n = comment::refresh_anchors(root, id)?;
                println!("✅ 已重算行号锚点：{} 条失效（stale）", n);
            }
            let cs = comment::list(root, id)?;
            if cs.is_empty() {
                println!("需求 {} 暂无评论。", id);
                return Ok(());
            }
            println!("评论（{} 条）：", cs.len());
            for c in &cs {
                let flag = if c.is_blocking_open() {
                    " [阻塞]"
                } else {
                    ""
                };
                let stale = if c.stale { " [锚点失效]" } else { "" };
                let loc = c
                    .line
                    .map(|n| format!("第{}行", n))
                    .unwrap_or_else(|| "未定位".to_string());
                println!(
                    "  {} [{}] {} | {} | {} | {}{}{}",
                    c.id,
                    c.state.as_str(),
                    loc,
                    c.ts,
                    c.author,
                    c.step.as_deref().unwrap_or("-"),
                    flag,
                    stale
                );
                if let Some(q) = &c.quote {
                    println!("      引用: {}", q);
                }
                for l in c.body.lines() {
                    println!("      {}", l);
                }
            }
        }
        Action::Check => {
            // 裁决来自拦截脚本（唯一判定逻辑），这里只做渲染与退出码。
            let verdict = gate::gate_check(root)?;
            render::print_verdict(&verdict);
            if !verdict.is_pass() {
                std::process::exit(1);
            }
        }
        Action::Bypass => {
            let actor = resolve_identity(a.author.as_deref(), "操作人", "--author")
                .unwrap_or_else(|_| "unknown".to_string());
            let p = gate::bypass(root, a.reason.as_deref().unwrap_or(""), &actor, a.ttl)?;
            println!(
                "⚠️ 已开启应急绕过：{} 分钟（操作人 {}，已记审计）",
                a.ttl, actor
            );
            println!("   令牌 : {}", p.display());
            println!("   到期后自动恢复硬拦截；请事后补齐清单审核。");
        }
        Action::AuditDigest => {
            let (path, hex, lines) = gate::audit_digest(root)?;
            println!("✅ 审计摘要已写入：{}", path.display());
            println!(
                "   日志   : .gates/audit/gate-audit.log（{} 行，本机不入库）",
                lines
            );
            println!("   sha256 : {}", hex);
            println!(
                "   请将 {} 随本次改动提交；PR 中可与各本机日志比对以发现篡改。",
                path.display()
            );
        }
        Action::Token { ref sub } => run_token(root, a, sub)?,
        Action::Ui => run_ui(root, a)?,
    }
    Ok(())
}

/// 审批令牌管理（方案 B）。
fn run_token(_root: &Path, a: &cli::Args, sub: &str) -> Result<()> {
    use req_guard_core::token;
    match sub {
        "issue" => {
            let (raw, expires, ttl, path) = token::issue(a.ttl)?;
            println!("✅ 已签发审批令牌（方案 B，有效期 {} 分钟）", ttl);
            println!("   配置  : {}", path.display());
            println!("   ⚠️ 请立即复制下面这串令牌原文——仅显示一次，不落盘：");
            println!("   ┌─ 令牌 ────────────────────────────────────");
            println!("   {}", raw);
            println!("   └──────────────────────────────────────────");
            println!("   到期 epoch : {}", expires);
            println!("   用法 : req-guard approve REQ-001 --step decomposition --reviewer 张三 --token <令牌>");
            println!("         或先 export REQ_GUARD_TOKEN=<令牌> 后省略 --token");
            println!("   请把令牌存入密码管理器/自身会话，勿提交版本库、勿发给 AI。");
        }
        "status" => match token::load() {
            Some(cfg) => {
                println!("✅ 审批令牌已启用（方案 B）");
                println!("   到期 epoch : {}", cfg.expires_epoch);
                println!("   哈希(前12) : {}", &cfg.hash[..cfg.hash.len().min(12)]);
            }
            None => {
                println!(
                    "ℹ️  审批令牌（方案 B）未启用，当前鉴权为方案 A（REQ_GUARD_AI_CTX 软标记）"
                );
            }
        },
        "revoke" => {
            let path = token::revoke()?;
            println!("✅ 已撤销并禁用审批令牌：{}", path.display());
            println!("   审批鉴权已回退到方案 A。");
        }
        _ => unreachable!("token 子命令已在校验层限制为 issue/status/revoke"),
    }
    Ok(())
}

/// 打开门禁管理台：按**构建 feature + 运行环境**选择界面（见《UI架构细化方案.md》§3）。
///
/// 界面只做管理台，所有状态读写都走 core，与 CLI 行为完全等价。
fn run_ui(root: &Path, a: &cli::Args) -> Result<()> {
    use req_guard_core::ui_mode::{self, UiAvailability, UiMode};

    let forced = if a.gui {
        Some(UiMode::Gui)
    } else if a.tui {
        Some(UiMode::Tui)
    } else {
        None
    };
    let avail = UiAvailability {
        gui: cfg!(feature = "gui"),
        tui: cfg!(feature = "tui"),
    };
    let mode = ui_mode::detect(forced, avail, &ui_mode::EnvFacts::capture())?;

    match mode {
        UiMode::Tui => {
            #[cfg(feature = "tui")]
            {
                req_guard_tui::run(root)
            }
            #[cfg(not(feature = "tui"))]
            {
                Err(GateError::Validation(format!(
                    "本次构建未包含 TUI（项目 {}）。请用 `cargo build -p req-guard --features tui` 重新构建",
                    root.display()
                )))
            }
        }
        UiMode::Gui => {
            #[cfg(feature = "gui")]
            {
                // 兜底：GUI 启动失败（无图形栈 / wgpu 初始化失败）时回退到 TUI，
                // 而不是让整个工具崩掉（见《UI架构细化方案.md》§3.1 第 8 条）。
                #[cfg(feature = "tui")]
                {
                    match req_guard_gui::run(root) {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            eprintln!("⚠️ GUI 启动失败，已回退到 TUI：{}", e);
                            req_guard_tui::run(root)
                        }
                    }
                }
                #[cfg(not(feature = "tui"))]
                {
                    req_guard_gui::run(root)
                }
            }
            #[cfg(not(feature = "gui"))]
            {
                Err(GateError::Validation(format!(
                    "本次构建未包含 GUI（项目 {}）。请用 `cargo build -p req-guard --features gui（或 full）` 重新构建，\
                     或改用 req-guard ui --tui",
                    root.display()
                )))
            }
        }
    }
}

/// 身份：优先命令行参数，回退环境变量 `REQ_GUARD_REVIEWER`，都没有则报错。
fn resolve_identity(v: Option<&str>, label: &str, flag: &str) -> Result<String> {
    if let Some(s) = v {
        if !s.trim().is_empty() {
            return Ok(s.trim().to_string());
        }
    }
    if let Ok(s) = std::env::var("REQ_GUARD_REVIEWER") {
        if !s.trim().is_empty() {
            return Ok(s.trim().to_string());
        }
    }
    Err(GateError::Validation(format!(
        "缺少{}：请使用 {} <姓名>，或设置环境变量 REQ_GUARD_REVIEWER",
        label, flag
    )))
}

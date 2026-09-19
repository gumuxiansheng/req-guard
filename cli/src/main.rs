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
use req_guard_core::idcheck;
use req_guard_core::requirement;
use req_guard_core::status;
use std::path::{Path, PathBuf};

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
    let resolved = resolve_root(a);
    let root = resolved.as_path();
    // 方案 B：审批命令显式给 --token 时注入环境，供 core 的 ensure_human 校验。
    // （无 --token 则走 REQ_GUARD_TOKEN 环境变量；未启用令牌则回退方案 A）
    if let Some(t) = a.token.as_deref() {
        if !t.trim().is_empty() {
            std::env::set_var(req_guard_core::token::TOKEN_ENV, t);
        }
    }
    // 方案 C：审批显式声明带外渠道（供 core 校验与台账 channel 标注）。
    if a.oob {
        std::env::set_var(req_guard_core::auth::OOB_DECL_ENV, "1");
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
            // P3：自定义编号先过写法 lint（提示不阻断，规范 §8；自动编号无此步骤）。
            if let Some(raw) = a.id.as_deref() {
                for w in idcheck::lint_id(raw) {
                    eprintln!("⚠️ 编号提示（不阻断）：{}", w.message);
                }
            }
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
        Action::Done => {
            let id = a.id.as_deref().unwrap_or("");
            let actor = resolve_identity(a.author.as_deref(), "操作人", "--author")?;
            let r = requirement::done(root, id, &actor)?;
            println!("✅ 已归档：{} {}（操作人 {}）", r.id, r.title, actor);
            println!("   归档后门禁跳过该需求；新的开发请新建清单。");
        }
        Action::Status => {
            let id = a.id.as_deref();
            render::print_status(root, id)?;
            if let Some(i) = id {
                render::print_comment_summary(&status::req_get(root, i)?);
            }
        }
        Action::List => render::print_status(root, None)?,
        Action::Ids => {
            if a.check {
                // P1：三类检测的判定在 core（idcheck::check），这里只渲染与退出码。
                let issues = idcheck::check(root)?;
                if issues.is_empty() {
                    println!("✅ 需求编号防冲突检测通过：无重复 id、无自动编号污染、无前缀歧义");
                }
                for i in &issues {
                    let mark = if i.severity.is_error() {
                        "✗"
                    } else {
                        "⚠️"
                    };
                    println!("{} [{}] {}", mark, i.severity.as_str(), i.message);
                }
                let errs = issues.iter().filter(|i| i.severity.is_error()).count();
                let warns = issues.len() - errs;
                if errs > 0 {
                    eprintln!(
                        "共 {} 项硬伤 / {} 项告警；修复硬伤后重跑 req-guard ids --check",
                        errs, warns
                    );
                    std::process::exit(1);
                } else if warns > 0 {
                    println!("共 {} 项告警（不阻断）", warns);
                }
            } else {
                // 裸 `ids`：机器可读清单（<id>\t<文件名>），供脚本 grep / CI 汇总。
                let reqs = requirement::list(root)?;
                if reqs.is_empty() {
                    println!("（无需求清单）");
                }
                for r in &reqs {
                    let name = r
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if r.id.is_empty() {
                        println!("-\t{}", name);
                    } else {
                        println!("{}\t{}", r.id, name);
                    }
                }
            }
        }
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
        Action::HookCheck => {
            // 供拦截脚本第 0 段调用：Rust 侧真解析 AI 工具 payload（脚本的 sed 正则
            // 会被 Unicode 转义绕过）。拦截时打印原因并 exit 1，脚本据此阻断。
            use std::io::Read as _;
            let mut payload = String::new();
            std::io::stdin()
                .read_to_string(&mut payload)
                .map_err(|e| GateError::Validation(format!("读取 stdin 失败：{}", e)))?;
            match gate::pretool_verdict(root, &payload) {
                gate::PretoolVerdict::Block(reason) => {
                    eprintln!("[req-guard] ⛔ 拦截：{}", reason);
                    std::process::exit(1);
                }
                // 清单正文：脚本见标记即放行本次写（否则 AI 连正文都写不了）
                gate::PretoolVerdict::AllowDoc => println!("{}", gate::ALLOW_DOC_MARKER),
                gate::PretoolVerdict::Continue => {}
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

/// 决定项目根：显式 `-p/--path` 原样采用；否则向上探测含 `.gates/` 的目录。
///
/// 为什么需要探测：双击 `req-guard-ui.exe` 时 CWD 是 exe 所在目录，
/// 在子目录里跑 `req-guard status` 时 CWD 也不是项目根；两种场景下
/// 默认的 `"."` 都会指向错误目录，表现为「找不到需求 / 门禁看着没生效」。
///
/// 探测顺序：CWD → 可执行文件所在目录。都找不到则**保持 CWD**——
/// `init` 必须在"当前目录"建门禁，凭空跳到某个祖先目录是危险的。
fn resolve_root(a: &cli::Args) -> PathBuf {
    if a.root_explicit {
        return a.root.clone();
    }
    const MAX_UP: usize = 6;
    if let Some(p) = gate::find_project_root(&a.root, MAX_UP) {
        return p;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // exe 通常埋在 `gates-tools/req-guard-ui/bin` 这类深层目录，多给几层。
            if let Some(p) = gate::find_project_root(dir, MAX_UP + 4) {
                return p;
            }
        }
    }
    a.root.clone()
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

//! req-guard CLI 入口：参数解析 + 文本输出。
//!
//! 业务逻辑（清单生命周期、审核评论、门禁判定）全部在 `req-guard-core`；
//! 本 crate 只做参数翻译与文本渲染，**不做任何判定**——判定唯一真相在 core。

mod cli;
mod render;

use cli::Action;
use req_guard_core::ac;
use req_guard_core::comment;
use req_guard_core::error::{GateError, Result};
use req_guard_core::gate;
use req_guard_core::idcheck;
use req_guard_core::requirement;
use req_guard_core::status;
use req_guard_core::touch;
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
    // 审批凭据：--token 显式给出时注入**进程内**凭据（不再写环境变量——
    // 环境变量会被同 shell 会话的 AI 子进程继承，见 core/src/auth.rs 的 L2）。
    // 未给 --token 时，L0–L1 仍可由 core 回退读取 REQ_GUARD_TOKEN；L2 起只认此处。
    if let Some(t) = a.token.as_deref() {
        req_guard_core::auth::set_credential(Some(t.to_string()));
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
        Action::Amend => {
            let id = a.id.as_deref().unwrap_or("");
            let step = a.step.as_deref().unwrap_or("");
            let reviewer = resolve_identity(a.reviewer.as_deref(), "审核人", "--reviewer", root)?;
            let strict = gate::strict_order(root);
            let r = requirement::amend(root, id, step, &reviewer, comment_of(a), strict)?;
            // 明示"重审仍必需"：amend 与 reject 同构，**不豁免**重新批准。
            println!(
                "✅ 已请求修订：{} / {}（{}，提请人 {}）\n   摘要已清空、段已回到待审 —— 请改完重新 `approve`。",
                r.id, step, requirement::step_label(step), reviewer
            );
        }
        Action::Approve | Action::Reject => {
            let pass = matches!(a.action, Action::Approve);
            let id = a.id.as_deref().unwrap_or("");
            let step = a.step.as_deref().unwrap_or("");
            let reviewer = resolve_identity(a.reviewer.as_deref(), "审核人", "--reviewer", root)?;
            // 是否强制审核顺序，由 .gates/req-guard.yaml 的 strict_order 决定（缺失时 fail-closed = true）。
            let strict = gate::strict_order(root);
            let r = requirement::review(root, id, step, &reviewer, pass, comment_of(a), strict)?;
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
            let author = resolve_identity(a.author.as_deref(), "评论作者", "--author", root)?;
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
            let author = resolve_identity(a.author.as_deref(), "审核人", "--author", root)?;
            comment::resolve(root, id, cid, &author)?;
            println!("✅ 已关闭评论 {}（需求 {}，审核人 {}）", cid, id, author);
            render::print_comment_summary(&status::req_get(root, id)?);
        }
        Action::Done => {
            let id = a.id.as_deref().unwrap_or("");
            let actor = resolve_identity(a.author.as_deref(), "操作人", "--author", root)?;
            let r = requirement::done(root, id, &actor)?;
            println!("✅ 已归档：{} {}（操作人 {}）", r.id, r.title, actor);
            println!("   归档后门禁跳过该需求；新的开发请新建清单。");
            // 触发点：done 成功后自动清扫到期归档（best-effort，失败不影响 done 本身）。
            match requirement::archive_due_authorized(
                root,
                &actor,
                gate::archive_after_days(root),
                false,
            ) {
                Ok(list) if !list.is_empty() => {
                    println!(
                        "   已自动归档 {} 条（done 满 {} 天）：",
                        list.len(),
                        gate::archive_after_days(root)
                    );
                    for x in &list {
                        println!("     - {} → {}", x.id, x.dst.display());
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("   ⚠️ 到期归档失败（不影响本次 done）：{}", e),
            }
        }
        Action::Archive => {
            let actor = resolve_identity(a.author.as_deref(), "操作人", "--author", root)?;
            let list = match a.id.as_deref() {
                Some(id) => requirement::archive_one(root, &actor, id, a.dry_run)?,
                None => requirement::archive_due(
                    root,
                    &actor,
                    gate::archive_after_days(root),
                    a.dry_run,
                )?,
            };
            if list.is_empty() {
                if a.dry_run {
                    println!("没有到期可归档的 done 需求。");
                } else {
                    println!("没有到期可归档的 done 需求（archive.after_days 已满足才搬移）。");
                }
            } else if a.dry_run {
                println!("以下需求已到期，执行 archive 将搬入归档区（dry-run 未落盘）：");
                for x in &list {
                    println!("  {} → {}", x.id, x.dst.display());
                }
            } else {
                println!("✅ 已归档 {} 条（操作人 {}）：", list.len(), actor);
                for x in &list {
                    println!("   - {} → {}", x.id, x.dst.display());
                }
            }
        }
        Action::Status => {
            if a.archived {
                let reqs = status::req_list_archived(root)?;
                if reqs.is_empty() {
                    println!("归档区为空（.gates/requirements/archive/ 下无历史需求）。");
                } else {
                    println!("🏛  归档区（{} 条，只读历史）：", reqs.len());
                    render::print_req_list(&reqs);
                }
                return Ok(());
            }
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
            // 绕过是门禁上唯一的合法逃逸口：操作人缺省取 git 身份（不再是 "unknown"），
            // 否则 identity::bind 在 auth.level≥1 下会因身份对不上而拒绝绕过。
            let actor = resolve_identity(a.author.as_deref(), "操作人", "--author", root)
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
        Action::Whoami => {
            // 身份绑定的自查入口：让管理员/审核人能在审批前确认"我会以谁的身份落账"。
            let level = req_guard_core::auth::effective_level(root);
            let facts = req_guard_core::identity::collect(root);
            println!("仓库根  : {}", root.display());
            match &facts.git {
                Some(id) => {
                    println!("审批身份 : {} <{}>", id.name, id.email);
                    println!("指纹 sig : {}", id.sig);
                }
                None => println!("审批身份 : 未取到 git 身份（user.name 为空或不在 git 仓库内）"),
            }
            println!("auth 等级: L{}", level);
            if facts.git.is_none() {
                println!(
                    "           无 git 环境时可用环境变量兜底：{} 声明邮箱、REQ_GUARD_REVIEWER 声明姓名",
                    req_guard_core::identity::EMAIL_ENV
                );
            }
            if level == 0 {
                println!(
                    "           L0：身份冲突或缺失只放行并留痕（台账 mismatch=1）；\
                     要强制可归属请设 auth.level >= 1"
                );
            } else {
                println!(
                    "           L{}：--reviewer 与上面身份不一致（或取不到身份）时，审批会被直接拒绝",
                    level
                );
            }
            println!(
                "AI 上下文 : {}",
                if req_guard_core::auth::is_ai_context() {
                    "是（审批类命令会被拒绝）"
                } else {
                    "否"
                }
            );
        }
        Action::Token { ref sub } => run_token(root, a, sub)?,
        Action::Ac { ref sub } => run_ac(root, a, sub)?,
        Action::TouchCheck => run_touch_check(root, a)?,
        Action::VerifyContent => run_verify_content(root, a)?,
        Action::Seal => run_seal(root, a)?,
        Action::Touch { ref sub } => run_touch(root, a, sub)?,
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
    let level = req_guard_core::auth::effective_level(_root);
    match sub {
        "issue" => {
            // 审批锁：凭据是方案 B 的信任根，AI 若能自签即可"自签→自批"，
            // 故 issue 必须过 ensure_token_admin（已有凭据→须出示当前凭据；
            // L1+ → 须人类在真实终端完成挑战码，管道签不出来）。
            req_guard_core::auth::ensure_token_admin("token issue", _root)?;
            // L3：一律签发**一次性票据**（静态令牌会重放，L3 不再受理）。
            let (raw, expires, ttl, path, mode_note) = if level >= 3 {
                let scope = match (a.id.as_deref(), a.step.as_deref()) {
                    (Some(id), Some(step)) if !id.trim().is_empty() && !step.trim().is_empty() => {
                        format!("{}:{}", id.trim(), step.trim())
                    }
                    _ => String::new(), // 未绑定对象 → 通用单次票
                };
                let (raw, exp, ttl, path) = token::issue_scoped(a.ttl, &scope)?;
                let note = if scope.is_empty() {
                    "一次性票据（通用：可审任意一个需求/步骤，用后即废）".to_string()
                } else {
                    format!("一次性票据（绑定 {}，用后即废）", scope)
                };
                (raw, exp, ttl, path, note)
            } else {
                let (raw, exp, ttl, path) = token::issue(a.ttl)?;
                (
                    raw,
                    exp,
                    ttl,
                    path,
                    "短期令牌（TTL 内可重复使用）".to_string(),
                )
            };
            println!("✅ 已签发审批凭据（{}，有效期 {} 分钟）", mode_note, ttl);
            println!("   配置  : {}", path.display());
            println!("   ⚠️ 请立即复制下面这串原文——仅显示一次，不落盘：");
            println!("   ┌─ 凭据 ────────────────────────────────────");
            println!("   {}", raw);
            println!("   └──────────────────────────────────────────");
            println!("   到期 epoch : {}", expires);
            if !token::entropy_strong() {
                println!(
                    "   ⚠️ 本机无 /dev/urandom（Windows），凭据熵来自时间抖动 + ASLR 地址混合，\
                     强度低于内核 CSPRNG；\n\
                     \x20    guard.cfg 里的哈希与 AI 同用户可读，建议**缩短 TTL** 并妥善保管原文。"
                );
            }
            println!(
                "   用法 : req-guard approve REQ-001 --step decomposition --reviewer 张三 --token <凭据>"
            );
            if level < 2 {
                println!("         或先 export REQ_GUARD_TOKEN=<凭据> 后省略 --token");
            } else {
                println!(
                    "   注意 : 本机审批严格等级 L{}，凭据**只能**用 --token 显式传入（环境变量通道已禁用，\n\
                     \x20        因为它会被同一 shell 会话里的 AI 子进程继承）。",
                    level
                );
            }
            println!("   请把凭据存入密码管理器/自身会话，勿提交版本库、勿发给 AI。");
        }
        "status" => {
            println!(
                "审批严格等级 : L{}（.gates/req-guard.yaml 的 auth.level）",
                level
            );
            match token::load_any() {
                Some(cfg) => {
                    let state = if !cfg.enabled {
                        "已撤销/未启用"
                    } else if cfg.used {
                        "已消费（一次性票据用完即废）"
                    } else if cfg.expires_epoch <= req_guard_core::gate::now_epoch() {
                        "已过期"
                    } else {
                        "生效中"
                    };
                    let mode = match cfg.mode {
                        token::Mode::Static => "static（短期令牌）",
                        token::Mode::Scoped => "scoped（一次性票据）",
                    };
                    println!("凭据状态     : {}", state);
                    println!("凭据形态     : {}", mode);
                    if !cfg.scope.is_empty() {
                        println!("绑定范围     : {}", cfg.scope);
                    }
                    println!("到期 epoch   : {}", cfg.expires_epoch);
                    if !cfg.hash.is_empty() {
                        println!("哈希(前12)   : {}", &cfg.hash[..cfg.hash.len().min(12)]);
                    }
                    if token::load().is_none() {
                        println!("→ 当前无生效凭据：审批在 L1+ 下须靠真实终端挑战码，或由 GUI 进程内签发。");
                    }
                }
                None => {
                    println!("凭据状态     : 从未签发");
                    if level == 0 {
                        println!("→ L0：鉴权仅靠方案 A 软标记（REQ_GUARD_AI_CTX）。");
                    } else {
                        println!(
                            "→ L{}：审批须凭据或真实终端挑战码（AI 管道拿不出）。",
                            level
                        );
                    }
                }
            }
        }
        "revoke" => {
            // 同 issue：撤销凭据同样是审批类动作（AI 撤销后再自签即为绕过）。
            req_guard_core::auth::ensure_token_admin("token revoke", _root)?;
            let path = token::revoke()?;
            println!("✅ 已撤销并禁用审批凭据：{}", path.display());
            println!("   审批鉴权回退到本机严格等级决定的其他通道。");
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
/// 解析审批人/作者名：命令行 > 环境变量 > **git 身份**。
///
/// 最后一档取 git 身份（`git config user.name`）是刻意的：审批人本来就等于
/// 提交署名者，让人每次手打姓名既啰嗦又容易打错（打错会在 auth.level≥1 下
/// 被身份绑定判为冲突而拒绝——那是好事，但不该由"手打错字"触发）。
fn resolve_identity(
    v: Option<&str>,
    label: &str,
    flag: &str,
    root: &std::path::Path,
) -> Result<String> {
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
    if let Some(id) = req_guard_core::identity::current(root) {
        return Ok(id.name);
    }
    Err(GateError::Validation(format!(
        "缺少{}：请使用 {} <姓名>，或设置环境变量 REQ_GUARD_REVIEWER，\
         或配置 git 身份（git config user.name \"你的名字\"）后由 req-guard 自动取用",
        label, flag
    )))
}

/// `req-guard ac check`：验收标准机械校验。
///
/// 审批/修订的留痕正文：读 `--comment`（落 `text`），兼容 `--reason`。
///
/// 曾经的 bug：这里读的是 `a.reason`，而 `--comment` 解析进的是 `a.text` ——
/// 于是 `req-guard reject <ID> --step X --comment "..."` 的注释**被静默丢弃**，
/// 审核记录里只留下 `-`。留痕丢字比不记录更糟：它让人以为已经记下了。
/// 两者都读是为了兼容既有脚本里的 `--reason` 写法。
fn comment_of(a: &cli::Args) -> &str {
    a.text
        .as_deref()
        .or(a.reason.as_deref())
        .unwrap_or("")
        .trim()
}

/// 判定全在 core（`ac::check` → `ac::lint`），这里只做参数翻译、渲染与退出码 ——
/// 与 `ids --check` 同一套房屋风格（`{标记} [{严重级}] {message}`）。
/// **Error 全部排在 Warn 之前**：门禁唯一的静默降级口是「仅 Warn 也放行」，
/// 排序固定才能保证降级发生时人先看到硬伤。
fn run_ac(root: &Path, a: &cli::Args, sub: &str) -> Result<()> {
    match sub {
        "check" => {}
        other => {
            return Err(GateError::Validation(format!(
                "未知 ac 子命令: {}（可选 check）",
                other
            )))
        }
    }
    let target = match (a.id.as_deref(), a.archived) {
        (Some(id), _) => ac::AcTarget::Id(id),
        // `--all` 走 `--archived` 旗标复用：语义都是"含归档区，只读"
        (None, true) => ac::AcTarget::IncludingArchived,
        (None, false) => ac::AcTarget::All,
    };
    let issues = ac::check(root, &target)?;

    let errors: Vec<&ac::AcIssue> = issues.iter().filter(|i| i.severity.is_error()).collect();
    let warns: Vec<&ac::AcIssue> = issues.iter().filter(|i| !i.severity.is_error()).collect();
    // 按设计文档 §2.4 的输出格式渲染：`{标记} [{严重级}] [{Kind}] {message}`。
    // **Kind 必须打出来**：文案只有自然语言时，人无法判断命中的是哪条规则，
    // 也没法据此去查规则表（同一段正文可能同时命中 A1 与 EmptySection）。
    for i in errors.iter().chain(warns.iter()) {
        let mark = if i.severity.is_error() {
            "✗"
        } else {
            "⚠️"
        };
        println!(
            "{} [{}] [{}] {}",
            mark,
            i.severity.as_str(),
            i.kind_as_str(),
            i.message
        );
    }
    if issues.is_empty() {
        println!(
            "✅ 清单内容合规：第 3 段 AC 编号连续、Given/When/Then 齐备且可度量，三段均有实质正文"
        );
    }
    if !errors.is_empty() {
        eprintln!(
            "共 {} 项硬伤 / {} 项告警；修复硬伤后重跑 req-guard ac check",
            errors.len(),
            warns.len()
        );
        std::process::exit(1);
    } else if !warns.is_empty() {
        println!("共 {} 项告警（不阻断）", warns.len());
    }
    Ok(())
}

/// `req-guard touch-check`：变更范围契约判定（判定全在 [`touch::check`]）。
///
/// 两个变更集来源：
/// - 默认：读 `HOOK_STAGED_FILES`，未设则回落 `git diff --cached`（pre-commit 路径）
/// - `--base <ref>`：`git diff --name-only <ref>...HEAD`（CI / L3 路径）
///
/// `--base` 不是可选装饰：没有它，`git commit --no-verify` 就完全绕过了这道墙
/// （见设计文档 §3.6）。
fn run_touch_check(root: &Path, a: &cli::Args) -> Result<()> {
    let scope = if gate::touch_scope_strict(root) {
        touch::TouchScope::Strict
    } else {
        touch::TouchScope::Union
    };
    let src = match a.base.as_deref() {
        Some(b) => touch::Changed::Range(b.to_string()),
        None => touch::Changed::Staged(touch::staged_files(root)?),
    };
    let only = std::env::var("HOOK_REQ")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let issues = touch::check(root, scope, only.as_deref(), &src)?;
    let errs: Vec<&touch::TouchIssue> = issues.iter().filter(|i| i.severity.is_error()).collect();
    for i in &errs {
        println!(
            "✗ [{}] [{}] {}",
            i.severity.as_str(),
            i.kind.as_str(),
            i.message
        );
    }
    if issues.is_empty() {
        println!("✅ 变更范围合规：本次改动都在技术方案段的 GATE:TOUCH 声明范围内");
    } else {
        eprintln!(
            "共 {} 项硬伤；处置见上方三条出路（改方案 / touch --declare / --no-verify）",
            errs.len()
        );
        std::process::exit(1);
    }
    Ok(())
}

/// `req-guard touch --declare <glob>...`：向清单追加变更范围声明。
fn run_touch(root: &Path, a: &cli::Args, sub: &str) -> Result<()> {
    if sub != "--declare" {
        return Err(GateError::Validation(format!(
            "未知 touch 子命令: {}（可选 --declare）",
            sub
        )));
    }
    if a.globs.is_empty() {
        return Err(GateError::Validation(
            "用法：req-guard touch --declare --glob <路径> [--glob <glob>...] [--reason <原因>]"
                .into(),
        ));
    }
    let id = match a.id.as_deref() {
        Some(i) => i.to_string(),
        None => {
            // 缺省取"最新活跃需求"（与 HOOK_SH 第 2 段同规则）
            let reqs = requirement::list(root)?;
            let mut live: Vec<requirement::Requirement> = Vec::new();
            for r in reqs {
                let c = std::fs::read_to_string(&r.path).unwrap_or_default();
                if requirement::head_status(&c) != "done" {
                    live.push(r);
                }
            }
            live.sort_by(|x, y| y.path.file_name().cmp(&x.path.file_name()));
            match live.first() {
                Some(r) => r.id.clone(),
                None => {
                    return Err(GateError::Validation(
                        "没有未归档的需求清单可追加声明；请先 req-guard create".into(),
                    ))
                }
            }
        }
    };
    let actor = resolve_identity(a.author.as_deref(), "操作人", "--author", root)
        .unwrap_or_else(|_| "unknown".to_string());
    let added = touch::declare(
        root,
        &id,
        &a.globs,
        a.reason.as_deref().unwrap_or(""),
        &actor,
    )?;
    if added.is_empty() {
        println!("声明未变化：{id} 的 GATE:TOUCH 已包含全部给定条目");
        return Ok(());
    }
    println!("✅ 已向 {id} 的 GATE:TOUCH 追加 {} 条声明：", added.len());
    for p in &added {
        println!("   + {p}");
    }
    if gate::touch_reapprove(root) {
        println!("   技术方案已打回 pending —— 范围扩张须重新过审（req-guard approve {id} --step solution）");
    }
    Ok(())
}

/// `req-guard verify-content`：已批准段的正文冻结校验（判定在 [`requirement::verify_sums`]）。
///
/// 由 `.gates/hooks/req-guard-check.sh` 第 3.5 段调用，也可手工执行。
/// 退出码 1 表示"已批准段的正文与批准时不一致"—— 本机与 CI 共用这一份判定。
/// `req-guard verify-content`：已批准段的正文冻结校验（判定在 [`requirement::verify_sums`]）。
///
/// 由 `.gates/hooks/req-guard-check.sh` 第 3.5 段调用，也可手工执行。
/// 退出码 1 表示"已批准段的正文与批准时不一致" —— 本机与 CI 共用这一份判定。
fn run_verify_content(root: &Path, a: &cli::Args) -> Result<()> {
    let reqs = match a.id.as_deref() {
        // 容忍 `REQ-003.md`：脚本里拿到的就是带扩展名的文件名，人类也常这么敲。
        // 两种写法都拼成 `REQ-003.md.md` 的话，钩子会静默变成"永远查不到需求"。
        Some(id) => vec![requirement::find(
            root,
            id.strip_suffix(".md").unwrap_or(id),
        )?],
        None => requirement::list(root)?,
    };
    // 计数而非借用：SumIssue 逐清单生成，借用会短于循环体
    let mut errs = 0usize;
    let mut warns = 0usize;
    let mut checked = 0usize;
    for r in &reqs {
        let content = std::fs::read_to_string(&r.path).unwrap_or_default();
        if requirement::head_status(&content) == "done" {
            continue; // 已归档是生命周期终点，不再校验其摘要
        }
        if !requirement::frozen_sections(&content).is_empty() {
            checked += 1;
        }
        for i in requirement::verify_sums(&content) {
            let line = format!("{} {}", r.id, i.message);
            if i.severity.is_error() {
                errs += 1;
                println!("✗ [错误] [{}] {}", i.kind.as_str(), line);
            } else {
                warns += 1;
                println!("⚠️ [警告] [{}] {}", i.kind.as_str(), line);
            }
        }
    }
    if errs == 0 {
        if checked == 0 {
            println!(
                "⚠️ 没有任何已批准段绑定内容冻结（sum=-）：批准后正文可被改动而无人察觉。\n\
                 执行 req-guard seal <需求ID> 可绑定当前内容。"
            );
        } else {
            println!("✅ 内容冻结校验通过：{checked} 份清单的已批准段正文与批准时一致");
        }
    }
    if errs > 0 {
        eprintln!("共 {errs} 项硬伤；处置见上方文案（reject 后重审 / seal 重新绑定）");
        std::process::exit(1);
    } else if warns > 0 {
        println!("共 {warns} 项告警（不阻断）");
    }
    Ok(())
}

/// `req-guard seal`：绑定 `sum=` 到当前正文。AI 不得执行（`ensure_human` 会拦）。
fn run_seal(root: &Path, a: &cli::Args) -> Result<()> {
    let Some(spec) = a.id.as_deref() else {
        return Err(GateError::Validation(
            "用法：req-guard seal <需求ID> [<需求ID> ...]".into(),
        ));
    };
    // 支持一次 seal 多份清单：L3 票据用后即废，而"逐份换票"的摩擦大到会有人干脆
    // 不 seal —— 于是"已批准却无冻结"就成了默认态，等于把这道门-optional。
    let ids: Vec<String> = spec
        .split([',', ' ', '、'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let outcomes = if ids.iter().any(|i| i == "ALL") {
        requirement::seal_all(root, a.reason.as_deref().unwrap_or(""))?
    } else {
        requirement::seal_many(root, &ids, a.reason.as_deref().unwrap_or(""))?
    };
    if outcomes.is_empty() {
        println!("没有未归档的需求清单可绑定");
        return Ok(());
    }
    let mut n = 0;
    for o in &outcomes {
        n += o.bound.len();
        println!("✅ {} 已绑定 {} 段：", o.id, o.bound.len());
        for (label, sum) in &o.bound {
            println!("   {label:<8} sum={}…", &sum[..8]);
        }
    }
    println!("\n共绑定 {n} 段。今后改动这些段都需要 reject → 重审；确实无需重审时再次执行本命令。");
    Ok(())
}

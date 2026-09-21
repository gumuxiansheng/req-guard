//! req-guard CLI 解析（零依赖，手写）。
//!
//! 命令一览：
//! ```text
//! req-guard init                        初始化 .gates/ 门禁（脚本 + AI hook + pre-commit）
//! req-guard create   -t <标题>           创建需求清单
//! req-guard approve  <需求ID> --step <步骤> --reviewer <姓名> [--comment <意见>]
//! req-guard reject   <需求ID> --step <步骤> --reviewer <姓名> [--comment <意见>]
//! req-guard comment  <需求ID> --author <姓名> --text <意见> [--step] [--quote] [--blocking] [--reply C001]
//! req-guard resolve  <需求ID> <评论ID> --author <姓名>     （AI 禁止调用）
//! req-guard done     <需求ID> --author <姓名>      归档需求（拦截随之跳过）
//! req-guard status   [<需求ID>]          查看清单与解锁状态
//! req-guard list
//! req-guard ids [--check]               列出编号；--check 防冲突三类检测
//! req-guard comments <需求ID> [--refresh-anchors]
//! req-guard check                       手动执行拦截判定（退出码 0 放行 / 1 拦截）
//! req-guard install  [--tool <a,b>] [--verify]       安装/修复拦截；--verify 只校验（CI 用）
//! req-guard bypass   --reason <原因> [--ttl 60]
//! req-guard audit-digest                 审计日志 SHA-256 摘要写入入库 DIGEST
//! req-guard hook-check                   PreToolUse hook 内部命令（读 stdin，由拦截脚本调用）
//! req-guard -V | --version              输出版本号（与 Cargo.toml / Release tag 一致）
//! ```

use std::path::PathBuf;

pub enum Action {
    Init,
    Create,
    Approve,
    Reject,
    Comment,
    Resolve,
    /// 归档需求：整体状态置 done，拦截与 check 随之跳过该需求（AI 禁止调用）。
    Done,
    /// 到期物理归档：done 满 `archive.after_days` 天的清单搬入
    /// `.gates/requirements/archive/<年>/`（AI 禁止调用；`done` 后自动触发）。
    Archive,
    Status,
    List,
    /// 需求编号工具：`ids` 列出全部编号；`ids --check` 执行防冲突三类检测
    /// （同 id 多文件 / 自动编号污染 / 前缀歧义，有硬伤退出码 1）。
    Ids,
    Comments,
    Check,
    Install,
    Bypass,
    /// 生成审计摘要：本机 gate-audit.log 的 SHA-256 → 入库 DIGEST（PR 可比对）。
    AuditDigest,
    /// 审批令牌管理（方案 B）：`token <issue|status|revoke>`。
    Token {
        sub: String,
    },
    /// 打开门禁管理台（TUI / GUI，按构建 feature 与运行环境自动选择）。
    Ui,
    /// PreToolUse hook 用：读 stdin 的 AI 工具 payload，做**证据保护**判定。
    /// 退出码 0 放行（交给后续门禁）、1 拦截（脚本据此 exit 1）。
    HookCheck,
}

pub struct Args {
    pub action: Action,
    pub root: PathBuf,
    /// `-p/--path` 是否由用户显式给出。
    ///
    /// 为 false 时 main 会向上探测项目根（双击 exe / 在子目录里执行的场景，
    /// CWD 往往不是项目根）；显式指定则**绝不**改动用户的意图。
    pub root_explicit: bool,
    pub id: Option<String>,
    pub comment_id: Option<String>,
    pub title: Option<String>,
    pub step: Option<String>,
    pub reviewer: Option<String>,
    pub author: Option<String>,
    pub text: Option<String>,
    pub quote: Option<String>,
    pub reply: Option<String>,
    pub blocking: bool,
    pub reason: Option<String>,
    pub tools: Vec<String>,
    pub ttl: u64,
    pub refresh_anchors: bool,
    /// 审批令牌（方案 B）：approve/reject/resolve/bypass 提供以通过鉴权。
    pub token: Option<String>,
    /// 带外审批声明（方案 C）：approve/reject/resolve/bypass 显式声明来自带外渠道。
    pub oob: bool,
    /// `install --verify`：只校验门禁就位情况（CI 用），不写入任何文件。
    pub verify: bool,
    /// `ids --check`：执行编号防冲突三类检测（而非仅列出编号）。
    pub check: bool,
    /// `ui --gui`：强制图形界面。
    pub gui: bool,
    /// `ui --tui`：强制终端界面。
    pub tui: bool,
    /// `status --archived`：展示归档区历史需求。
    pub archived: bool,
    /// `archive --dry-run`：只列出将归档项，不搬移、不写审计。
    pub dry_run: bool,
}

pub struct Parsed {
    pub args: Args,
}

pub fn parse() -> std::result::Result<Parsed, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // -V/--version 短路处理：输出版本号后正常退出（退出码 0）。
    // 版本号取自编译期的 CARGO_PKG_VERSION，即 Cargo.toml 的版本，
    // 因此「Release tag == 二进制自报版本」可直接用它校验（见 scripts/build-release.sh）。
    if matches!(argv.first().map(String::as_str), Some("-V" | "--version")) {
        println!("req-guard {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    // `req-guard oob <命令>`：带外前端——等价于 `<命令> --oob`（方案 C 渠道声明）。
    if matches!(argv.first().map(String::as_str), Some("oob")) {
        let mut v = argv[1..].to_vec();
        v.push("--oob".to_string());
        return parse_from(&v);
    }
    parse_from(&argv)
}

/// 构造某动作的默认值集合。
///
/// 抽出来是为了复用：无参数调用（双击 exe）时也要能造出一份 `Args`。
fn default_args(action: Action) -> Args {
    Args {
        action,
        root: PathBuf::from("."),
        root_explicit: false,
        id: None,
        comment_id: None,
        title: None,
        step: None,
        reviewer: None,
        author: None,
        text: None,
        quote: None,
        reply: None,
        blocking: false,
        reason: None,
        tools: Vec::new(),
        ttl: 60,
        refresh_anchors: false,
        token: None,
        oob: false,
        verify: false,
        check: false,
        gui: false,
        tui: false,
        archived: false,
        dry_run: false,
    }
}

fn parse_from(args: &[String]) -> std::result::Result<Parsed, String> {
    let mut it = args.iter().peekable();
    let tok = match it.next() {
        Some(t) => t.clone(),
        None => {
            // 无参数：对**带界面的变体**（req-guard-ui.exe，以 --features tui/gui 构建）
            // 直接打开管理台——双击 exe 时不给参数才是常态，此时打印帮助并 exit 2
            // 会表现为"控制台一闪而过、什么都没发生"。
            // 纯 CLI 变体（无界面 feature）保持原行为：打印帮助。
            if cfg!(any(feature = "tui", feature = "gui")) {
                return Ok(Parsed {
                    args: default_args(Action::Ui),
                });
            }
            return Err(help());
        }
    };
    let action = match tok.as_str() {
        "init" => Action::Init,
        "create" => Action::Create,
        "approve" => Action::Approve,
        "reject" => Action::Reject,
        "comment" => Action::Comment,
        "resolve" => Action::Resolve,
        "done" => Action::Done,
        "archive" => Action::Archive,
        "status" => Action::Status,
        "list" => Action::List,
        "ids" => Action::Ids,
        "comments" => Action::Comments,
        "check" => Action::Check,
        "install" => Action::Install,
        "bypass" => Action::Bypass,
        "audit-digest" => Action::AuditDigest,
        "token" => {
            // subcommand：token <issue|status|revoke>
            let sub = match it.peek() {
                Some(s) if !s.starts_with('-') => it.next().unwrap().clone(),
                _ => return Err("token 需要子命令: issue | status | revoke".into()),
            };
            match sub.as_str() {
                "issue" | "status" | "revoke" => Action::Token { sub },
                other => {
                    return Err(format!(
                        "未知 token 子命令: {}（可选 issue | status | revoke）",
                        other
                    ))
                }
            }
        }
        "ui" => Action::Ui,
        "hook-check" => Action::HookCheck,
        "-h" | "--help" => return Err(help()),
        other => return Err(format!("未知命令: {}\n\n{}", other, help())),
    };

    let mut a = default_args(action);

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-t" | "--title" => a.title = Some(next(&mut it, "--title")?),
            "--step" => a.step = Some(next(&mut it, "--step")?),
            "--reviewer" => a.reviewer = Some(next(&mut it, "--reviewer")?),
            "--author" => a.author = Some(next(&mut it, "--author")?),
            "--text" => a.text = Some(next(&mut it, "--text")?),
            "--quote" => a.quote = Some(next(&mut it, "--quote")?),
            "--reply" => a.reply = Some(next(&mut it, "--reply")?),
            "--comment" => a.text = Some(next(&mut it, "--comment")?),
            "--reason" => a.reason = Some(next(&mut it, "--reason")?),
            "--blocking" => a.blocking = true,
            "--refresh-anchors" => a.refresh_anchors = true,
            "--verify" => a.verify = true,
            "--check" => a.check = true,
            "--gui" => a.gui = true,
            "--tui" => a.tui = true,
            "--archived" => a.archived = true,
            "--dry-run" => a.dry_run = true,
            "--tool" => {
                let v = next(&mut it, "--tool")?;
                for p in v.split(',') {
                    let t = p.trim();
                    if !t.is_empty() {
                        a.tools.push(t.to_string());
                    }
                }
            }
            "--ttl" => {
                let v = next(&mut it, "--ttl")?;
                a.ttl = v
                    .parse()
                    .map_err(|_| format!("--ttl 需为整数分钟，当前: {}", v))?;
            }
            "--token" => a.token = Some(next(&mut it, "--token")?),
            "--oob" => a.oob = true,
            "-p" | "--path" => {
                a.root = PathBuf::from(next(&mut it, "--path")?);
                a.root_explicit = true;
            }
            "-h" | "--help" => return Err(help()),
            other => {
                if other.starts_with('-') {
                    return Err(format!("未知参数: {}\n\n{}", other, help()));
                }
                if a.id.is_none() {
                    a.id = Some(other.to_string());
                } else if a.comment_id.is_none() {
                    a.comment_id = Some(other.to_string());
                } else {
                    return Err(format!("多余的位置参数: {}", other));
                }
            }
        }
    }
    validate(&a)?;
    Ok(Parsed { args: a })
}

fn validate(a: &Args) -> std::result::Result<(), String> {
    if a.verify && !matches!(a.action, Action::Init | Action::Install) {
        return Err("--verify 仅用于 install（例：req-guard install --verify）".into());
    }
    match a.action {
        Action::Create => {
            if a.title.is_none() {
                return Err("create 需要 -t/--title <需求标题>".into());
            }
        }
        Action::Approve | Action::Reject => {
            if a.id.is_none() {
                return Err("需要指定需求 ID，例如：req-guard approve REQ-001 --step decomposition --reviewer 张三".into());
            }
            if a.step.is_none() {
                return Err("需要 --step <decomposition|solution|testplan>".into());
            }
        }
        Action::Comment => {
            if a.id.is_none() {
                return Err("需要指定需求 ID".into());
            }
            if a.text.is_none() {
                return Err("comment 需要 --text <意见内容>".into());
            }
        }
        Action::Resolve => {
            if a.id.is_none() || a.comment_id.is_none() {
                return Err("用法：req-guard resolve <需求ID> <评论ID> --author <姓名>".into());
            }
        }
        Action::Done => {
            if a.id.is_none() {
                return Err("用法：req-guard done <需求ID> --author <姓名>".into());
            }
        }
        Action::Bypass if a.reason.is_none() => {
            return Err("bypass 必须填写 --reason <原因>（用于审计追溯）".into());
        }
        Action::Ui if a.gui && a.tui => {
            return Err("--gui 与 --tui 不能同时使用（不指定则自动探测）".into());
        }
        _ => {}
    }
    Ok(())
}

fn next<'a, I>(it: &mut std::iter::Peekable<I>, flag: &str) -> std::result::Result<String, String>
where
    I: Iterator<Item = &'a String>,
{
    match it.next() {
        Some(v) => Ok(v.clone()),
        None => Err(format!("参数 {} 缺少取值", flag)),
    }
}

fn help() -> String {
    "req-guard <命令> [选项]   （AI 需求门禁：三段清单审核 + 硬拦截）\n\
\n\
命令:\n\
  init                       初始化 .gates/ 门禁（脚本 + AI 工具 hook + pre-commit）\n\
  create   -t <标题>         创建需求清单（REQ-001…）\n\
  approve  <需求ID> --step <步骤> --reviewer <姓名> [--comment <意见>]\n\
  reject   <需求ID> --step <步骤> --reviewer <姓名> [--comment <意见>]\n\
  comment  <需求ID> --author <姓名> --text <意见> [--step <步骤>]\n\
                    [--quote <原文片段>] [--blocking] [--reply <评论ID>]\n\
  resolve  <需求ID> <评论ID> --author <姓名>    关闭评论（AI 禁止调用）\n\
  done     <需求ID> --author <姓名>    归档需求：拦截随之跳过（AI 禁止调用）\n\
  archive  [<需求ID>] --author <姓名> [--dry-run]\n\
                                     到期物理归档：done 满 archive.after_days 天的\n\
                                     清单搬入 archive/<年>/（done 成功后自动触发；\n\
                                     此命令用于手动补扫；--dry-run 只预览不搬移）\n\
  status   [<需求ID>] [--archived]   查看三段状态与是否解锁；--archived 展示归档历史\n\
  list                       列出全部需求\n\
  ids      [--check]         列出需求编号（<id>\t<文件名>）；--check 防冲突三类检测\n\
                             （同 id 多文件 / 自动编号污染 / 前缀歧义；硬伤退出码 1，CI 可挂）\n\
  comments <需求ID> [--refresh-anchors]         查看评论 / 重算行号锚点\n\
  check                      手动拦截判定（退出码 0 放行 / 1 拦截）\n\
  install  [--tool <a,b>] [--verify]           安装或修复拦截；--verify 只校验就位情况（CI 用）\n\
  bypass   --reason <原因> [--ttl 60]           有时效的应急绕过（强制审计）\n\
  audit-digest               审计日志 SHA-256 摘要写入入库 DIGEST（PR 可比对）\n\
  token issue  [--ttl 60]   签发审批令牌（方案 B，原文仅打印一次，请带外保存）\n\
  token status               查看令牌启用状态与到期\n\
  token revoke               撤销并禁用审批令牌\n\
  ui       [--gui | --tui]   打开门禁管理台（需以 --features tui 构建）\n\
  hook-check                 PreToolUse hook 内部命令：读 stdin 校验 AI 写操作\n\
\n\
通用选项:\n\
  -p, --path <项目根>   默认当前目录\n\
  -V, --version         输出版本号\n\
  -h, --help            输出本帮助\n\
  --token <令牌>        审批令牌（方案 B）；或用 REQ_GUARD_TOKEN\n\
  --oob                 声明带外审批渠道（方案 C；亦可写 req-guard oob <命令>）\n\
\n\
步骤: decomposition(需求分解) -> solution(技术方案) -> testplan(测试计划)\n\
规则: 三段全部 approved 且无未解决的阻塞性评论，AI 才被允许编写代码。\n"
        .into()
}

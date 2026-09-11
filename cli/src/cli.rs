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
//! req-guard status   [<需求ID>]          查看清单与解锁状态
//! req-guard list
//! req-guard comments <需求ID> [--refresh-anchors]
//! req-guard check                       手动执行拦截判定（退出码 0 放行 / 1 拦截）
//! req-guard install  [--tool <a,b>]
//! req-guard bypass   --reason <原因> [--ttl 60]
//! ```

use std::path::PathBuf;

pub enum Action {
    Init,
    Create,
    Approve,
    Reject,
    Comment,
    Resolve,
    Status,
    List,
    Comments,
    Check,
    Install,
    Bypass,
    /// 打开门禁管理台（TUI / GUI，按构建 feature 与运行环境自动选择）。
    Ui,
}

pub struct Args {
    pub action: Action,
    pub root: PathBuf,
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
    /// `ui --gui`：强制图形界面。
    pub gui: bool,
    /// `ui --tui`：强制终端界面。
    pub tui: bool,
}

pub struct Parsed {
    pub args: Args,
}

pub fn parse() -> std::result::Result<Parsed, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    parse_from(&argv)
}

fn parse_from(args: &[String]) -> std::result::Result<Parsed, String> {
    let mut it = args.iter().peekable();
    let tok = match it.next() {
        Some(t) => t.clone(),
        None => return Err(help()),
    };
    let action = match tok.as_str() {
        "init" => Action::Init,
        "create" => Action::Create,
        "approve" => Action::Approve,
        "reject" => Action::Reject,
        "comment" => Action::Comment,
        "resolve" => Action::Resolve,
        "status" => Action::Status,
        "list" => Action::List,
        "comments" => Action::Comments,
        "check" => Action::Check,
        "install" => Action::Install,
        "bypass" => Action::Bypass,
        "ui" => Action::Ui,
        "-h" | "--help" => return Err(help()),
        other => return Err(format!("未知命令: {}\n\n{}", other, help())),
    };

    let mut a = Args {
        action,
        root: PathBuf::from("."),
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
        gui: false,
        tui: false,
    };

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
            "--gui" => a.gui = true,
            "--tui" => a.tui = true,
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
            "-p" | "--path" => a.root = PathBuf::from(next(&mut it, "--path")?),
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
  status   [<需求ID>]        查看三段状态与是否解锁\n\
  list                       列出全部需求\n\
  comments <需求ID> [--refresh-anchors]         查看评论 / 重算行号锚点\n\
  check                      手动拦截判定（退出码 0 放行 / 1 拦截）\n\
  install  [--tool <a,b>]    安装或修复拦截\n\
  bypass   --reason <原因> [--ttl 60]           有时效的应急绕过（强制审计）\n\
  ui       [--gui | --tui]   打开门禁管理台（需以 --features tui 构建）\n\
\n\
通用选项:\n\
  -p, --path <项目根>   默认当前目录\n\
\n\
步骤: decomposition(需求分解) -> solution(技术方案) -> testplan(测试计划)\n\
规则: 三段全部 approved 且无未解决的阻塞性评论，AI 才被允许编写代码。\n"
        .into()
}

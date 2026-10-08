//! req-guard CLI 解析（零依赖，手写）。
//!
//! 命令一览：
//! ```text
//! req-guard init                        初始化 .gates/ 门禁（脚本 + AI hook + pre-commit）
//! req-guard create   -t <标题>           创建需求清单
//! req-guard approve  <需求ID> --step <步骤> [--reviewer <姓名>] [--comment <意见>]
//! req-guard approve  <需求ID> --all-steps               一条命令批三段（轻档；三段各留一条台账）
//! req-guard reject   <需求ID> --step <步骤> [--reviewer <姓名>] [--comment <意见>]
//! req-guard amend    <需求ID> --step <步骤> --comment <意见>   （修订：回退待审+清摘要，必须重审）
//! req-guard comment  <需求ID> --author <姓名> --text <意见> [--step] [--quote] [--blocking] [--reply C001]
//! req-guard resolve  <需求ID> <评论ID> --author <姓名>     （AI 禁止调用）
//! req-guard done     <需求ID> --author <姓名>      归档需求（拦截随之跳过）
//! req-guard status   [<需求ID>]          查看清单与解锁状态
//! req-guard list
//! req-guard ids [--check]               列出编号；--check 防冲突三类检测
//! req-guard comments <需求ID> [--refresh-anchors]
//! req-guard check                       手动执行拦截判定（退出码 0 放行 / 1 拦截）
//! req-guard ac check [<需求ID>] [--all]  验收标准机械校验（A1–A12，见 core/src/ac.rs）
//! req-guard touch-check [--base <ref>]     变更范围契约（见 core/src/touch.rs）
//! req-guard tier check [--staged | --base <ref>]  分级门禁：算档位并输出理由（REQ-019）
//! req-guard touch --declare <glob>... [--reason <原因>]
//! req-guard ac check [<需求ID>] [--all]  验收标准机械校验（core/src/ac.rs 的 A1–A12）
//! req-guard install  [--tool <a,b>] [--verify]       安装/修复拦截；--verify 只校验（CI 用）
//! req-guard bypass   --reason <原因> [--ttl 60]
//! req-guard audit-digest                 审计日志 SHA-256 摘要写入入库 DIGEST
//! req-guard whoami                       打印审批身份（git 身份 + sig + auth 等级）
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
    /// 打印本仓库的审批身份（git 身份 + 指纹 sig + 当前 auth 等级）。
    Whoami,
    /// 审批令牌管理（方案 B）：`token <issue|status|revoke>`。
    Token {
        sub: String,
    },
    /// 验收标准机械校验：`ac check`（A1–A12，见 core/src/ac.rs）。
    Ac {
        sub: String,
    },
    /// 变更范围契约：`touch-check`（判定）/ `touch --declare`（扩张范围）。
    Touch {
        sub: String,
    },
    /// `touch-check`：判定实际改动 ⊆ `GATE:TOUCH` 声明并集。
    TouchCheck,
    /// `verify-content`：校验已批准段的正文仍与批准时一致（`HOOK_SH` 第 3.5 段调用）。
    VerifyContent,
    /// `seal`：把已批准段的 `sum=` 绑定到当前正文（人类专属；存量清单迁移用）。
    Seal,
    /// 修订：与 reject 同构，台账记 AMEND（REQ-004 G1）
    Amend,
    /// 一次命令完成「读草稿 → 写正文 → 重新批准」（REQ-007）
    Apply,
    /// 打开门禁管理台（TUI / GUI，按构建 feature 与运行环境自动选择）。
    Ui,
    /// PreToolUse hook 用：读 stdin 的 AI 工具 payload，做**证据保护**判定。
    /// 退出码 0 放行（交给后续门禁）、1 拦截（脚本据此 exit 1）。
    HookCheck,
    /// 分级门禁：`tier check`（按变更集算档位，输出档位与理由）。
    Tier {
        sub: String,
    },
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
    /// `init --for-ci`：沙箱/自检模式——审批等级降到 L0 且显式声明放弃 L3。
    /// 仅供「门禁机制自检」用；真实项目用裸 `init`（保持 L3）。
    pub for_ci: bool,
    /// `install --verify --quick`：只跑子串快筛，跳过实跑语义自检（本地高频调用）。
    pub quick: bool,
    /// `token revoke --i-lost-it`：承认当前凭据原文已丢失，走恢复路径
    /// （须人类在场，且强制记入入库台账 REVOKE-FORCED）。
    pub i_lost_it: bool,
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
    /// `touch-check --base <ref>`：改用「相对该 ref 的差异」作变更集（L3 / CI 路径）。
    pub base: Option<String>,
    /// `check --staged`：以已暂存文件集作变更集（pre-commit 路径）。
    pub staged: bool,
    /// `check --stdin`：从 stdin 的 AI 工具 payload 取 `file_path` 作变更集（PreToolUse 路径）。
    pub stdin: bool,
    /// `check --req <需求ID>`：显式指定「本次改动属于哪份需求」（消歧用）。
    ///
    /// 与 `id` 分开：位置参数 `check REQ-001` 会被拒（`check` 不接位置参数），
    /// 否则「`id` 有值但被忽略」是个静默失效 —— 看起来指定了需求，其实没生效。
    pub req: Option<String>,
    /// `touch --declare` 的路径 / glob（可重复；刻意不复用 `--tool`，那个是工具名）。
    pub globs: Vec<String>,
    /// `approve --all-steps`：一条命令批三段（轻档审批形态，REQ-019 §2.5）。
    pub all_steps: bool,
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
        for_ci: false,
        quick: false,
        i_lost_it: false,
        check: false,
        gui: false,
        tui: false,
        archived: false,
        dry_run: false,
        base: None,
        staged: false,
        stdin: false,
        req: None,
        globs: Vec::new(),
        all_steps: false,
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
        "whoami" => Action::Whoami,
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
        "ac" => {
            // subcommand：ac check
            let sub = match it.peek() {
                Some(s) if !s.starts_with('-') => it.next().unwrap().clone(),
                _ => return Err("ac 需要子命令: check".into()),
            };
            match sub.as_str() {
                "check" => Action::Ac { sub },
                other => {
                    return Err(format!("未知 ac 子命令: {}（可选 check）", other));
                }
            }
        }
        "touch" => {
            // 与 `token` 不同：`touch` 的子命令本身就是 flag（`--declare`），
            // 所以不能沿用"只接受不以 `-` 开头的子命令"那条 —— 沿用会导致
            // `touch --declare` 永远解析不出来。
            let sub = match it.peek() {
                Some(_) => it.next().unwrap().clone(),
                None => return Err("touch 需要子命令: --declare".into()),
            };
            match sub.as_str() {
                "--declare" => Action::Touch { sub },
                other => {
                    return Err(format!("未知 touch 子命令: {}（可选 --declare）", other));
                }
            }
        }
        "tier" => {
            // subcommand：`tier check`（REQ-019 §2.8）。
            let sub = match it.peek() {
                Some(s) if !s.starts_with('-') => it.next().unwrap().clone(),
                _ => return Err("tier 需要子命令: check".into()),
            };
            match sub.as_str() {
                "check" => Action::Tier { sub },
                other => {
                    return Err(format!("未知 tier 子命令: {}（可选 check）", other));
                }
            }
        }
        "touch-check" => Action::TouchCheck,
        "verify-content" => Action::VerifyContent,
        "seal" => Action::Seal,
        "amend" => Action::Amend,
        "apply" => Action::Apply,
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
            "--for-ci" => a.for_ci = true,
            "--quick" => a.quick = true,
            "--i-lost-it" => a.i_lost_it = true,
            "--check" => a.check = true,
            "--gui" => a.gui = true,
            "--tui" => a.tui = true,
            "--archived" => a.archived = true,
            // `ac check --all`：与 --archived 同义（含归档区，只读）。
            // 单独起个名是因为 CI 模板里读起来更直白，不必知道 --archived 的历史含义。
            "--all" => a.archived = true,
            "--dry-run" => a.dry_run = true,
            "--base" => a.base = Some(next(&mut it, "--base")?),
            "--staged" => a.staged = true,
            "--stdin" => a.stdin = true,
            "--req" => a.req = Some(next(&mut it, "--req")?),
            "--glob" => a.globs.push(next(&mut it, "--glob")?),
            "--all-steps" => a.all_steps = true,
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
            // `--all-steps` 只对 approve 有意义：reject 的语义是「打回某一段」，
            // 一次打回三段与「整份打回」在状态机里不是一回事，故拒。
            if a.all_steps {
                if !matches!(a.action, Action::Approve) {
                    return Err("--all-steps 仅用于 approve（打回请逐段 --step）".into());
                }
                if a.step.is_some() {
                    return Err("--all-steps 与 --step 互斥：前者批三段，后者批一段".into());
                }
            } else if a.step.is_none() {
                return Err(
                    "需要 --step <decomposition|solution|testplan>（或用 --all-steps 一次批三段）"
                        .into(),
                );
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
        Action::Check | Action::Tier { .. } => {
            // 变更集来源三选一。两个来源混在一起判必然产生无法解释的裁决，
            // 而裁决出错时人根本看不出是哪一条规则导致的 —— 直接拒，不猜。
            let sources = [
                a.staged.then_some("--staged"),
                a.base.as_ref().map(|_| "--base"),
                a.stdin.then_some("--stdin"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if sources.len() > 1 {
                return Err(format!(
                    "变更集来源互斥：{} 只能选一个（--staged / --base <ref> / --stdin）",
                    sources.join(" 与 ")
                ));
            }
            if a.id.is_some() {
                return Err("check 不接受位置参数；指定需求请用 --req <需求ID>".into());
            }
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
  approve  <需求ID> --step <步骤> [--reviewer <姓名>] [--comment <意见>]
  approve  <需求ID> --all-steps               一次批三段（轻档审批形态）：原子性 + 三条台账
                             （channel=quick）+ 需 scope 为 <需求ID>:* 的通配票据；
                              三段实质正文仍强制，AC 选填但写了必须全量合规\n\
  reject   <需求ID> --step <步骤> [--reviewer <姓名>] [--comment <意见>]
  amend    <需求ID> --step <步骤> --comment <意见>    修订（回退待审+清摘要，必须重审）\n\
                             --reviewer 可省略：缺省取 git 身份（user.name）\n\\
  apply    <需求ID> --step <步骤> --comment <意见>    一次完成修订：读 .gates/drafts/<需求ID>.draft.md\n\
                             写入正文 + 重新批准 + 绑定新摘要（AI 禁止执行）\n\
                             草稿通道三条约束：① 草稿**只写散文**（GATE 块由 req-guard 维护，\n\
                             块内条目请直接编辑清单或用 amend）；② **一次只应用一段**\n\
                             （多段草稿会被拒绝）；③ 草稿**整段重写**（短于原段会被拒绝，\n\
                             长出的行落在该段末尾，无法在段中间插入）\n\
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
  check [--staged | --base <ref> | --stdin] [--req <需求ID>]\n\
                             门禁裁决（退出码 0 放行 / 1 拦截）。裁决对象是**本次变更集**：\n\
                             按各需求技术方案段的 GATE:TOUCH 声明反查归属，只判相关的那几份。\n\
                             --staged 以已暂存文件集为变更集（pre-commit）；--base <ref> 用\n\
                             相对该 ref 的差异（CI / L3）；--stdin 从 stdin 的 AI 工具\n\
                             payload 取 file_path（PreToolUse）。三者互斥；都不给 = 全局判定。\n\
                             --req <需求ID> 显式指定归属（消歧用；亦可写 HOOK_REQ=<ID>）\n\
  ac check [<需求ID>]        验收标准机械校验（A1–A12；硬伤退出码 1）\n\
  ac check --all             同上，且含归档区（审计用，只读）\n\
  tier check [--staged | --base <ref>]     分级门禁：按变更集算档位（只读）
                             输出档位、逐文件有效行、命中 glob、声明档 vs 派生档；
                              只改注释/空行 → 有效行 0 → 免审档；豁免区不参与定档
  touch-check [--base <ref>] 变更范围契约：实际改动 ⊆ GATE:TOUCH 声明并集\n\
                             （touch.scope=strict 时只比「本次改动归属的那一份」，\n\
                               归属不唯一即报错，不猜）\n\
  verify-content [<需求ID>]  校验已批准段正文未被改动（pre-commit 内部调用）\n\
  seal <需求ID> [...]       把已批准段的 sum= 绑定到当前正文（AI 禁止执行）
                           已绑定过的清单须加 --reason <原因>（记 RESEAL 事件）
  amend <需求ID> --step <步骤> --comment <意见>
                           修订已批准的段（回退待审 + 清摘要，必须重审）\n\
  touch --declare --glob <路径> [--glob <glob>...] [--reason <原因>]\n\
                             扩张声明范围（AI 禁止；会打回技术方案重审）\n\
  install  [--tool <a,b>] [--verify]           安装或修复拦截；--verify 只校验就位情况（CI 用）\n\
  bypass   --reason <原因> [--ttl 60]           有时效的应急绕过（强制审计）\n\
  audit-digest               审计日志 SHA-256 摘要写入入库 DIGEST（PR 可比对）\n\
  whoami                     打印本仓库审批身份（git 身份 + sig 指纹 + auth 等级）\n\
  token issue  [<需求ID>] [--step <步骤>] [--ttl 60]\n\
                              签发审批凭据（L3 下为一次性票据，可绑定需求+步骤；\n\
                              原文仅打印一次，请带外保存）\n\
  token status               查看严格等级与凭据状态\n\
  token revoke               撤销并禁用审批凭据\n\
  ui       [--gui | --tui]   打开门禁管理台（需以 --features tui 构建）\n\
  hook-check                 PreToolUse hook 内部命令：读 stdin 校验 AI 写操作\n\
\n\
通用选项:\n\
  -p, --path <项目根>   默认当前目录\n\
  -V, --version         输出版本号\n\
  -h, --help            输出本帮助\n\
  --token <凭据>        审批凭据；L0–L1 亦可用环境变量 REQ_GUARD_TOKEN\n\
  --oob                 声明带外审批渠道（方案 C；亦可写 req-guard oob <命令>）\n\
\n\
步骤: decomposition(需求分解) -> solution(技术方案) -> testplan(测试计划)\n\
规则: 三段全部 approved 且无未解决的阻塞性评论，AI 才被允许编写代码。
多需求: .gates/req-guard.yaml 的 multi.mode=resolve（默认）按本次变更集反查归属，
       只判相关的那几份；mode=all 为保守档（全部清单都得批）。
       multi.bind_branch=true 时分支名里的 REQ-<id> 可用于消歧（永不覆盖反查结果）。
"
        .into()
}

//! 文本渲染：把 core 的结构化状态拼成人类可读输出。
//!
//! **判定与渲染分离**：core 只提供数据（[`ReqStatus`]），本模块只负责排版。
//! 换界面（TUI / GUI）时无需改 core，也就不会出现"界面不同、结论不同"。

use req_guard_core::error::Result;
use req_guard_core::status::{self, ReqStatus};
use std::path::Path;

/// 渲染 `status` / `list` 命令的输出（单个需求或全部需求）。
pub fn print_status(root: &Path, id: Option<&str>) -> Result<()> {
    let reqs: Vec<ReqStatus> = match id {
        Some(i) => vec![status::req_get(root, i)?],
        None => status::req_list(root)?,
    };
    if reqs.is_empty() {
        println!("尚未创建任何需求清单。");
        println!("  创建：req-guard create -t \"<需求标题>\"");
        return Ok(());
    }
    print_req_list(&reqs);
    Ok(())
}

/// 渲染一批状态卡片（活跃列表与 `--archived` 归档列表共用）。
pub fn print_req_list(reqs: &[ReqStatus]) {
    for r in reqs {
        print_one(r);
    }
}

/// 渲染单条需求的状态卡片。
pub fn print_one(r: &ReqStatus) {
    println!("需求 {} {}", r.id, r.title);
    println!("  文件 : {}", r.path.display());
    println!("  状态 : {}", r.state.as_str());
    println!("  步骤 :");
    for s in &r.steps {
        let mark = if s.state.is_approved() { "✓" } else { " " };
        match &s.reviewer {
            Some(rv) => println!(
                "    [{}] {:<8} {}（审核人: {}）",
                mark,
                s.key,
                s.state.as_str(),
                rv
            ),
            None => println!("    [{}] {:<8} {}", mark, s.key, s.state.as_str()),
        }
    }
    if r.unlocked {
        println!("  判定 : 已解锁，AI 可以开始编写代码");
    } else {
        println!("  判定 : 未解锁，AI 不得编写/修改源码（门禁拦截）");
    }
    print_rework(r);
    println!();
}

/// 返工率（REQ-004 G4 / T2）：只在**有修订记录**时输出，避免刷屏。
///
/// 只统计不阻断 —— 反复返工的段是"方案当初没想清楚"的信号，需要复盘；
/// 把它做成门禁只会催生"少写 amend 刷分"。
fn print_rework(r: &ReqStatus) {
    let counts = req_guard_core::requirement::amend_counts(&r.root, &r.id);
    let hot: Vec<String> = counts
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(k, n)| format!("{} 修订{}次", k, n))
        .collect();
    if hot.is_empty() {
        return;
    }
    println!("  返工 : {}", hot.join(" / "));
}

/// 渲染评论摘要（仅在存在未解决评论时输出，保证审核意见必然被看到）。
pub fn print_comment_summary(r: &ReqStatus) {
    if r.open_comments == 0 {
        return;
    }
    if r.blocking_comments > 0 {
        println!(
            "   评论 : {} 条待处理（其中 {} 条阻塞，将拦截编码）",
            r.open_comments, r.blocking_comments
        );
    } else {
        println!("   评论 : {} 条待处理（非阻塞）", r.open_comments);
    }
}

/// 渲染门禁裁决（`check` 命令）。
///
/// **拦截原因与放行明细一律走 stderr**（AI 与人都要看得见）：
/// 放行时也要打明细，因为里面可能有内容冻结告警 —— 告警是门禁唯一的静默降级口，
/// 丢了就等于「看起来有冻结、实际没有」。
///
/// 绕过标记走 **stdout**：脚本改为委托 `check` 之后，脚本的 stdout 仍须带它 ——
/// 外部工具靠读脚本 stdout 判断「本次放行是不是靠绕过」的那条契约不能破。
pub fn print_verdict(v: &req_guard_core::gate::GateVerdict) {
    match v {
        req_guard_core::gate::GateVerdict::Pass {
            summary,
            detail,
            bypassed,
        } => {
            println!("{}", summary);
            if *bypassed {
                println!("{}", req_guard_core::gate::BYPASS_MARKER);
            }
            for line in detail {
                eprintln!("{}", line);
            }
        }
        req_guard_core::gate::GateVerdict::Block { summary, detail } => {
            eprintln!("{}", summary);
            for line in detail {
                eprintln!("{}", line);
            }
        }
    }
}

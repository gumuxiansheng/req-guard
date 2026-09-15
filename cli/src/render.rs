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
    for r in &reqs {
        print_one(r);
    }
    Ok(())
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
    println!();
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
/// 拦截原因原样透传到 stderr，保证与脚本输出一致（AI 与人都要看得见）。
///
/// 放行时**仅**在命中绕过窗口才打印脚本明细：正常放行保持既有输出不变，
/// 而"靠绕过放行"必须让审核人与 CI 日志看见，不能只剩一句"三段已批准"。
pub fn print_verdict(v: &req_guard_core::gate::GateVerdict) {
    match v {
        req_guard_core::gate::GateVerdict::Pass {
            summary,
            detail,
            bypassed,
        } => {
            println!("{}", summary);
            if *bypassed {
                for line in detail {
                    eprintln!("{}", line);
                }
            }
        }
        req_guard_core::gate::GateVerdict::Block { detail, .. } => {
            for line in detail {
                eprintln!("{}", line);
            }
        }
    }
}

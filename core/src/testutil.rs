//! 仅测试使用的临时目录工具。
//!
//! 项目坚持**零外部依赖**，因此不引入 `tempfile`——这里用「进程内自增序号 +
//! 进程 ID」拼出唯一目录名，测试结束由调用方 `remove_dir_all` 清理。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 创建一个空的唯一临时目录，供用例做真机文件读写。
pub fn temp_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "req-guard-test-{}-{}-{}",
        tag,
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("创建测试临时目录失败");
    p
}

/// 用例收尾：尽力清理，失败不影响断言结果。
pub fn cleanup(dir: &std::path::Path) {
    let _ = fs::remove_dir_all(dir);
}

/// 测试用：把项目审批严格等级降到 L0。
///
/// 用途：`install` 写出的模板默认 `auth.level: 3`（新项目一律最严），而多数单测
/// 验的是门禁/归档等**非鉴权**行为，不该依赖"人类凭据"（那需要真实终端或 guard.cfg）。
/// 追加一个 `auth:` 块即可——[`crate::gate::auth_level`] 以最后一次出现的 `level:` 为准。
pub fn disable_auth(root: &std::path::Path) {
    set_auth_level(root, 0);
}

/// 测试用：把项目审批严格等级设为指定值（含 [`disable_auth`] 的 L0）。
///
/// [`crate::gate::auth_level`] 以最后一次出现的 `level:` 为准，故重复追加安全。
pub fn set_auth_level(root: &std::path::Path, level: u8) {
    let y = root.join(".gates/req-guard.yaml");
    // 允许在只建了临时根目录（尚无 .gates/）的场景直接调用
    if let Some(dir) = y.parent() {
        fs::create_dir_all(dir).expect("创建测试 .gates 目录失败");
    }
    let mut s = fs::read_to_string(&y).unwrap_or_default();
    s.push_str(&format!("\nauth:\n  level: {}\n", level));
    fs::write(&y, s).expect("写入测试 yaml 失败");
}

/// 给清单三段各填一行实质正文。
///
/// 段落实质性门禁（`core/src/section.rs`）之后，"刚 `create` 出来、还没填内容的
/// 清单"不再能通过 `approve` —— 这是设计意图。于是凡是用 `create` + `approve`
/// 夹具来测**别的东西**（审核顺序、归档、身份绑定、审计）的用例，都得先填一段。
/// 统一走这个助手，避免每个用例各写一份填充逻辑、哪天又漏一处。
pub fn fill_sections(root: &std::path::Path, id: &str) {
    let p = crate::requirement::find(root, id).expect("清单应存在").path;
    let mut c = std::fs::read_to_string(&p).expect("清单应可读");
    for (heading, line) in [
        ("## 1. 需求分解", "- 背景与问题：本用例的测试夹具。"),
        ("## 2. 技术方案", "- 总体思路：本用例的测试夹具。"),
        ("## 3. 测试计划", "- 测试计划：本用例的测试夹具。"),
    ] {
        let needle = format!("{heading}\n");
        assert!(c.contains(&needle), "模板结构变了：找不到 {heading}");
        c = c.replacen(&needle, &format!("{needle}\n{line}\n"), 1);
    }
    std::fs::write(&p, c).expect("清单应可写");
}

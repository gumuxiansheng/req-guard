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
    let y = root.join(".gates/req-guard.yaml");
    let mut s = fs::read_to_string(&y).unwrap_or_default();
    s.push_str("\nauth:\n  level: 0\n");
    fs::write(&y, s).expect("写入测试 yaml 失败");
}

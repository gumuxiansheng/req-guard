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

#!/usr/bin/env python3
"""req-guard 拦截脚本真机验证。

从 `src/gate.rs` 抽取 `HOOK_SH`，在临时目录里构造 10 个场景实跑，校验退出码。
脚本必须与仓库一起版本化（原先放在 target/ 下，cargo clean 就丢）。

用法：
    python scripts/verify_gate.py
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# Windows CI（GitHub runners）默认用 cp1252：stdout 编码中文会 UnicodeEncodeError，
# 子进程管道按 ANSI 解码中文输出会 UnicodeDecodeError（崩在 readerthread 里）。
# 故无论平台一律显式 UTF-8；errors="replace" 保证控制台编码异常时也不至于中断。
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):  # 极旧 Python / 流已被替换
        pass

# 子进程管道解码同样必须显式指定（text=True 会用 locale 编码 → Windows 上必崩）
RUN_KW = {"encoding": "utf-8", "errors": "replace"}

ROOT = Path(__file__).resolve().parent.parent
# HOOK_SH 常量随业务逻辑住在 core（workspace 拆分后）。
SRC = ROOT / "core" / "src" / "gate.rs"
REQ_DIR = ".gates/requirements"
HOOK_REL = ".gates/hooks/req-guard-check.sh"

# 模拟 AI 工具 PreToolUse：AI 试图直接改写审核评论文件
STDIN_AI_WRITE_COMMENTS = (
    '{"tool_name":"Write","tool_input":'
    '{"file_path":".gates/requirements/REQ-001.comments.md"}}'
)

# 同上，但把后缀里的 '.' 写成 Unicode 转义 \u002e —— 与明文**完全等价**。
# 脚本兜底分支用 sed 抠字段，抠到的是原始串（不以 .comments.md 结尾）→ 静默放过；
# Rust 侧真解析会解出真实路径并拦截。场景 14 锁的就是这个差距。
STDIN_AI_WRITE_COMMENTS_ESCAPED = (
    '{"tool_name":"Write","tool_input":'
    '{"file_path":".gates/requirements/REQ-001.comments\\u002emd"}}'
)

# 场景 14 需要 req-guard 本体在 PATH 上（脚本第 0 段优先调 `req-guard hook-check`）。
# 未构建二进制时跳过该场景——不给"只跑脚本"的用法增加新前置条件。
BIN_DIR = ROOT / "target" / "debug"
BIN = BIN_DIR / ("req-guard.exe" if sys.platform == "win32" else "req-guard")


def make_req(status: str) -> str:
    """造一份三段同状态的清单。"""
    lines = [
        "# REQ-001 用户登录改造",
        "",
        "<!-- GATE:HEAD id=REQ-001 status=draft created=2026-09-09 -->",
    ]
    for name, label in (
        ("decomposition", "需求分解"),
        ("solution", "技术方案"),
        ("testplan", "测试计划"),
    ):
        lines.append(
            f"<!-- GATE:STEP name={name} label={label} "
            f"status={status} reviewer=- updated=- -->"
        )
    return "\n".join(lines) + "\n"


def make_comments(blocking: bool) -> str:
    return (
        "# REQ-001 审核评论\n\n<!-- GATE:COMMENTS req=REQ-001 -->\n"
        f"<!-- GATE:COMMENT id=C001 step=solution author=kou ts=2026-09-09 "
        f"state=open blocking={str(blocking).lower()} line=42 reply=- stale=false -->\n"
        "> quote: 回滚方案\n\n需要明确数据库迁移回退步骤。\n<!-- /GATE:COMMENT -->\n"
        "<!-- /GATE:COMMENTS -->\n"
    )


def extract_const(name: str) -> str:
    """从 gate.rs 抽取任意 `pub const NAME: &str = r#"..."#;` 常量。"""
    text = SRC.read_text(encoding="utf-8")
    m = re.search(rf'pub const {name}: &str = r#"(.*?)"#;', text, re.S)
    if not m:
        sys.exit(f"未能从 src/gate.rs 抽取 {name}")
    return m.group(1)


HOOK = extract_const("HOOK_SH")
DENY_HOOK = extract_const("DENY_SH")
# Windows CI 的 POSIX shell 由 Git for Windows 提供；缺失时给出可操作提示，
# 而不是抛一串 FileNotFoundError 让人以为是脚本 bug。
SH = shutil.which("sh")
if not SH:
    sys.exit(
        "未找到 POSIX shell（sh）：本脚本实跑 .sh 拦截脚本，"
        "Windows 请确认 Git for Windows 已安装且 sh 在 PATH 中。"
    )


def run(name, req_content, comments=None, bypass=False, stdin_data=None, extra=None, env=None):
    """在独立临时目录里跑一次拦截脚本，返回退出码。"""
    work = Path(tempfile.mkdtemp(prefix="reqguard-")) / name
    (work / Path(HOOK_REL).parent).mkdir(parents=True)
    (work / REQ_DIR).mkdir(parents=True)
    (work / HOOK_REL).write_text(HOOK, encoding="utf-8")

    if req_content:
        (work / REQ_DIR / "REQ-001.md").write_text(req_content, encoding="utf-8")
    if comments:
        (work / REQ_DIR / "REQ-001.comments.md").write_text(comments, encoding="utf-8")
    for fname, content in (extra or {}).items():
        (work / REQ_DIR / fname).write_text(content, encoding="utf-8")
    if bypass:
        expires = int(time.time()) + 3600
        (work / ".gates" / ".bypass").write_text(
            f"reason=hotfix\nactor=kou\ncreated_epoch=0\nexpires_epoch={expires}\n",
            encoding="utf-8",
        )

    if stdin_data is not None:
        # 管道 → 非 tty：脚本会读取 stdin JSON（AI 工具 PreToolUse 形态）
        r = subprocess.run(
            [SH, str(work / HOOK_REL)],
            cwd=work,
            capture_output=True,
            text=True,
            input=stdin_data,
            env=env,
            **RUN_KW,
        )
    else:
        # DEVNULL 防止 pre-commit/手动 check 场景下 `cat` 挂起
        r = subprocess.run(
            [SH, str(work / HOOK_REL)],
            cwd=work,
            capture_output=True,
            text=True,
            stdin=subprocess.DEVNULL,
            env=env,
            **RUN_KW,
        )
    shutil.rmtree(work.parent, ignore_errors=True)
    return r.returncode


partial = make_req("pending").replace(
    "name=decomposition label=需求分解 status=pending",
    "name=decomposition label=需求分解 status=approved",
)

CASES = [
    # name, 清单内容, 评论内容, 是否开绕过, stdin, 期望退出码, 额外文件
    ("1_无需求清单", None, None, False, None, 1, {}),
    ("2_三段未审核", make_req("pending"), None, False, None, 1, {}),
    ("3_仅分解通过", partial, None, False, None, 1, {}),
    ("4_三段全通过", make_req("approved"), None, False, None, 0, {}),
    ("5_绕过窗口内", make_req("pending"), None, True, None, 0, {}),
    ("6_AI写评论文件", make_req("approved"), None, False, STDIN_AI_WRITE_COMMENTS, 1, {}),
    ("7_阻塞评论未解决", make_req("approved"), make_comments(True), False, None, 1, {}),
    ("8_非阻塞评论仅提示", make_req("approved"), make_comments(False), False, None, 0, {}),
    (
        # 孤立评论文件名排序靠前，绝不能被当成"活跃需求"（否则空转拦截）
        "9_孤立评论文件不干扰放行",
        make_req("approved"),
        None,
        False,
        None,
        0,
        {"REQ-999.comments.md": "# REQ-999 审核评论\n"},
    ),
    (
        # 逃逸阀不得覆盖证据完整性：绕过窗口内仍必须拦下 AI 改写评论文件
        "10_绕过不覆盖证据保护",
        make_req("approved"),
        None,
        True,
        STDIN_AI_WRITE_COMMENTS,
        1,
        {},
    ),
]

ok = True
for name, req, cmts, bp, stdin_data, expect, extra in CASES:
    rc = run(name, req, cmts, bp, stdin_data, extra)
    good = rc == expect
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  {name}: exit={rc} (期望 {expect})")


def verify_pre_commit_fail_closed() -> bool:
    """11_pre_commit缺失脚本即拦截（fail-closed，§4.2）。

    从 gate.rs 抽取 PRE_COMMIT_BLOCK 拼成 pre-commit 脚本，
    在"没有 .gates/hooks/req-guard-check.sh"的目录里实跑：必须 exit 1。
    """
    text = SRC.read_text(encoding="utf-8")
    m = re.search(r"const PRE_COMMIT_BLOCK: &str = r#\"(.*?)\"#;", text, re.S)
    if not m:
        print("FAIL  11_pre_commit缺失脚本即拦截: 未能抽取 PRE_COMMIT_BLOCK")
        return False
    work = Path(tempfile.mkdtemp(prefix="reqguard-pc-"))
    pc = work / ".git" / "hooks" / "pre-commit"
    pc.parent.mkdir(parents=True)
    pc.write_text("#!/bin/sh\n" + m.group(1), encoding="utf-8")
    # 刻意不创建 .gates/hooks/req-guard-check.sh → 门禁脚本缺失
    r = subprocess.run(
        [SH, str(pc)], cwd=work, capture_output=True, text=True,
        stdin=subprocess.DEVNULL, **RUN_KW,
    )
    shutil.rmtree(work, ignore_errors=True)
    good = r.returncode == 1
    print(
        f"{'PASS' if good else 'FAIL'}  11_pre_commit缺失脚本即拦截: "
        f"exit={r.returncode} (期望 1)"
    )
    return good


ok = ok and verify_pre_commit_fail_closed()


def verify_deny_wrapper() -> bool:
    """12/13_deny 包装拦截编码（Codex/Cursor 路径）。

    未过审时 check.sh 返回 1，deny 包装必须转成工具能识别的拒绝 exit 2；
    已过审时 check.sh 返回 0，deny 包装必须原样放行 exit 0。
    """
    results = []
    for name, status, expect in (
        ("12_deny包装拦截转exit2", "pending", 2),
        ("13_deny包装放行转exit0", "approved", 0),
    ):
        work = Path(tempfile.mkdtemp(prefix="reqguard-deny-"))
        (work / Path(HOOK_REL).parent).mkdir(parents=True)
        (work / REQ_DIR).mkdir(parents=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        deny_rel = ".gates/hooks/req-guard-deny.sh"
        (work / deny_rel).parent.mkdir(parents=True, exist_ok=True)
        (work / deny_rel).write_text(DENY_HOOK, encoding="utf-8")
        (work / REQ_DIR / "REQ-001.md").write_text(make_req(status), encoding="utf-8")
        r = subprocess.run(
            [SH, str(work / deny_rel)], cwd=work, capture_output=True, text=True,
            stdin=subprocess.DEVNULL, **RUN_KW,
        )
        shutil.rmtree(work, ignore_errors=True)
        good = r.returncode == expect
        results.append(good)
        print(
            f"{'PASS' if good else 'FAIL'}  {name}: exit={r.returncode} (期望 {expect})"
        )
    return all(results)


ok = ok and verify_deny_wrapper()

# 14) AI 用 Unicode 转义绕过"禁止改评论文件"：Rust 真解析必须拦下。
#     三段已批准（否则会被门禁拦下，就证明不了是证据保护在起作用）。
name14 = "14_转义路径改写评论文件"
if BIN.exists():
    env14 = dict(os.environ)
    env14["PATH"] = str(BIN_DIR) + os.pathsep + env14.get("PATH", "")
    code14 = run(
        name14,
        make_req("approved"),
        stdin_data=STDIN_AI_WRITE_COMMENTS_ESCAPED,
        env=env14,
    )
    good = code14 == 1
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  {name14}: exit={code14} (期望 1)")

    doc_rel = ".gates/requirements/REQ-001.md"
    body = make_req("approved")

    # 15) AI 填写清单正文（状态行原样）→ 必须放行。
    #     否则"AI 填三段正文"这步会被门禁自己拦死，文档流程走不下去。
    payload15 = json.dumps(
        {"tool_input": {"file_path": doc_rel, "content": body}}, ensure_ascii=False
    )
    name15 = "15_AI填写清单正文"
    code15 = run(name15, body, stdin_data=payload15, env=env14)
    good = code15 == 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  {name15}: exit={code15} (期望 0)")

    # 16) AI 顺手改掉 status → 拦（自批）。磁盘已是 approved，若不是被本层拦下，
    #     门禁本会放行（exit 0），故 exit=1 只可能来自防自批这一层。
    tampered = body.replace("status=approved", "status=pending", 1)
    payload16 = json.dumps(
        {"tool_input": {"file_path": doc_rel, "content": tampered}}, ensure_ascii=False
    )
    name16 = "16_AI篡改状态行"
    code16 = run(name16, body, stdin_data=payload16, env=env14)
    good = code16 == 1
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  {name16}: exit={code16} (期望 1)")
else:
    print(f"SKIP  {name14}: 未构建 {BIN}（先执行 cargo build）")

print("\n结论:", "全部通过" if ok else "存在失败")
sys.exit(0 if ok else 1)

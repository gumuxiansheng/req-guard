#!/usr/bin/env python3
"""req-guard 拦截脚本真机验证。

从 `core/src/gate.rs` 抽取 `HOOK_SH`，在临时目录里构造场景实跑，校验退出码。
脚本必须与仓库一起版本化（原先放在 target/ 下，cargo clean 就丢）。

用法:
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
TOUCH_REL = ".gates/hooks/req-guard-touch-check.sh"

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
TOUCH_HOOK = extract_const("HOOK_TOUCH_SH")
DENY_HOOK = extract_const("DENY_SH")
# Windows CI 的 POSIX shell 由 Git for Windows 提供；缺失时给出可操作提示，
# 而不是抛一串 FileNotFoundError 让人以为是脚本 bug。
SH = shutil.which("sh")
if not SH:
    sys.exit(
        "未找到 POSIX shell（sh）：本脚本实跑 .sh 拦截脚本，"
        "Windows 请确认 Git for Windows 已安装且 sh 在 PATH 中。"
    )


def run(name, req_content, comments=None, bypass=False, stdin_data=None, extra=None, env=None,
        git=True):
    """在独立临时目录里跑一次拦截脚本，返回退出码。

    REQ-006 P2 起脚本**只取参与渲染**，判定在 `req-guard check`（core），
    故本函数必须做两件事，否则每个场景都会 exit 1（假通过）：

    1. `git init`：无变更集上下文时脚本走 `check --staged`，它回落
       `git diff --cached` —— 非 git 目录会报错并被当成拦截。空索引的
       `git diff --cached` 退出码为 0 且输出为空，于是单需求仓库的语义
       与改造前逐字一致（这正是 G4 兼容性门槛要的）。
    2. 把 `BIN_DIR` 塞进 PATH：判定在 core，缺二进制即 fail-closed。
    """
    work = Path(tempfile.mkdtemp(prefix="reqguard-")) / name
    (work / Path(HOOK_REL).parent).mkdir(parents=True)
    (work / REQ_DIR).mkdir(parents=True)
    (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
    if git:
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
    env = dict(env or os.environ)
    env["PATH"] = str(BIN_DIR) + os.pathsep + env.get("PATH", "")

    if req_content:
        (work / REQ_DIR / "REQ-001.md").write_text(req_content, encoding="utf-8")
    if comments:
        (work / REQ_DIR / "REQ-001.comments.md").write_text(comments, encoding="utf-8")
    for fname, content in (extra or {}).items():
        # extra 键可为相对 REQ_DIR 的子路径（如 archive/2026/REQ-00x.md）
        dst = work / REQ_DIR / fname
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(content, encoding="utf-8")
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
    (
        # 归档（done）需求被跳过：REQ-002 更新且未审但 GATE:HEAD status=done，
        # 脚本必须跳过它、回到已批准的 REQ-001 → 放行。
        # 若不跳过会选中 REQ-002（三段 pending）→ 恒拦截，done 命令即失效。
        "17_归档需求被门禁跳过",
        make_req("approved"),
        None,
        False,
        None,
        0,
        {
            "REQ-002.md": make_req("pending")
            .replace("REQ-001", "REQ-002")
            .replace("status=draft", "status=done")
        },
    ),
    (
        # 到期物理归档后的子目录（archive/<年>/）必须被脚本**无视**：
        # 文件未审（pending）但只要在子目录里就不该被选为活跃需求。
        # 若 `ls | grep '\.md$'` 递归进子目录 → 会选中它 → 恒拦截，
        # 归档功能就把自己锁死了（方案选子目录正是因为四处扫描点都不递归）。
        "18_归档子目录不干扰活跃需求定位",
        make_req("approved"),
        None,
        False,
        None,
        0,
        {
            "archive/2026/REQ-003.md": make_req("pending")
            .replace("REQ-001", "REQ-003")
        },
    ),
]

def verify_section_gate() -> bool:
    """27–28 段落实质性场景（REQ-003）。

    判定在 core（`req-guard ac check`），这里只验「二进制在 PATH 时接线正确」。
    规则的细粒度覆盖在 `cargo test -p req-guard-core section` 里。
    """
    if not BIN.exists():
        print("SKIP  27-28_段落实质性: 未构建 req-guard 二进制")
        return True

    ok = True
    for name, fill, expect in [
        ("27_三段全空报EmptySection", False, 1),
        ("28_三段有实质正文则放行", True, 0),
    ]:
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{name}"))
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        r = subprocess.run(
            [str(BIN), "init", "-p", ".", "--tool", "none"],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        if r.returncode != 0:
            print(f"FAIL  {name}: init 失败: {r.stderr[:200]}")
            ok = False
            continue
        subprocess.run(
            [str(BIN), "create", "-p", ".", "-t", "段落实质性"],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        docs = sorted((work / REQ_DIR).glob("REQ-*.md"))
        if not docs:
            print(f"FAIL  {name}: 未生成清单")
            ok = False
            continue
        doc = docs[0]
        content = doc.read_text(encoding="utf-8")
        if fill:
            # 三段各填一行：跨行注释与标题都不算，实质行才算
            for heading in ("## 1. 需求分解", "## 2. 技术方案", "## 3. 测试计划"):
                content = content.replace(
                    heading + "\n", heading + "\n\n- 实质内容一行。\n", 1
                )
            doc.write_text(content, encoding="utf-8")
        r = subprocess.run(
            [str(BIN), "ac", "check", "-p", "."],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        good = r.returncode == expect
        ok = ok and good
        print(f"{'PASS' if good else 'FAIL'}  {name}: exit={r.returncode} (期望 {expect})")
        if not good:
            print(f"      ↳ {(r.stdout + r.stderr)[:300]}")
    return ok


SUM_FIELD = "sum=" + "a" * 64


def verify_content_freeze() -> bool:
    """29–31 内容冻结场景（REQ-002）。

    29/30 用 `req-guard verify-content` 直接验判定；31 验脚本在**二进制缺失**时
    fail-closed —— 那是最容易漏的一格（看起来在拦，实际因为找不到命令而恒拦）。
    """
    if not BIN.exists():
        print("SKIP  29-31_内容冻结: 未构建 req-guard 二进制")
        return True

    ok = True
    req = (
        "# REQ-001 内容冻结\n\n"
        "<!-- GATE:HEAD id=REQ-001 status=approved created=2026-01-01 -->\n"
        "<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=t updated=- "
        + SUM_FIELD + " -->\n"
        "<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=t updated=- "
        + SUM_FIELD + " -->\n"
        "<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=t updated=- "
        + SUM_FIELD + " -->\n\n"
        "## 1. 需求分解\n\n- 背景：内容冻结演示。\n\n"
        "## 2. 技术方案\n\n<!-- GATE:TOUCH -->\nsrc/**\n<!-- /GATE:TOUCH -->\n"
        "\n- 思路：原始内容。\n\n"
        "## 3. 测试计划\n\n<!-- GATE:AC -->\n### AC-001\n"
        "- Given: 已批准\n- When: 执行 check\n- Then: 退出码 0\n<!-- /GATE:AC -->\n"
        "\n- 用例：见上。\n\n## 审核记录\n"
    )

    # 摘要要对得上才算"一致"，故先让 req-guard 自己 seal 一遍再改内容
    for name, mutate, expect in [
        ("29_已批准段摘要一致则放行", False, 0),
        ("30_已批准段被改则拦截", True, 1),
    ]:
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{name}"))
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        subprocess.run(
            [str(BIN), "init", "-p", ".", "--tool", "none"],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        (work / REQ_DIR).mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).parent.mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        (work / REQ_DIR / "REQ-001.md").write_text(req, encoding="utf-8")
        # 用真实 seal 绑定当前正文（沙箱 L0）
        yaml = work / ".gates" / "req-guard.yaml"
        if yaml.exists():
            yaml.write_text(
                yaml.read_text(encoding="utf-8").replace("level: 3", "level: 0"),
                encoding="utf-8",
            )
        subprocess.run(
            [str(BIN), "seal", "REQ-001", "--reason", "测试夹具绑定", "-p", "."],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        if mutate:
            f = work / REQ_DIR / "REQ-001.md"
            f.write_text(
                f.read_text(encoding="utf-8").replace("- 思路：原始内容。", "- 思路：偷偷改。"),
                encoding="utf-8",
            )
        r = subprocess.run(
            [str(BIN), "verify-content", "-p", "."],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        good = r.returncode == expect
        ok = ok and good
        print(f"{'PASS' if good else 'FAIL'}  {name}: exit={r.returncode} (期望 {expect})")
        if not good:
            print(f"      ↳ {(r.stdout + r.stderr)[:300]}")

    # 31_二进制缺失 → 脚本 fail-closed（不能因为找不到命令就静默放行）
    work = Path(tempfile.mkdtemp(prefix="reqguard-31"))
    (work / REQ_DIR).mkdir(parents=True)
    (work / HOOK_REL).parent.mkdir(parents=True, exist_ok=True)
    (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
    (work / REQ_DIR / "REQ-001.md").write_text(req, encoding="utf-8")
    env = dict(os.environ)
    env["PATH"] = ""  # 彻底没有 req-guard
    r = subprocess.run(
        [SH, str(work / HOOK_REL)],
        cwd=work, capture_output=True, text=True,
        stdin=subprocess.DEVNULL, env=env, **RUN_KW,
    )
    good = r.returncode == 1 and "req-guard" in (r.stdout + r.stderr)
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  31_二进制缺失fail_closed: exit={r.returncode} (期望 1 且提示缺二进制)")
    return ok


def verify_fenced_boundary() -> bool:
    """38–40 REQ-008：代码围栏内的 `## ` 不得成为段落边界。

    38 是本缺陷的原始现场（起草 REQ-007 时在正文里展示 `## solution` 格式，
    TOUCH 块被判"消失"）；39 是变长围栏；40 锁住"围栏外标题仍算边界"，
    防止掩码把真边界也吃掉（那会让三段永远分不开）。
    """
    if not BIN.exists():
        print("SKIP  38-40_围栏边界: 未构建 req-guard 二进制")
        return True

    ok = True

    def sandbox(tag: str, solution_body: str) -> Path:
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{tag}"))
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        subprocess.run(
            [str(BIN), "init", "-p", ".", "--tool", "none"],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        (work / REQ_DIR).mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).parent.mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        (work / REQ_DIR / "REQ-001.md").write_text(
            "# REQ-001 围栏边界\n\n"
            "<!-- GATE:HEAD id=REQ-001 status=approved created=2026-01-01 -->\n"
            + "".join(
                f"<!-- GATE:STEP name={n} label={l} status=approved reviewer=t updated=- -->\n"
                for n, l in (
                    ("decomposition", "需求分解"),
                    ("solution", "技术方案"),
                    ("testplan", "测试计划"),
                )
            )
            + "\n## 1. 需求分解\n\n- 背景：围栏边界演示。\n\n"
            "## 2. 技术方案\n\n"
            + solution_body
            + "\n<!-- GATE:TOUCH -->\ncore/**\n<!-- /GATE:TOUCH -->\n"
            "\n## 3. 测试计划\n\n<!-- GATE:AC -->\n### AC-001\n"
            "- Given: 一份三段均已批准的技术方案段，其中含代码围栏示例\n"
            "- When: 执行 req-guard ac check 校验清单内容\n"
            "- Then: 退出码为 0，SectionNotFound 问题数为 0\n"
            "<!-- /GATE:AC -->\n\n- 用例：见上。\n\n## 审核记录\n",
            encoding="utf-8",
        )
        y = work / ".gates" / "req-guard.yaml"
        if y.exists():
            y.write_text(
                y.read_text(encoding="utf-8").replace("level: 3", "level: 0"),
                encoding="utf-8",
            )
        return work

    def run(work: Path, *args: str):
        return subprocess.run(
            [str(BIN), *args, "-p", "."],
            cwd=work, capture_output=True, text=True,
            env={**os.environ, "REQ_GUARD_AI_CTX": ""}, **RUN_KW,
        )

    # 38_围栏内标题不算边界：TOUCH 块必须仍被读到
    work = sandbox(
        "38-fenced",
        "本段正文。\n\n   ```\n   ## solution\n   ```\n",
    )
    r = run(work, "touch-check")
    good = r.returncode == 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  38_围栏内标题不算边界: exit={r.returncode} (期望 0)")
    if not good:
        print(f"      ↳ 38: {(r.stdout + r.stderr)[:300]}")

    # 39_变长围栏（四反引号包三反引号）
    work = sandbox(
        "39-longfence",
        "本段正文。\n\n   ````\n   ```\n   ## fake\n   ```\n   ````\n",
    )
    r = run(work, "touch-check")
    good = r.returncode == 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  39_变长围栏内标题不算边界: exit={r.returncode} (期望 0)")
    if not good:
        print(f"      ↳ {(r.stdout + r.stderr)[:250]}")

    # 40_围栏外标题仍算边界：TOUCH 块若被误判到别段，ac check 会报 SectionNotFound
    work = sandbox("40-outside", "本段正文。\n")
    r = run(work, "ac", "check", "REQ-001")
    good = r.returncode == 0 and "SectionNotFound" not in (r.stdout + r.stderr)
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  40_围栏外标题仍算边界: exit={r.returncode} 且无 SectionNotFound")
    if not good:
        print(f"      ↳ {(r.stdout + r.stderr)[:300]}")
    if not good:
        print(f"      ↳ {(r.stdout + r.stderr)[:250]}")
    return ok


def verify_install_exempt() -> bool:
    """36–37 REQ-005：install 生成物豁免，且**精确到文件**。

    36/37 是配对的一格：生成物放行，同目录下的**非**生成物仍须声明。
    只测前者会漏掉"图省事给了 `.claude/**`"这个退化 —— 那会把工具自己的
    CLAUDE.md 一起放行，正是本需求明令禁止的绕道。
    """
    if not BIN.exists():
        print("SKIP  36-37_install生成物豁免: 未构建 req-guard 二进制")
        return True

    ok = True

    def sandbox(tag: str) -> Path:
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{tag}"))
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        r = subprocess.run(
            [str(BIN), "init", "-p", ".", "--tool", "claude,codebuddy,codex,cursor"],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        (work / HOOK_REL).parent.mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        (work / REQ_DIR).mkdir(parents=True, exist_ok=True)
        # 夹具必须是**完整**清单：`make_req` 只造 GATE 头，没有 `## N.` 章节，
        # 也没有 GATE:TOUCH 声明 —— 那样 touch-check 会因 SectionNotFound /
        # EmptyDeclaration 恒拦，36/37 就都变成"因为错误的原因通过/失败"。
        # 那是本项目最坏的失效模式（看着在拦，其实没在判你写的那份）。
        (work / REQ_DIR / "REQ-001.md").write_text(
            "# REQ-001 豁免场景\n\n"
            "<!-- GATE:HEAD id=REQ-001 status=approved created=2026-01-01 -->\n"
            + "".join(
                f"<!-- GATE:STEP name={n} label={l} status=approved reviewer=t updated=- -->\n"
                for n, l in (
                    ("decomposition", "需求分解"),
                    ("solution", "技术方案"),
                    ("testplan", "测试计划"),
                )
            )
            + "\n## 1. 需求分解\n\n- 背景：install 生成物豁免演示。\n\n"
            "## 2. 技术方案\n\n<!-- GATE:TOUCH -->\n"
            "core/**\n"
            "<!-- /GATE:TOUCH -->\n"
            "\n- 思路：声明范围后核对实际改动。\n\n"
            "## 3. 测试计划\n\n<!-- GATE:AC -->\n### AC-001\n"
            "- Given: 一份三段均已批准的技术方案段，其中含代码围栏示例\n"
            "- When: 执行 req-guard ac check 校验清单内容\n"
            "- Then: 退出码为 0，SectionNotFound 问题数为 0\n"
            "<!-- /GATE:AC -->\n\n- 用例：见上。\n\n## 审核记录\n",
            encoding="utf-8",
        )
        y = work / ".gates" / "req-guard.yaml"
        if y.exists():
            y.write_text(
                y.read_text(encoding="utf-8").replace("level: 3", "level: 0"),
                encoding="utf-8",
            )
        del r
        return work

    def touch_rc(work: Path, rel: str):
        dst = work / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text("{}\n", encoding="utf-8")
        subprocess.run(["git", "add", rel], cwd=work, capture_output=True, text=True)
        r = subprocess.run(
            [str(BIN), "touch-check", "-p", "."],
            cwd=work, capture_output=True, text=True,
            env={**os.environ, "REQ_GUARD_AI_CTX": ""}, **RUN_KW,
        )
        return r.returncode, r.stdout + r.stderr

    # 36_install生成物豁免：**两层**都要成立
    #   第一层：`.gitignore` 挡住（主）—— 装过 install 的仓不该再看到这些文件；
    #   第二层：强行 `git add -f` 混进索引后 `touch-check` 仍放行（纵深防御）——
    #           否则「.gitignore 被改掉 / 被人手工取消忽略」就会让它们重新变成
    #           NotDeclared，又是每份清单都会冒出来的拦截。
    work = sandbox("36-exempt")
    gi = (work / ".gitignore").read_text(encoding="utf-8")
    need = [
        "/.claude/settings.json",
        "/.codebuddy/settings.json",
        "/.codex/hooks.json",
        "/.cursor/hooks.json",
    ]
    missing_gi = [n for n in need if n not in gi]
    r = subprocess.run(["git", "add", ".claude/settings.json"], cwd=work,
                       capture_output=True, text=True, **RUN_KW)
    blocked_by_gitignore = r.returncode != 0
    r2 = subprocess.run(["git", "add", "-f", ".claude/settings.json"], cwd=work,
                        capture_output=True, text=True, **RUN_KW)
    rc = 99
    if r2.returncode == 0:
        rc = subprocess.run(
            [str(BIN), "touch-check", "-p", "."],
            cwd=work, capture_output=True, text=True,
            env={**os.environ, "REQ_GUARD_AI_CTX": ""}, **RUN_KW,
        ).returncode
    good36 = not missing_gi and blocked_by_gitignore and rc == 0
    ok = ok and good36
    print(
        f"{'PASS' if good36 else 'FAIL'}  36_install生成物豁免: "
        f"gitignore缺失={len(missing_gi)} git已挡={blocked_by_gitignore} 强入后touch-check={rc} "
        f"(期望 0 / True / 0)"
    )
    if missing_gi:
        print(f"      ↳ 缺: {missing_gi}")
    if not good36:
        rr = subprocess.run([str(BIN), "touch-check", "-p", "."], cwd=work,
                            capture_output=True, text=True,
                            env={**os.environ, "REQ_GUARD_AI_CTX": ""}, **RUN_KW)
        print(f"      ↳ {(rr.stdout + rr.stderr)[:400]}")

    # 37_同目录非生成物仍须声明（精确到文件，不给目录通配）
    work = sandbox("37-not-exempt")
    rc, out37 = touch_rc(work, ".claude/CLAUDE.md")
    # 必须是 `NotDeclared` 这条规则命中，而**不是**夹具缺声明导致的某种拦截
    good37 = rc == 1 and "NotDeclared" in out37 and "CLAUDE.md" in out37
    ok = ok and good37
    print(f"{'PASS' if good37 else 'FAIL'}  37_同目录非生成物仍须声明: exit={rc} (期望 1)")
    return ok


def verify_amend_gate() -> bool:
    """32–34 REQ-004 场景：修订路径、seal 一次性、设计文档交叉引用。

    这三格都必须用**子进程跑二进制**（不是脚本）：`amend` / `seal` 是审批类动作，
    沙箱里把 auth.level 降到 L0 才放行 —— 这也顺带证明它们走的是审批守卫。
    """
    if not BIN.exists():
        print("SKIP  32-34_amend与seal: 未构建 req-guard 二进制")
        return True

    ok = True

    def sandbox(tag: str, files: dict) -> Path:
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{tag}"))
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        subprocess.run(
            [str(BIN), "init", "-p", ".", "--tool", "none"],
            cwd=work, capture_output=True, text=True, **RUN_KW,
        )
        (work / REQ_DIR).mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).parent.mkdir(parents=True, exist_ok=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        for rel, body in files.items():
            dst = work / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_text(body, encoding="utf-8")
        y = work / ".gates" / "req-guard.yaml"
        if y.exists():
            y.write_text(
                y.read_text(encoding="utf-8").replace("level: 3", "level: 0"),
                encoding="utf-8",
            )
        return work

    def call(work: Path, *args: str):
        return subprocess.run(
            [str(BIN), *args, "-p", "."],
            cwd=work, capture_output=True, text=True,
            env={**os.environ, "REQ_GUARD_AI_CTX": ""}, **RUN_KW,
        )

    base = (
        "# REQ-001 修订路径\n\n"
        "<!-- GATE:HEAD id=REQ-001 status=approved created=2026-01-01 -->\n"
        "<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=t updated=- "
        + SUM_FIELD + " -->\n"
        "<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=t updated=- "
        + SUM_FIELD + " -->\n"
        "<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=t updated=- "
        + SUM_FIELD + " -->\n\n"
        "## 1. 需求分解\n\n- 背景：修订路径演示。\n\n"
        "## 2. 技术方案\n\n<!-- GATE:TOUCH -->\nsrc/**\n<!-- /GATE:TOUCH -->\n"
        "\n- 思路：原始内容。\n\n"
        "## 3. 测试计划\n\n<!-- GATE:AC -->\n### AC-001\n"
        "- Given: 已批准\n- When: 执行 check\n- Then: 退出码 0\n<!-- /GATE:AC -->\n"
        "\n- 用例：见上。\n\n## 审核记录\n"
    )

    # 32_amend 打回后必须重新批准才放行
    work = sandbox("32-amend", {f"{REQ_DIR}/REQ-001.md": base})
    r = call(work, "amend", "REQ-001", "--step", "solution", "--comment", "补异常分支")
    good = r.returncode == 0
    doc = (work / REQ_DIR / "REQ-001.md").read_text(encoding="utf-8")
    good = good and "status=amended" in doc and "sum=-" in doc
    ledger = (work / ".gates" / "audit" / "ledger.md")
    lt = ledger.read_text(encoding="utf-8") if ledger.exists() else ""
    good = good and "AMEND REQ-001" in lt and "REJECT REQ-001" not in lt
    # 未重审 → check 必须拦
    chk = call(work, "check")
    good = good and chk.returncode == 1
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  32_amend打回须重审: amend={r.returncode} check={chk.returncode} (期望 0 / 1)")
    if not good:
        print(f"      ↳ amend: {(r.stdout + r.stderr)[:300]}")
        print(f"      ↳ doc amended={chr(39)}{'status=amended' in doc}{chr(39)} sum-={chr(39)}{'sum=-' in doc}{chr(39)}")
        print(f"      ↳ ledger: {lt[:200]}")

    # 33_seal 一次性：已绑定须 --reason，且记 RESEAL
    work = sandbox("33-seal", {f"{REQ_DIR}/REQ-001.md": base})
    r1 = call(work, "seal", "REQ-001")
    good = r1.returncode == 1 and "--reason" in (r1.stdout + r1.stderr)
    r2 = call(work, "seal", "REQ-001", "--reason", "格式同步")
    lt = (work / ".gates" / "audit" / "ledger.md").read_text(encoding="utf-8")
    good = good and r2.returncode == 0 and "RESEAL REQ-001" in lt
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  33_seal一次性: 无reason={r1.returncode} 带reason={r2.returncode} (期望 1 / 0)")
    if not good:
        print(f"      ↳ {(r1.stdout + r1.stderr)[:200]}")

    # 34_设计文档交叉引用失效 → ac check 拦
    for tag, doc_rel, want in [
        ("34a-缺文件", None, 1),
        ("34b-缺小节", "docs/设计/A.md", 1),
    ]:
        files = {f"{REQ_DIR}/REQ-001.md": base}
        if doc_rel:
            files[doc_rel] = "# 设计\n\n## 1.1 现存小节\n\n正文\n"
        sec = "docs/设计/缺失.md §2.2" if doc_rel is None else "docs/设计/A.md §9.9"
        files[f"{REQ_DIR}/REQ-001.md"] = base.replace("- 思路：原始内容。", f"- 思路：见 {sec}。")
        work = sandbox(tag, files)
        r = call(work, "ac", "check", "REQ-001")
        out = r.stdout + r.stderr
        good = r.returncode == want and "BrokenCrossRef" in out
        ok = ok and good
        print(f"{'PASS' if good else 'FAIL'}  {tag}引用失效: exit={r.returncode} (期望 {want})")
        if not good:
            print(f"      ↳ {out[:200]}")
    return ok


def verify_touch_gate() -> bool:
    """19–25 变更范围契约场景（设计文档 §6.4）。

    与前 18 个场景不同：判定在 **core**（`req-guard touch-check`），
    脚本只负责取 staged 文件集并透传退出码。所以这些场景测的是
    「脚本接线是否正确」+「core 判定在真实 git 索引下是否成立」。
    判定逻辑本身的细粒度覆盖在 `cargo test -p req-guard-core touch` 里。
    """
    if not BIN.exists():
        print("SKIP  19-25_变更范围契约: 未构建 req-guard 二进制（cargo build）")
        return True

    ok = True
    for name, staged, declared, expect in [
        ("19_staged含未声明文件", ["docs/b.md"], ["src/**"], 1),
        ("20_staged全为已声明文件", ["src/a.rs"], ["src/**"], 0),
        ("21_staged含exempt路径", [".gates/requirements/REQ-001.md"], ["src/**"], 0),
        ("22_staged为空", [], ["src/**"], 0),
        ("23_无TOUCH块视为未声明", ["src/a.rs"], [], 1),
    ]:
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{name}"))
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        subprocess.run(["git", "config", "user.email", "t@t"], cwd=work, check=True)
        subprocess.run(["git", "config", "user.name", "t"], cwd=work, check=True)
        (work / REQ_DIR).mkdir(parents=True)
        (work / TOUCH_REL).parent.mkdir(parents=True, exist_ok=True)
        (work / TOUCH_REL).write_text(TOUCH_HOOK, encoding="utf-8")
        touch_block = (
            f"<!-- GATE:TOUCH -->\n"
            + "".join(f"{d}\n" for d in declared)
            + "<!-- /GATE:TOUCH -->\n"
            if declared
            else ""
        )
        # 先造 staged 文件，再写需求文档：场景 21 会把
        # `.gates/requirements/REQ-001.md` 本身作为"已暂存文件"造出来，
        # 顺序反了就会用 `"x\n"` **把被测的需求文档覆写掉**，
        # 于是判定看到的是一份没有 TOUCH 块的残缺文档 —— 测的就不是声明豁免了。
        for f in staged:
            p = work / f
            p.parent.mkdir(parents=True, exist_ok=True)
            if p.name == "REQ-001.md":
                continue  # 需求文档由下面写入，别用占位内容覆盖
            p.write_text("x\n", encoding="utf-8")

        # 注意：**不能**靠 str.replace 往 make_req() 的产物里插 `## 2. 技术方案` ——
        # make_req() 根本没有二级标题，replace 静默什么都不做，于是每个场景都因为
        # "缺 TOUCH 块"而 exit 1，期望 1 的场景变成假通过。这里显式拼完整三段。
        (work / REQ_DIR / "REQ-001.md").write_text(
            make_req("approved")
            + f"\n## 1. 需求分解\n\n- 背景\n\n## 2. 技术方案\n\n{touch_block}\n"
            + "## 3. 测试计划\n\n<!-- GATE:AC -->\n### AC-001\n"
            + "- Given: 清单已批准\n- When: 执行 check\n- Then: 退出码 0\n"
            + "<!-- /GATE:AC -->\n\n## 审核记录\n\n<!-- GATE:AUDIT -->\n<!-- /GATE:AUDIT -->\n",
            encoding="utf-8",
        )
        if staged:
            subprocess.run(["git", "add", "-A"], cwd=work, check=True)
        # touch 脚本是 fail-closed 的：req-guard 不在 PATH 就直接拦。
        # 必须把 BIN_DIR 塞进 PATH，否则**每个场景都会 exit 1**，
        # 期望 1 的场景就成了假通过（测的是"二进制缺失"而不是"越界"）。
        env = dict(os.environ)
        env["PATH"] = str(BIN_DIR) + os.pathsep + env.get("PATH", "")
        r = subprocess.run(
            [SH, str(work / TOUCH_REL)], cwd=work, capture_output=True, text=True,
            stdin=subprocess.DEVNULL, env=env, **RUN_KW,
        )
        good = r.returncode == expect
        if not good and expect == 0:
            print(f"      ↳ stderr: {r.stderr.strip()[:300]}")
        ok = ok and good
        print(f"{'PASS' if good else 'FAIL'}  {name}: exit={r.returncode} (期望 {expect})")

    # 24_PRE_COMMIT_BLOCK 含 touch 段（正则抽取断言，防"实现了但没接线"）
    text = SRC.read_text(encoding="utf-8")
    m = re.search(r"const PRE_COMMIT_BLOCK: &str = r#\"(.*?)\"#;", text, re.S)
    if not m:
        print("FAIL  24_pre_commit含touch段: 未能抽取 PRE_COMMIT_BLOCK")
        ok = False
    elif "req-guard-touch-check.sh" not in m.group(1):
        print("FAIL  24_pre_commit含touch段: PRE_COMMIT_BLOCK 里没有 touch 调用")
        ok = False
    else:
        # 25_整块与增量段逐字一致（core 单测同款断言，这里从 Python 侧再锁一次）
        m2 = re.search(r'const PRE_COMMIT_TOUCH_BLOCK: &str = r#"(.*?)"#;', text, re.S)
        if not m2 or not m.group(1).endswith(m2.group(1)):
            print("FAIL  25_整块与增量段一致: PRE_COMMIT_BLOCK 未以 PRE_COMMIT_TOUCH_BLOCK 结尾")
            ok = False
        else:
            print("PASS  24_pre_commit含touch段")
            print("PASS  25_整块与增量段一致")

    # 26_touch 段脚本缺失即拦截（fail-closed，与主门禁同款）
    work = Path(tempfile.mkdtemp(prefix="reqguard-26"))
    (work / ".git" / "hooks").mkdir(parents=True)
    pc = work / ".git" / "hooks" / "pre-commit"
    m = re.search(r"const PRE_COMMIT_BLOCK: &str = r#\"(.*?)\"#;", text, re.S)
    (work / REQ_DIR).mkdir(parents=True)
    (work / HOOK_REL).parent.mkdir(parents=True, exist_ok=True)
    (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
    (work / REQ_DIR / "REQ-001.md").write_text(make_req("approved"), encoding="utf-8")
    pc.write_text("#!/bin/sh\n" + m.group(1), encoding="utf-8")
    # 刻意不创建 TOUCH_REL → 第二段必须拦
    env = dict(os.environ)
    env["PATH"] = str(BIN_DIR) + os.pathsep + env.get("PATH", "")
    r = subprocess.run(
        [SH, str(pc)], cwd=work, capture_output=True, text=True,
        stdin=subprocess.DEVNULL, env=env, **RUN_KW,
    )
    good = r.returncode == 1
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  26_touch脚本缺失即拦截: exit={r.returncode} (期望 1)")
    return ok


ok = True
for name, req, cmts, bp, stdin_data, expect, extra in CASES:
    rc = run(name, req, cmts, bp, stdin_data, extra)
    good = rc == expect
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  {name}: exit={rc} (期望 {expect})")


def verify_draft_contract() -> bool:
    """REQ-010 P0/P1：草稿通道三条约束 + 三处文案（AC-012 / AC-013）。

    为什么这些必须进本脚本（而不是只靠 Rust 单测）：单测里注释掉校验，
    `cargo test` 当然会红 —— 但那是**编译不过**，不是门禁漂移。
    AC-013 要的是"校验被注掉后本脚本退出码非 0"：即文案与校验的对应关系
    要能被机械检出，否则 `--help`/README 里写着三条约束、实际只拦两条，
    用户没有任何途径察觉。
    """
    ok = True
    prose = "第一行。\n第二行。\n第三行。\n"
    draft_dir = ".gates/drafts"

    def make_frozen_req() -> str:
        """造一份「三段已批 + 有 TOUCH 声明 + 各段有散文」的清单。

        这里**不能**用 `make_req`：那份三段全空、且无 `GATE:TOUCH`。后果有二 ——
        apply 会先被"没有有效变更范围声明"拦下，于是 D1 的 exit=1 来自**另一条**校验，
        断言就成了自证（把多段校验注掉，D1 照样 PASS）。有散文才能让"草稿比原段短"
        这条真正成为唯一拦下它的原因。
        """
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
                f"<!-- GATE:STEP name={name} label={label} status=approved "
                "reviewer=t updated=- -->"
            )
            lines.append("")
        lines.append("## 1. 需求分解")
        lines += ["", "- 背景：原始内容。", ""]
        lines.append("## 2. 技术方案")
        lines += ["", "<!-- GATE:TOUCH -->", "src/**", "<!-- /GATE:TOUCH -->", ""]
        lines += [prose, ""]
        lines.append("## 3. 测试计划")
        lines += ["", prose, ""]
        return "\n".join(lines) + "\n"

    req = make_frozen_req()
    # 草稿散文**刻意与清单当前散文不同**但行数相同：
    # 若内容相同，apply 会先被"内容完全相同，不做任何事"拦下 —— 那 D1 的
    # exit=1 又变成自证（换任何校验都会红）。断言要能唯一归因，
    # 前提是除被测校验外**其它校验都应当通过**。
    new_prose = "改后的第一行。\n改后的第二行。\n改后的第三行。\n"

    def apply_with(draft_text: str, step: str = "solution", sid: str = "draft"):
        work = Path(tempfile.mkdtemp(prefix="reqguard-")) / sid
        (work / Path(HOOK_REL).parent).mkdir(parents=True)
        (work / REQ_DIR).mkdir(parents=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        (work / REQ_DIR / "REQ-001.md").write_text(req, encoding="utf-8")
        (work / draft_dir).mkdir(parents=True)
        (work / draft_dir / "REQ-001.draft.md").write_text(draft_text, encoding="utf-8")
        # 沙箱 L0：审批锁在 L3 下会拒绝 AI 自批（那是设计意图，不能在验证里绕开语义）
        (work / ".gates" / "req-guard.yaml").write_text("level: 0\n", encoding="utf-8")
        env = dict(os.environ)
        env["PATH"] = str(BIN_DIR) + os.pathsep + env.get("PATH", "")
        rc = subprocess.run(
            [str(BIN), "apply", "REQ-001", "--step", step,
             "--reviewer", "寇工", "--comment", "改完了"],
            cwd=work, capture_output=True, text=True, env=env, **RUN_KW,
        ).returncode
        shutil.rmtree(work.parent, ignore_errors=True)
        return rc

    # ① 多段草稿 -> 拒（退出码非 0）
    # 关键：两段各自都写满与原段等长的散文。写成 "甲。" 会让 exit=1 来自
    # "草稿比原段短"那条校验 —— 那 D1 就是自证：把多段校验注掉，它照样 PASS。
    # 断言要能唯一归因，多段草稿本身就得是**合法**的（只是段数超一条）。
    multi = "{step=solution}\n" + new_prose + "\n{step=testplan}\n" + new_prose
    rc = apply_with(multi, sid="multi")
    good = rc != 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  D1_apply拒多段草稿: exit={rc} (期望非0)")

    # ② 重复段名 -> 拒（AC-009）
    rc = apply_with("{step=solution}\n" + new_prose + "{step=solution}\n" + new_prose, sid="dup")
    good = rc != 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  D2_apply拒重复段名: exit={rc} (期望非0)")

    # ③ 草稿含 GATE 块标记 -> 拒（AC-003）
    rc = apply_with("{step=solution}\n" + new_prose + "<!-- GATE:AC -->\n", sid="gatein")
    good = rc != 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  D3_apply拒草稿内GATE块: exit={rc} (期望非0)")

    # ④ 草稿短于原段 -> 拒（AC-005 / AC-011）
    rc = apply_with("{step=solution}\n只有一行。\n", sid="short")
    good = rc != 0
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  D4_apply拒短草稿: exit={rc} (期望非0)")

    # ⑤ 三处文案同时含关键句（AC-012 / U-07）
    # `--help` 走 stderr 且退出码为 2（clap 惯例），故 stdout+stderr 都要看 ——
    # 只读 stdout 会把"文案其实存在"误判成缺失。
    h = subprocess.run([str(BIN), "--help"], capture_output=True, text=True, **RUN_KW)
    help_txt = h.stdout + h.stderr
    readme = (ROOT / ".gates" / "README.md").read_text(encoding="utf-8")
    # 草稿文件头 = `DRAFT_CONTRACT`，由 write_draft 写在**第一行**。
    # 这里从源码里读常量本身，而不是再抄一份 —— 抄的那份必然漂移，
    # 而漂移正是 AC-013 要机械检出的东西。
    req_src = (ROOT / "core" / "src" / "requirement.rs").read_text(encoding="utf-8")
    m = re.search(r'pub const DRAFT_CONTRACT: &str = "\\\n(.*?)";', req_src, re.S)
    draft_head = m.group(1) if m else ""
    missing = [f"{where_}:{kw}" for where_, text in
               (("--help", help_txt), (".gates/README.md", readme), ("草稿文件头", draft_head))
               for kw in ("草稿", "散文", "一次") if kw not in text]
    good = not missing
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  D5_三处文案含草稿契约: "
          + ("齐全" if good else f"缺 {missing}"))
    return ok


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
ok = ok and verify_touch_gate()
ok = ok and verify_content_freeze()
ok = ok and verify_amend_gate()
ok = ok and verify_install_exempt()
ok = ok and verify_fenced_boundary()
ok = ok and verify_draft_contract()
ok = ok and verify_section_gate()


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
        # 与 `run()` 同款两件套：git init（`check --staged` 要 git diff --cached）
        # + PATH 里有 req-guard（判定在 core，缺二进制即 fail-closed）。
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        denv = dict(os.environ)
        denv["PATH"] = str(BIN_DIR) + os.pathsep + denv.get("PATH", "")
        r = subprocess.run(
            [SH, str(work / deny_rel)], cwd=work, capture_output=True, text=True,
            stdin=subprocess.DEVNULL, env=denv, **RUN_KW,
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

# ============================================================================
# REQ-006 P2 场景组：多需求并行的门禁裁决（编号实施时取verify_gate.py 最大号 +1）
# ----------------------------------------------------------------------------
# 既有 1–18 与 19–40 号场景**一个都没改期望值**，它们现在跑的是「脚本 → check --staged
# → core」这条真实链路（`run()` 里的 git init + PATH 两件套就是为此）。
# 下面这组只测**新增**行为：多需求裁决、hint 消歧、委托接线、fail-closed。
# ============================================================================


def make_req_with_touch(req_id: str, status: str, declares) -> str:
    """造一份带 `GATE:TOUCH` 声明的清单（反查索引的数据源）。"""
    block = (
        "<!-- GATE:TOUCH -->\n" + "".join(f"{d}\n" for d in declares) + "<!-- /GATE:TOUCH -->\n"
        if declares
        else ""
    )
    return make_req(status).replace("REQ-001", req_id) + (
        f"\n## 1. 需求分解\n\n- 背景：本场景夹具。\n\n## 2. 技术方案\n\n- 思路：夹具。\n\n"
        + block
        + "\n## 3. 测试计划\n\n- 计划：夹具。\n"
    )


def verify_multi_gate() -> bool:
    """REQ-006 新增场景组。判定全在 core，本组测的是「脚本接线 + 多需求语义」。"""
    if not BIN.exists():
        print(f"SKIP  REQ-006_多需求裁决: 未构建 {BIN}")
        return True

    ok = True
    doc = ".gates/requirements/REQ-001.md"

    def one(name, files, payload_path=None, expect=1, hint=None):
        """files: {相对路径: 内容}；payload_path: 用 payload 形态喂给脚本。"""
        nonlocal ok
        work = Path(tempfile.mkdtemp(prefix=f"reqguard-{name}-")) / name
        (work / Path(HOOK_REL).parent).mkdir(parents=True, exist_ok=True)
        (work / REQ_DIR).mkdir(parents=True)
        (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
        for rel, content in files.items():
            dst = work / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_text(content, encoding="utf-8")
        subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
        env = dict(os.environ)
        env["PATH"] = str(BIN_DIR) + os.pathsep + env.get("PATH", "")
        if hint:
            env["HOOK_REQ"] = hint
        if payload_path:
            stdin_data = json.dumps(
                {"tool_input": {"file_path": payload_path}}, ensure_ascii=False
            )
            r = subprocess.run(
                [SH, str(work / HOOK_REL)], cwd=work, capture_output=True, text=True,
                input=stdin_data, env=env, **RUN_KW,
            )
        else:
            r = subprocess.run(
                [str(BIN), "check"], cwd=work, capture_output=True, text=True,
                stdin=subprocess.DEVNULL, env=env, **RUN_KW,
            )
        shutil.rmtree(work.parent, ignore_errors=True)
        good = r.returncode == expect
        ok = ok and good
        print(f"{'PASS' if good else 'FAIL'}  {name}: exit={r.returncode} (期望 {expect})")
        return r

    a_ok = make_req_with_touch("REQ-001", "approved", ["core/src/**"])
    a_pending = make_req_with_touch("REQ-001", "pending", ["core/src/**"])
    b_ok = make_req_with_touch("REQ-002", "approved", ["cli/src/**"])

    # ① 互锁回归（G2）：已批 A + 未批 B，写 A 范围内 → 放行。
    #    改造前会被逆序第一个（B 或 A）误锁，与"正在做哪份"无关。
    one("R6_g2_已批范围内不因无关未批清单被拦",
        {doc: a_ok, ".gates/requirements/REQ-002.md": a_pending.replace("REQ-001", "REQ-002")},
        payload_path="core/src/a.rs", expect=0)

    # ② 漏拦回归（G1）：未批 A + 已批 B，写 A 声明范围内 → 拦。
    #    改造前抽中已批的 B 就放行，AI 在零审批清单上写代码而门禁显示绿灯。
    one("R6_g1_未批清单范围内必须拦",
        {doc: a_pending, ".gates/requirements/REQ-002.md": b_ok},
        payload_path="core/src/a.rs", expect=1)

    # ③ 歧义分支：多份 live 且无任何已批清单声明该路径 → 拦，且文案含三张牌。
    r = one("R6_g3_无从归因报歧义并给出路",
            {doc: a_ok, ".gates/requirements/REQ-002.md": b_ok},
            payload_path="docs/x.md", expect=1)
    for kw in ("touch --declare", "--req", "req-guard done"):
        if kw not in (r.stderr + r.stdout):
            print(f"      ↳ 歧义文案缺出路「{kw}」")
            ok = False

    # ④ hint 消歧：同一歧义场景，HOOK_REQ 指定那份 → 放行。
    one("R6_g4_HOOK_REQ消歧放行",
        {doc: a_ok, ".gates/requirements/REQ-002.md": b_ok},
        payload_path="docs/x.md", expect=0, hint="REQ-001")

    # ⑤ hint 不相交：指定的清单与本次改动无关 → 拦（hint 不是绕过口）。
    one("R6_g5_HOOK_REQ不相交则拦",
        {doc: a_ok, ".gates/requirements/REQ-002.md": b_ok},
        payload_path="core/src/a.rs", expect=1, hint="REQ-002")

    # ⑥ 委托接线（反回归）：脚本必须委��� core，且不再自带任何裁决。
    for needle, should in (("req-guard check --stdin", True),
                           ("req-guard check --staged", True),
                           ("sort -r", False), ("GATE:STEP", False),
                           ("verify-content", False), ("expires_epoch=", False)):
        present = needle in HOOK
        good = present == should
        ok = ok and good
        print(f"{'PASS' if good else 'FAIL'}  R6_g6_委托接线[{needle}]: "
              f"{'存在' if present else '不存在'} (期望{'存在' if should else '不存在'})")

    # ⑦ fail-closed：PATH 里没有 req-guard → 拦，且给出可操作指引。
    work = Path(tempfile.mkdtemp(prefix="reqguard-R6_g7-")) / "g7"
    (work / Path(HOOK_REL).parent).mkdir(parents=True)
    (work / REQ_DIR).mkdir(parents=True)
    (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
    (work / REQ_DIR / "REQ-001.md").write_text(a_ok, encoding="utf-8")
    subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
    bare = {"PATH": "/nonexistent-for-req-guard"}
    r = subprocess.run([SH, str(work / HOOK_REL)], cwd=work, capture_output=True, text=True,
                       stdin=subprocess.DEVNULL, env=bare, **RUN_KW)
    shutil.rmtree(work.parent, ignore_errors=True)
    good = r.returncode == 1 and "PATH" in (r.stdout + r.stderr)
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  R6_g7_缺二进制fail_closed: exit={r.returncode} (期望 1 且提示 PATH)")

    # ⑧ pre-commit 链路：脚本走 --staged，验证"取到的路径集正确"而不只是"core 判得对"。
    #    必须放**两份** live：单需求仓库里未声明的路径按 R7 放行（G4 兼容性 ——
    #    改造前门禁也从不看路径），那样这条场景就测不出取参是否正确了。
    work = Path(tempfile.mkdtemp(prefix="reqguard-R6_g8-")) / "g8"
    (work / Path(HOOK_REL).parent).mkdir(parents=True)
    (work / REQ_DIR).mkdir(parents=True)
    (work / HOOK_REL).write_text(HOOK, encoding="utf-8")
    (work / REQ_DIR / "REQ-001.md").write_text(a_ok, encoding="utf-8")
    (work / REQ_DIR / "REQ-002.md").write_text(b_ok, encoding="utf-8")
    (work / "core" / "src").mkdir(parents=True)
    (work / "core" / "src" / "a.rs").write_text("x\n")
    (work / "docs").mkdir()
    (work / "docs" / "x.md").write_text("x\n")
    subprocess.run(["git", "init", "-q", "."], cwd=work, check=True)
    env = dict(os.environ)
    env["PATH"] = str(BIN_DIR) + os.pathsep + env.get("PATH", "")
    subprocess.run(["git", "add", "-A"], cwd=work, check=True)
    rc_all = subprocess.run([SH, str(work / HOOK_REL)], cwd=work, capture_output=True,
                            text=True, stdin=subprocess.DEVNULL, env=env, **RUN_KW).returncode
    subprocess.run(["git", "rm", "-q", "--cached", "-r", "core"], cwd=work, check=True)
    rc_undeclared = subprocess.run([SH, str(work / HOOK_REL)], cwd=work, capture_output=True,
                                   text=True, stdin=subprocess.DEVNULL, env=env, **RUN_KW).returncode
    shutil.rmtree(work.parent, ignore_errors=True)
    good = rc_undeclared == 1
    ok = ok and good
    print(f"{'PASS' if good else 'FAIL'}  R6_g8_pre-commit取参: "
          f"仅暂存已批声明内={rc_all}(期望0) 仅暂存未声明={rc_undeclared}(期望1)")

    return ok


ok = ok and verify_multi_gate()

print("\n结论:", "全部通过" if ok else "存在失败")
sys.exit(0 if ok else 1)

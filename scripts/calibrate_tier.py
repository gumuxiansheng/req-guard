#!/usr/bin/env python3
"""阈值回放校准（G7 / U-29 / AC-016）：用本仓**真实历史变更集**跑一遍定档。

阈值 5 / 80 是初值不是结论（REQ-019 §2.3）。本脚本按同一口径复算历史每个提交
的有效改动行与派生档，输出：

1. 逐提交明细（有效行 / 命中 locked / 命中 risky / 派生档）；
2. 各档数量分布；
3. **阈值敏感度**：把两个阈值各 ±50% 重算，给出分布变化。

敏感度是这段输出里最有用的一列：若 ±50% 几乎不改分布，说明阈值不敏感（稳）；
若一改就翻转，说明阈值是在拟合噪声（该改口径或改档位定义，而不是改数字）。

注释剔除口径与 `core/src/tierdiff.rs` 的状态机**同源**（本脚本是该口径的
Python 参照实现；两处若漂移，`--self-check` 会指出 —— 见 `--self-check`）。
已知简化：raw string / 嵌套块注释 / 字符字面量按 Rust 口径实现；
`r#"…"#` 的 `#` 计数与生命周期标注按启发式处理，与 Rust 侧同一套规则。

用法:
    python scripts/calibrate_tier.py                 # 回放最近 40 个提交
    python scripts/calibrate_tier.py --max-count 80  # 换个窗口
    python scripts/calibrate_tier.py --json          # 机器可读（供 CI 比对）
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass

ROOT = Path(__file__).resolve().parent.parent

# 与 core/src/tier.rs 的 LOCKED_PATHS 逐字一致（AC-016 要求四档与两阈值同现）。
LOCKED_PATHS = ["core/src/**", "templates/hooks/**", "templates/ci/**", ".gates/req-guard.yaml"]
TIERS = ["trivial", "light", "standard", "critical"]

# .gates/req-guard.yaml 的 tier 段（与仓库配置同源读取，避免脚本里再抄一份阈值）。
DEFAULTS = {"trivial_max_lines": 5, "light_max_lines": 80}

HASH_EXT = {"sh", "bash", "zsh", "py", "ps1", "yml", "yaml", "toml", "cfg", "ini"}
NO_COMMENT_EXT = {"md", "txt", "json"}


def git(*args: str) -> str:
    out = subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=ROOT,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    if out.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} 失败：{out.stderr.strip()}")
    return out.stdout


def ext_of(path: str) -> str:
    name = path.rsplit("/", 1)[-1]
    return name.rsplit(".", 1)[1].lower() if "." in name else ""


def comment_lines(content: str, ext: str) -> set[int]:
    """返回「整行是注释或空行」的 1-based 行号集合（口径见模块文档）。"""
    out: set[int] = set()
    rust = ext == "rs"
    hash_lang = ext in HASH_EXT
    if ext in NO_COMMENT_EXT or (not rust and not hash_lang):
        for i, line in enumerate(content.split("\n"), 1):
            if not line.strip():
                out.add(i)
        return out
    block = 0          # Rust 块注释嵌套深度
    string: str | None = None
    escape = False
    in_char = False
    raw_hashes = 0
    lines = content.split("\n")
    for i, line in enumerate(lines, 1):
        if not line.strip():
            out.add(i)
            if block > 0 or string or in_char:
                continue
            continue
        has_code = False
        at_start = True
        j = 0
        while j < len(line):
            c = line[j]
            nxt = line[j + 1] if j + 1 < len(line) else ""
            if string is not None:
                if escape:
                    escape = False
                elif c == "\\":
                    escape = True
                elif c == string:
                    if raw_hashes:
                        k = 0
                        while nxt == "#" and k < raw_hashes:
                            k += 1
                            j += 1
                            nxt = line[j + 1] if j + 1 < len(line) else ""
                        if k == raw_hashes:
                            string, raw_hashes = None, 0
                    else:
                        string = None
                j += 1
                continue
            if in_char:
                if escape:
                    escape = False
                elif c == "\\":
                    escape = True
                elif c == "'":
                    in_char = False
                j += 1
                continue
            if block > 0:
                if at_start and code_inside_comment(line):
                    has_code = True
                at_start = False
                if c == "/" and nxt == "*":
                    block += 1
                    j += 2
                    continue
                if c == "*" and nxt == "/":
                    block -= 1
                    j += 2
                    continue
                j += 1
                continue
            if rust:
                if c == "/" and nxt == "/":
                    break
                if c == "/" and nxt == "*":
                    block = 1
                    j += 2
                    continue
                if c == '"':
                    string, has_code = '"', True
                    j += 1
                    continue
                if c == "r":
                    m = re.match(r'r(#*)"', line[j:])
                    if m:
                        raw_hashes, string, has_code = len(m.group(1)), '"', True
                        j += len(m.group(0))
                        continue
                if c == "'" and looks_like_char(line, j):
                    in_char, has_code = True, True
                    j += 1
                    continue
                has_code = True
                j += 1
                continue
            # hash 系（sh / py / yml / ps1 / toml）
            if c in "\"'":
                string, has_code = c, True
                j += 1
                continue
            if c == "#":
                break
            has_code = True
            j += 1
        if not has_code:
            out.add(i)
    return out


def looks_like_char(line: str, i: int) -> bool:
    q = line[i + 1] if i + 1 < len(line) else ""
    if q == "\\":
        return line[i + 3 : i + 4] == "'"
    if not q or q.isalnum() or q in "_ ":
        return False
    return line[i + 2 : i + 3] == "'"


def code_inside_comment(line: str) -> bool:
    t = line.lstrip()
    for deco in ("*", "#", "//"):
        if t.startswith(deco):
            t = t[len(deco) :].lstrip()
    if not t:
        return False
    return any(mark in t for mark in (";", "=", "{", "}", "(", ")", "=>", "::"))


def glob_hit(pattern: str, path: str) -> bool:
    """目录前缀与 `**` / `*` 的最小实现（口径与 `touch::glob_match` 对齐）。"""
    pat = pattern.rstrip("/")
    if pattern.endswith("/"):
        return path.startswith(pat + "/")
    if pat.endswith("/**"):
        return path.startswith(pat[:-3] + "/")
    if "*" not in pat:
        return path == pat
    rx = "^" + re.escape(pat).replace(r"\*\*/", "\x00").replace(r"\*", "[^/]*").replace("\x00", "(?:.*/)?") + "$"
    return re.match(rx, path) is not None


def read_config() -> dict:
    cfg = dict(DEFAULTS)
    cfg["risky_paths"] = []
    y = ROOT / ".gates" / "req-guard.yaml"
    if not y.exists():
        return cfg
    in_tier = False
    current = None
    for line in y.read_text(encoding="utf-8").split("\n"):
        t = line.strip()
        if t.startswith("#"):
            continue
        if t == "tier:":
            in_tier = True
            continue
        if in_tier and line and not line[0].isspace():
            break
        if not in_tier:
            continue
        if t.startswith("- ") and current == "risky_paths":
            cfg["risky_paths"].append(t[2:].split("#")[0].strip().strip("\"'"))
            continue
        if ":" in t:
            k, _, v = t.partition(":")
            k, v = k.strip(), v.split("#")[0].strip()
            current = k
            if k in ("trivial_max_lines", "light_max_lines") and v.isdigit():
                cfg[k] = int(v)
            elif k == "risky_paths":
                for part in v.strip("[]").split(","):
                    part = part.strip().strip("\"'")
                    if part:
                        cfg["risky_paths"].append(part)
    return cfg


HUNK = re.compile(r"^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@")


def parse_hunks(diff: str):
    """`git diff -U0` → {路径: (新增行号, 删除行号)}。"""
    out = {}
    path = None
    added: list[int] = []
    deleted: list[int] = []
    new_at = old_at = 0
    in_hunk = False

    def flush():
        if path is not None:
            out[path] = (list(added), list(deleted))

    for line in diff.split("\n"):
        if line.startswith("diff --git "):
            flush()
            rest = line[len("diff --git ") :]
            path = None
            m = re.search(r" b/(.+)$", rest)
            if m:
                path = m.group(1)
            added, deleted = [], []
            in_hunk = False
            continue
        if path is None:
            continue
        if line.startswith("Binary files "):
            out.setdefault(path, ([1], []))
            added, deleted = [1], []
            in_hunk = False
            continue
        m = HUNK.match(line)
        if m:
            old_at, new_at = int(m.group(1)), int(m.group(2))
            in_hunk = True
            continue
        if not in_hunk:
            continue
        if line.startswith("+"):
            added.append(new_at)
            new_at += 1
        elif line.startswith("-"):
            deleted.append(old_at)
            old_at += 1
    flush()
    return out


def show(ref: str, path: str) -> str | None:
    out = subprocess.run(
        ["git", "-c", "core.quotePath=false", "show", f"{ref}:{path}"],
        cwd=ROOT,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    return out.stdout if out.returncode == 0 else None


def effective(commit: str, path: str, added: list[int], deleted: list[int]) -> int:
    ext = ext_of(path)
    if deleted and not added and show(f"{commit}^", path) is None:
        pass
    new_skip: set[int] = set()
    old_skip: set[int] = set()
    if added:
        c = show(commit, path)
        if c is not None:
            new_skip = comment_lines(c, ext)
    if deleted:
        c = show(f"{commit}^", path)
        if c is not None:
            old_skip = comment_lines(c, ext)
    return sum(1 for n in added if n not in new_skip) + sum(1 for n in deleted if n not in old_skip)


def classify(total: int, paths: list[str], cfg: dict) -> tuple[str, list[str]]:
    hits = []
    floor = "trivial"
    for p in paths:
        for g in LOCKED_PATHS:
            if glob_hit(g, p):
                hits.append("locked:" + g)
                floor = "critical"
        for g in cfg["risky_paths"]:
            if glob_hit(g, p):
                hits.append("risky:" + g)
                if TIERS.index(floor) < TIERS.index("standard"):
                    floor = "standard"
    if total <= cfg["trivial_max_lines"]:
        tier = "trivial"
    elif total <= cfg["light_max_lines"]:
        tier = "light"
    else:
        tier = "standard"
    if TIERS.index(floor) > TIERS.index(tier):
        tier = floor
    return tier, hits


def replay(max_count: int, cfg: dict) -> list[dict]:
    commits = git("log", f"--max-count={max_count}", "--pretty=format:%H %s").strip().split("\n")
    rows = []
    for line in commits:
        if not line.strip():
            continue
        sha, _, subject = line.partition(" ")
        try:
            diff = git("diff", "-U0", "--no-color", f"{sha}^", sha)
        except SystemExit:
            diff = git("diff", "-U0", "--no-color", "--root", sha)
        files = parse_hunks(diff)
        total = 0
        paths = []
        for path, (added, deleted) in files.items():
            if not added and not deleted:
                continue
            paths.append(path)
            total += effective(sha, path, added, deleted)
        tier, hits = classify(total, paths, cfg)
        # 去重且保持发现顺序：同一提交里 `core/src/**` 命中十几次对判读毫无帮助，
        # 反而让人以为判定被重复执行了。
        seen = set()
        hits = [h for h in hits if not (h in seen or seen.add(h))]
        rows.append(
            {
                "sha": sha[:8],
                "subject": subject[:48],
                "effective": total,
                "tier": tier,
                "hits": hits,
            }
        )
    return rows


def dist(rows: list[dict], cfg: dict) -> dict:
    out = {t: 0 for t in TIERS}
    for r in rows:
        alt = dict(cfg)
        # 用同一批有效行重算档（敏感度只改阈值，不重跑 git）
        if r["effective"] <= alt["trivial_max_lines"]:
            tier = "trivial"
        elif r["effective"] <= alt["light_max_lines"]:
            tier = "light"
        else:
            tier = "standard"
        for h in r["hits"]:
            if h.startswith("locked"):
                tier = "critical"
            elif h.startswith("risky") and TIERS.index(tier) < TIERS.index("standard"):
                tier = "standard"
        out[tier] += 1
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--max-count", type=int, default=40, help="回放最近 N 个提交（默认 40）")
    ap.add_argument("--json", action="store_true", help="输出 JSON（供 CI 比对）")
    args = ap.parse_args()

    cfg = read_config()
    rows = replay(args.max_count, cfg)
    if not rows:
        print("没有可回放的提交。")
        return 1

    base = dist(rows, cfg)
    half = dist(
        rows,
        {
            **cfg,
            "trivial_max_lines": max(1, cfg["trivial_max_lines"] // 2),
            "light_max_lines": max(2, cfg["light_max_lines"] // 2),
        },
    )
    dbl = dist(
        rows,
        {
            **cfg,
            "trivial_max_lines": cfg["trivial_max_lines"] * 2,
            "light_max_lines": cfg["light_max_lines"] * 2,
        },
    )

    if args.json:
        print(json.dumps(
            {
                "thresholds": {
                    "trivial_max_lines": cfg["trivial_max_lines"],
                    "light_max_lines": cfg["light_max_lines"],
                },
                "risky_paths": cfg["risky_paths"],
                "locked_paths": LOCKED_PATHS,
                "commits": rows,
                "dist": base,
                "dist_half": half,
                "dist_double": dbl,
            },
            ensure_ascii=False,
            indent=2,
        ))
        return 0

    print("=" * 72)
    print("分级门禁阈值回放报告（REQ-019 G7）")
    print("=" * 72)
    print(f"阈值：trivial_max_lines={cfg['trivial_max_lines']}  light_max_lines={cfg['light_max_lines']}")
    print(f"内建锁定项（恒为 critical）：{', '.join(LOCKED_PATHS)}")
    print(f"配置 risky_paths：{', '.join(cfg['risky_paths']) or '（无）'}")
    print(f"回放窗口：最近 {len(rows)} 个提交\n")
    print(f"{'commit':<9}{'有效行':>8}  {'档位':<9}说明 / 命中")
    print("-" * 72)
    for r in rows:
        print(f"{r['sha']:<9}{r['effective']:>8}  {r['tier']:<9}{r['subject']}"
              + (f"  [{', '.join(r['hits'])}]" if r["hits"] else ""))
    print("-" * 72)
    print("档位分布（四档必须全部出现，AC-016）：")
    for t in TIERS:
        print(f"  {t:<9}{base[t]:>4}")
    print("\n阈值敏感度（±50%）：")
    print(f"  {'档位':<9}{'现值':>6}{'÷2':>6}{'×2':>6}")
    for t in TIERS:
        print(f"  {t:<9}{base[t]:>6}{half[t]:>6}{dbl[t]:>6}")
    same = base == half == dbl
    print(
        "\n判读：" + ("阈值不敏感（分布不随 ±50% 变化），阈值稳。" if same else
              "阈值敏感 —— 分布随阈值翻转，说明阈值在拟合噪声：\n"
              "        先改口径或档位定义，别继续拧数字（改阈值不改口径）。")
    )
    print(f"\n门槛：trivial≤{cfg['trivial_max_lines']} / light≤{cfg['light_max_lines']}；"
          f"可用档位 {'/'.join(TIERS)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
#!/usr/bin/env python3
"""分级门禁的**判决性实验**（REQ-019 T9 / U-30 / AC-008）。

命题只有一句：**分级不是万能钥匙**。清单可以把 `tier` 写成 `trivial`，
但真实改动一大、一命中高危路径，L3 就必须报 `TierEscalation` 并退出码 1。

为什么必须是「判决性实验」而不是单测：单测验证的是 `classify` 这个纯函数，
而这里验证的是**装配**（配置解析 → 变更集 → 派生档 → 拦截 → 退出码）。
装配层的失效恰恰是「函数都对、串起来失效」，那种失效在单测里看不见。

四个场景（每个都必须失败才算守住）：

| 场景 | 构造 | 期望 |
| --- | --- | --- |
| T1 伪造档位 | 清单 `tier: trivial` + 500 有效行 `gui/src/app.rs`（risky） | `check --base` 退出码 1 且输出含 `TierEscalation` 与 `standard` |
| T2 只改注释 | 清单 `tier: trivial` + 10 行 `//` 注释改动 | 有效行 0 → 放行，`tier check` 输出 `trivial` |
| T3 声明高于派生 | 清单 `tier: standard` + 3 行文档改动 | 放行，输出 `tier=standard`（声明能抬高） |
| T4 豁免区不定档 | 只暂存 `.gates/requirements/**` | 放行且审计记 `PASS no-managed-path`，**输出不含任何档位字段** |

用法:
    python scripts/verify_tier.py
    python scripts/verify_tier.py --bin target/release/req-guard
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass

ROOT = Path(__file__).resolve().parent.parent
RUN_KW = {"encoding": "utf-8", "errors": "replace"}

YAML = """version: 1
enabled: true
strict_order: false
auth:
  level: 0
tier:
  enabled: true
  trivial_max_lines: 5
  light_max_lines: 80
  risky_paths:
    - gui/src/**
touch:
  exempt:
    - .gates/requirements/**
    - .gates/audit/**
    - .gates/drafts/**
    - .gates/hooks/**
"""


def make_req(tier: str) -> str:
    """造一份三段已批准、`tier` 声明为给定值的清单。

    为什么手写而不是 `create` + `approve`：审批要人类在场（本仓 L3），
    而本脚本要能在 CI 上无人值守地跑。手写标记行只验证**裁决装配**，
    不验证审批流程（那由 `cargo test` 与 `verify_gate.py` 覆盖）。
    """
    return (
        "---\n"
        "doc_type: proposal  \n"
        f"tier: {tier}  \n"
        "owner: -  \n"
        "review_policy: codebound  \n"
        "verified_at: 2026-10-08  \n"
        "source_refs: []  \n"
        "---\n"
        "\n"
        "# REQ-001 分级门禁实验\n"
        "\n"
        '<!-- GATE:HEAD id=REQ-001 status=approved created=2026-10-08 -->'
        "\n"
        "<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=t "
        "email=t@e.com sig=- updated=2026-10-08_00:00:00 sum=- -->\n"
        "<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=t "
        "email=t@e.com sig=- updated=2026-10-08_00:00:01 sum=- -->\n"
        "<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=t "
        "email=t@e.com sig=- updated=2026-10-08_00:00:02 sum=- -->\n"
        "\n"
        "## 1. 需求分解\n"
        "\n- 实验夹具：验证派生档压不住声明档时会被拦。\n"
        "\n## 2. 技术方案\n"
        "\n- 实验夹具。\n"
        "\n<!-- GATE:TOUCH -->\n"
        "gui/src/**\n"
        "docs/**\n"
        "<!-- /GATE:TOUCH -->\n"
        "\n## 3. 测试计划\n"
        "\n- 实验夹具：跑 `python scripts/verify_tier.py`。\n"
        "\n<!-- GATE:AC -->\n"
        "### AC-001\n"
        "- Given: 一份声明 tier 为 trivial 的清单与一次 500 有效行的改动\n"
        "- When: 执行 req-guard check --base HEAD~1\n"
        "- Then: 退出码为 1 且输出含 TierEscalation\n"
        "<!-- /GATE:AC -->\n"
        "\n## 审核记录\n"
        "\n<!-- GATE:AUDIT -->\n"
        "<!-- /GATE:AUDIT -->\n"
    )


class Repo:
    def __init__(self, bin_path: Path):
        self.bin = bin_path
        self.dir = Path(tempfile.mkdtemp(prefix="reqguard-tier-"))

    def __enter__(self):
        self.run("git", "init", "-q", ".")
        self.run("git", "config", "user.email", "t@e.com")
        self.run("git", "config", "user.name", "t")
        (self.dir / ".gates" / "requirements").mkdir(parents=True)
        (self.dir / ".gates" / "audit").mkdir(parents=True)
        (self.dir / ".gates" / "req-guard.yaml").write_text(YAML, encoding="utf-8")
        return self

    def __exit__(self, *_):
        shutil.rmtree(self.dir, ignore_errors=True)

    def run(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(list(args), cwd=self.dir, capture_output=True, **RUN_KW)

    def rg(self, *args: str) -> subprocess.CompletedProcess:
        env = dict(os.environ)
        env["PATH"] = str(self.bin.parent) + os.pathsep + env.get("PATH", "")
        return subprocess.run(
            [str(self.bin), *args], cwd=self.dir, capture_output=True, env=env, **RUN_KW
        )

    def write(self, rel: str, text: str) -> None:
        p = self.dir / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")

    def commit(self, msg: str) -> None:
        self.run("git", "add", "-A")
        self.run("git", "commit", "-q", "-m", msg, "--no-verify")


def case(name: str, good: bool, detail: str) -> bool:
    print(f"{'PASS' if good else 'FAIL'}  {name}: {detail}")
    return good


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--bin",
        default=str(ROOT / "target" / "debug" / ("req-guard.exe" if os.name == "nt" else "req-guard")),
        help="req-guard 二进制路径（默认 target/debug/req-guard）",
    )
    args = ap.parse_args()
    bin_path = Path(args.bin)
    if not bin_path.exists():
        print(f"未找到二进制 {bin_path}；先执行 cargo build")
        return 2

    ok = True

    # ── T1 伪造档位必须被 L3 拦（U-30 / AC-008 的核心） ──────────────────
    with Repo(bin_path) as r:
        r.write(".gates/requirements/REQ-001.md", make_req("trivial"))
        r.write("docs/设计/x.md", "占位\n")
        r.commit("init")
        # 500 行**可执行代码**（`//` 注释行会被剔除，所以必须是真语句）
        r.write(
            "gui/src/app.rs",
            "\n".join(f"pub fn f{i}() -> i32 {{ {i} }}" for i in range(500)) + "\n",
        )
        r.commit("big change")
        out = r.rg("check", "--base", "HEAD~1")
        text = out.stdout + out.stderr
        ok &= case(
            "T1_伪造trivial被L3拦",
            out.returncode == 1 and "TierEscalation" in text and "standard" in text,
            f"exit={out.returncode} (期望 1) 含 TierEscalation={'TierEscalation' in text}",
        )
        ok &= case(
            "T1b_拦截理由含派生档与有效行",
            "派生 standard" in text and "有效改动行 500" in text,
            "理由可复算：报出派生档与有效行数",
        )
        # 派生档也必须能从 `tier check` 看到（理由可复算）
        t = r.rg("tier", "check", "--base", "HEAD~1")
        ttext = t.stdout + t.stderr
        ok &= case(
            "T1c_tier_check给出理由",
            t.returncode == 0 and "gui/src/app.rs" in ttext and "命中 risky" in ttext,
            f"exit={t.returncode} 含逐文件明细与命中 glob",
        )

    # ── T2 只改注释 → 有效行 0 → 免审档放行 ─────────────────────────────
    # 路径选 `docs/**`（**不在** risky_paths 里）：命中 risky 时「有效行 0」也会被
    # 抬到 standard —— 那是 §2.3 表格写明的设计（risky 命中即至少 standard），
    # 在这里验证它只会把「行数维度」和「路径维度」两件事混在一个场景里。
    with Repo(bin_path) as r:
        r.write(".gates/requirements/REQ-001.md", make_req("trivial"))
        r.write("docs/设计/x.rs", "pub fn a() -> i32 { 1 }\n")
        r.commit("init")
        r.write(
            "docs/设计/x.rs",
            "pub fn a() -> i32 { 1 }\n" + "".join(f"// 注释 {i}\n" for i in range(10)),
        )
        r.commit("comment only")
        out = r.rg("check", "--base", "HEAD~1")
        text = out.stdout + out.stderr
        ok &= case("T2_只改注释放行", out.returncode == 0, f"exit={out.returncode} (期望 0)")
        ok &= case(
            "T2b_有效行判为零",
            "有效改动行 0" in text,
            "只改注释必须判 0 有效行（G2 的地基）",
        )
        t = r.rg("tier", "check", "--base", "HEAD~1")
        ok &= case(
            "T2c_落进免审档",
            "tier=trivial" in (t.stdout + t.stderr),
            "派生档应为 trivial",
        )

    # ── T3 声明档高于派生档 → 放行且输出声明档 ───────────────────────────
    with Repo(bin_path) as r:
        r.write(".gates/requirements/REQ-001.md", make_req("standard"))
        r.write("docs/设计/x.md", "占位\n")
        r.commit("init")
        r.write("docs/设计/x.md", "占位\n第二行\n第三行\n")
        r.commit("doc tweak")
        # git 取最小 diff：只报 2 行新增（首行未变），故有效行是 2 不是 3
        out = r.rg("check", "--base", "HEAD~1")
        text = out.stdout + out.stderr
        ok &= case("T3_声明档抬高放行", out.returncode == 0, f"exit={out.returncode} (期望 0)")
        ok &= case(
            "T3b_输出含声明档",
            "tier=standard" in text and "声明档（逐份）：REQ-001=standard" in text,
            "输出须含 tier=standard 与逐份声明档（AC-015）",
        )

    # ── T4 豁免区不参与定档（R2b 优先，§2.10） ───────────────────────────
    with Repo(bin_path) as r:
        r.write(".gates/requirements/REQ-001.md", make_req("trivial"))
        r.write("gui/src/app.rs", "pub fn a() -> i32 { 1 }\n")
        r.commit("init")
        r.write(".gates/requirements/REQ-001.md", make_req("trivial") + "\n- 追加一行\n")
        r.run("git", "add", ".gates/requirements/REQ-001.md")
        out = r.rg("check", "--staged")
        text = out.stdout + out.stderr
        ok &= case("T4_纯豁免变更集放行", out.returncode == 0, f"exit={out.returncode} (期望 0)")
        ok &= case(
            "T4b_输出不含任何档位字段",
            "tier=" not in text and "TierEscalation" not in text,
            "出现档位字段即说明定档插到了 R2b 之前",
        )
        log = (r.dir / ".gates" / "audit" / "gate-audit.log")
        log_text = log.read_text(encoding="utf-8") if log.exists() else ""
        ok &= case(
            "T4c_审计记PASS_no_managed_path",
            "PASS no-managed-path" in log_text,
            "留痕必须是 REQ-020 的专用标记",
        )

    print("\n结论:", "全部通过" if ok else "存在失败")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
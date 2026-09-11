#!/usr/bin/env python3
"""生成 GUI 内嵌用的 Noto Sans SC 子集。

为什么需要
----------
egui 默认字体（`default_fonts`）只含拉丁字形，**不含 CJK**：
不内嵌中文字体，界面上所有中文都会显示成"豆腐块"（□），等于不可用。
而完整 Noto Sans SC 约 8–10MB，直接内嵌会让二进制无谓地变大，
因此这里下载完整字体后**子集化**到 2–3MB 再交由 `include_bytes!` 内嵌。

用法
----
    python scripts/make_font_subset.py

产出
----
`gui/assets/NotoSansSC-Regular.otf`（**入库**，被 `gui/src/fonts.rs` 内嵌）
`gui/assets/NotoSansSC-Regular.full.otf`（不入库的中间产物，可用 `--keep-full` 保留）

许可
----
Noto Sans SC 采用 **SIL Open Font License 1.1**，允许内嵌与再分发。
"""
from __future__ import annotations

import argparse
import subprocess
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "gui" / "assets"
FULL = ASSETS / "NotoSansSC-Regular.full.otf"
OUTPUT = ASSETS / "NotoSansSC-Regular.otf"

# notofonts/noto-cjk 的 SubsetOTF（简中）版本，经 jsdelivr 分发。
FONT_URL = (
    "https://cdn.jsdelivr.net/gh/notofonts/noto-cjk@main/"
    "Sans/SubsetOTF/SC/NotoSansSC-Regular.otf"
)

# 除汉字外，界面还会用到的符号区间（状态点、勾选、箭头等）。
SYMBOL_RANGES = [
    (0x0020, 0x007E),  # 基本 ASCII
    (0x00A0, 0x00FF),  # Latin-1 补充
    (0x2000, 0x206F),  # 通用标点
    (0x2190, 0x21FF),  # 箭头
    (0x2500, 0x257F),  # 制表符（边框）
    (0x25A0, 0x25FF),  # 几何图形：● ○ ■ ▶
    (0x2600, 0x26FF),  # 杂项符号：⛔
    (0x2700, 0x27BF),  # 装饰符号：✅ ✓ ✗
    (0x3000, 0x303F),  # CJK 标点：中文逗号/句号/书名号
    (0xFF00, 0xFFEF),  # 全角字符
]


def gb2312_hanzi() -> set[int]:
    """枚举 GB2312 全部汉字（含一二级字库，共 6763 字）。"""
    out: set[int] = set()
    for hi in range(0xB0, 0xF8):
        for lo in range(0xA1, 0xFF):
            try:
                ch = bytes([hi, lo]).decode("gb2312")
            except UnicodeDecodeError:
                continue
            if unicodedata.category(ch) == "Lo":
                out.add(ord(ch))
    return out


def build_unicodes() -> list[int]:
    codes: set[int] = set()
    for lo, hi in SYMBOL_RANGES:
        codes.update(range(lo, hi + 1))
    codes.update(gb2312_hanzi())
    return sorted(codes)


def download() -> None:
    ASSETS.mkdir(parents=True, exist_ok=True)
    print(f"下载字体：{FONT_URL}")
    subprocess.run(
        ["curl", "-sSL", "-C", "-", "-o", str(FULL), FONT_URL, "--max-time", "900", "--retry", "5"],
        check=True,
    )
    if not FULL.exists() or FULL.stat().st_size < 1_000_000:
        sys.exit(f"下载失败或文件过小：{FULL}")


def subset(codes: list[int]) -> None:
    codes_file = ASSETS / "unicodes.txt"
    # pyftsubset 的 --unicodes-file 每行一个十六进制码位（可带 U+ 前缀）
    codes_file.write_text("\n".join(f"U+{c:04X}" for c in codes) + "\n", encoding="utf-8")
    print(f"子集化：{len(codes)} 个码位 → {OUTPUT.name}")
    subprocess.run(
        [
            sys.executable,
            "-m",
            "fontTools.subset",
            str(FULL),
            f"--unicodes-file={codes_file}",
            f"--output-file={OUTPUT}",
            "--layout-features=kern,liga",
            "--no-hinting",
            "--desubroutinize",
            "--drop-tables+=DSIG",
            "--name-IDs=1,2,3,4,6",
            "--notdef-outline",
        ],
        check=True,
    )
    codes_file.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep-full", action="store_true", help="保留完整字体（默认删除以省空间）")
    parser.add_argument("--skip-download", action="store_true", help="已有完整字体时跳过下载")
    args = parser.parse_args()

    if not args.skip_download or not FULL.exists():
        download()
    subset(build_unicodes())

    full_mb = FULL.stat().st_size / 1_048_576
    out_mb = OUTPUT.stat().st_size / 1_048_576
    print(f"完成：{full_mb:.2f}MB → {out_mb:.2f}MB（压缩率 {out_mb / full_mb:.0%}）")
    if not args.keep_full:
        FULL.unlink(missing_ok=True)
        print("已删除完整字体（中间产物）")


if __name__ == "__main__":
    main()

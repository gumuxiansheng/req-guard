#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""req-guard 发布包组装辅助脚本

由 scripts/make-release-package.sh 调用，负责四类纯 Python 工作：

    deps     从 cargo metadata 采集依赖许可证，生成第三方声明与逐包清单
    subst    替换文档里的 {{占位符}}，并为 .ps1 补 UTF-8 BOM（Windows PowerShell 5.1 必需）
    sums     生成 SHA256SUMS.txt（相对路径、'/' 分隔，跨平台一致）
    archive  打包 zip / tar.gz（可裁剪掉其它平台的 bin 目录）

这样拆分的理由：bash 擅长拷贝/权限/流程编排，Python 擅长编码、JSON 与归档；
两者混在一个 bash 脚本里写中文与引号很容易出错。
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path

# Windows PowerShell 5.1 按 ANSI 读取无 BOM 的 .ps1 → 中文乱码 + ParserError。
# 项目里生成的 hook 脚本同样强制 BOM（见 core/src/gate.rs ps1_with_bom）。
BOM = b"\xef\xbb\xbf"

TEXT_SUFFIXES = {".md", ".txt", ".yml", ".yaml", ".sh", ".ps1", ".json"}


# --------------------------------------------------------------------------
# deps：第三方依赖许可证
# --------------------------------------------------------------------------
def cmd_deps(args: argparse.Namespace) -> int:
    meta_path = Path(args.meta)
    if not meta_path.is_file():
        print("[deps] 无 cargo metadata，跳过（沿用包内既有声明）", file=sys.stderr)
        return 0

    meta = json.loads(meta_path.read_text(encoding="utf-8"))
    pkgs = {}
    for p in meta.get("packages", []):
        if not p.get("source"):      # source 为空 = 工作区自身成员（core/cli/tui/gui）
            continue
        pkgs[p["name"]] = p

    # 逐包清单（名称 / 版本 / 许可证）
    lines = []
    for name in sorted(pkgs, key=str.lower):
        p = pkgs[name]
        lic = (p.get("license") or "(未在 metadata 中声明)").strip()
        repo = p.get("repository") or ""
        lines.append(f"{name} {p.get('version', '')} | {lic}" + (f" | {repo}" if repo else ""))

    txt_path = Path(args.out_txt)
    txt_path.parent.mkdir(parents=True, exist_ok=True)
    header = [
        "req-guard 第三方 Rust 依赖清单",
        f"生成时间：{datetime.now(timezone.utc).astimezone().isoformat(timespec='seconds')}",
        f"依赖总数：{len(pkgs)}（不含工作区自身成员 core / cli / tui / gui）",
        "字段：包名 版本 | 许可证 | 仓库",
        "-" * 78,
    ]
    txt_path.write_text("\n".join(header + lines) + "\n", encoding="utf-8", newline="\n")

    # 按许可证聚合的摘要表（写进 THIRD-PARTY-NOTICES.md）
    by_license: dict[str, list[str]] = {}
    for name, p in pkgs.items():
        key = (p.get("license") or "(未声明)").strip()
        by_license.setdefault(key, []).append(name)

    rows = ["| 许可证 | 包数 | 代表组件 |", "|---|---:|---|"]
    for lic in sorted(by_license, key=lambda k: (-len(by_license[k]), k)):
        sample = "、".join(sorted(by_license[lic], key=str.lower)[:3])
        rows.append(f"| `{lic}` | {len(by_license[lic])} | {sample} |")
    summary = "\n".join(rows)

    _replace_in_file(
        Path(args.out_md),
        {
            "{{DEP_TOTAL}}": str(len(pkgs)),
            "{{DEP_SUMMARY}}": summary,
            "{{DEP_GENERATED_AT}}": datetime.now(timezone.utc).astimezone().strftime("%Y-%m-%d %H:%M:%S"),
        },
    )
    print(f"[deps] 依赖 {len(pkgs)} 个 → {txt_path.name}")
    return 0


# --------------------------------------------------------------------------
# subst：占位符替换 + ps1 BOM
# --------------------------------------------------------------------------
def cmd_subst(args: argparse.Namespace) -> int:
    root = Path(args.root)
    values = dict(args.define or [])
    n = 0
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix.lower() not in TEXT_SUFFIXES:
            continue
        raw = path.read_bytes()
        try:
            text = raw.decode("utf-8-sig")
        except UnicodeDecodeError:
            continue
        new = text.replace("\r\n", "\n")   # 统一 LF：.sh 带 CRLF 在 Linux 上会直接执行失败
        for k, v in values.items():
            new = new.replace(k, v)
        if new != text:
            path.write_text(new, encoding="utf-8", newline="\n")
            n += 1
        # .ps1 必须有 BOM：Windows PowerShell 5.1 按 ANSI 读无 BOM 文件 → 中文乱码 + ParserError
        if path.suffix.lower() == ".ps1":
            data = path.read_bytes()
            if not data.startswith(BOM):
                path.write_bytes(BOM + data)
    print(f"[subst] 替换 {n} 个文件（占位符 {len(values)} 个）")
    return 0


# --------------------------------------------------------------------------
# sums：SHA256SUMS.txt
# --------------------------------------------------------------------------
def cmd_sums(args: argparse.Namespace) -> int:
    root = Path(args.root)
    out = Path(args.out)
    entries = []
    for path in sorted(root.rglob("*"), key=lambda p: str(p.relative_to(root)).replace("\\", "/")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel == out.name:
            continue
        h = hashlib.sha256()
        with path.open("rb") as f:
            for chunk in iter(lambda: f.read(65536), b""):
                h.update(chunk)
        entries.append(f"{h.hexdigest()}  {rel}")
    out.write_text("\n".join(entries) + "\n", encoding="utf-8", newline="\n")
    print(f"[sums] {len(entries)} 个文件 → {out.name}")
    return 0


# --------------------------------------------------------------------------
# archive：zip / tar.gz，可裁剪平台目录
# --------------------------------------------------------------------------
def _mode_for(path: Path) -> int:
    """归档内文件的权限位：可执行物（.sh 脚本、Linux 二进制）0755，其余 0644。

    构建机在 Windows 上时 chmod 对本地文件无效，因此权限必须写在归档条目里；
    Windows 的 .exe 由系统按扩展名识别，不需要 Unix 执行位。
    """
    name = path.name.lower()
    if name.endswith(".exe"):
        return 0o644
    if name.endswith(".sh"):
        return 0o755
    parts = {p.lower() for p in path.parts}
    if "bin" in parts:            # bin/<平台>/req-guard、req-guard-ui …
        return 0o755
    return 0o644


def cmd_archive(args: argparse.Namespace) -> int:
    """打包 zip / tar.gz。

    全流程**不执行任何删除**：
      · 产物先写 .tmp 再 os.replace 覆盖（不 unlink 已存在的归档）
      · 中间目录建在 trash_root 下，用完改名留在原地而不 rmtree
    这样脚本在受删除保护的环境（CI/沙箱）里也能无人值守跑完。
    """
    src = Path(args.root)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp_out = out.with_name(out.name + ".tmp")

    trash_root = Path(args.trash_root) if args.trash_root else out.parent / ".trash"
    trash_root.mkdir(parents=True, exist_ok=True)
    work = trash_root / f"_build-{src.name}-{os.getpid()}"
    if work.exists():                      # 同名残留：改名让路，不删除
        work.rename(work.with_name(work.name + "-prev"))
    shutil.copytree(src, work)

    # 裁剪掉不属于本归档的平台目录（keep_bin 为空 = 全平台归档，不裁剪）
    if args.keep_bin:
        # 裁剪同样只"移出"不删除：挪到 trash_root 下，避免任何 unlink/rmtree
        bin_dir = work / "bin"
        for child in sorted(bin_dir.iterdir()):
            if child.is_dir() and child.name not in args.keep_bin:
                dst = trash_root / f"_pruned-{child.name}-{os.getpid()}"
                n = 0
                while dst.exists():
                    n += 1
                    dst = trash_root / f"_pruned-{child.name}-{os.getpid()}-{n}"
                child.rename(dst)

    # 归档内的校验清单必须**只包含**本归档的文件
    if args.regenerate_sums:
        subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "sums",
             "--root", str(work), "--out", str(work / "SHA256SUMS.txt")],
            check=True,
        )

    if args.format == "zip":
        with zipfile.ZipFile(tmp_out, "w", zipfile.ZIP_DEFLATED) as z:
            for path in sorted(work.rglob("*")):
                if path.is_file():
                    # 归档内的根文件夹必须用发布包名（work 是 .trash 下的临时目录，名字不能外泄）
                    rel = path.relative_to(work).as_posix()
                    zi = zipfile.ZipInfo(f"{src.name}/{rel}")
                    zi.compress_type = zipfile.ZIP_DEFLATED
                    zi.external_attr = (_mode_for(path) << 16)
                    z.writestr(zi, path.read_bytes())
    else:
        import tarfile

        def _tar_filter(ti: tarfile.TarInfo) -> tarfile.TarInfo:
            # 归档条目里显式写权限位：构建机若是 Windows，chmod 在本地文件系统上根本不生效，
            # 但 tar/zip 里记录的 mode 会在 Linux 解压时还原——这才是用户真正拿到的执行位。
            # 目录必须保持 0755，否则解压后无法进入（0644 的目录不可遍历）。
            ti.mode = 0o755 if ti.isdir() else _mode_for(Path(ti.name))
            ti.uid = ti.gid = 0
            ti.uname = ti.gname = "root"
            return ti

        with tarfile.open(tmp_out, "w:gz") as t:
            t.add(work, arcname=src.name, filter=_tar_filter)

    os.replace(tmp_out, out)               # 原子覆盖，不经过删除
    # 中间目录改名留存（dist/.trash 下），需要时人工清理
    work.rename(work.with_name(work.name + "-done"))

    size_mb = out.stat().st_size / 1024 / 1024
    print(f"[archive] {out.name}  {size_mb:.2f} MB")
    return 0


# --------------------------------------------------------------------------
def _replace_in_file(path: Path, mapping: dict[str, str]) -> None:
    if not path.is_file():
        print(f"[deps] 警告：找不到 {path}", file=sys.stderr)
        return
    text = path.read_text(encoding="utf-8")
    for k, v in mapping.items():
        text = text.replace(k, v)
    path.write_text(text, encoding="utf-8", newline="\n")


def main() -> int:
    ap = argparse.ArgumentParser(description="req-guard 发布包组装辅助脚本")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("deps")
    p.add_argument("--meta", required=True)
    p.add_argument("--out-md", required=True)
    p.add_argument("--out-txt", required=True)
    p.set_defaults(func=cmd_deps)

    p = sub.add_parser("subst")
    p.add_argument("--root", required=True)
    p.add_argument("--define", nargs="*", default=[], metavar="KEY=VALUE")
    p.set_defaults(func=cmd_subst)

    p = sub.add_parser("sums")
    p.add_argument("--root", required=True)
    p.add_argument("--out", required=True)
    p.set_defaults(func=cmd_sums)

    p = sub.add_parser("archive")
    p.add_argument("--root", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--format", choices=["zip", "targz"], default="zip")
    p.add_argument("--keep-bin", nargs="*", default=[])
    p.add_argument("--regenerate-sums", action="store_true")
    p.add_argument("--trash-root", default="")
    p.set_defaults(func=cmd_archive)

    args = ap.parse_args()

    if args.cmd == "subst":
        pairs = []
        for item in args.define:
            if "=" not in item:
                continue
            k, v = item.split("=", 1)
            pairs.append((k, v))
        args.define = pairs
    if args.cmd == "archive":
        args.format = "targz" if args.format == "targz" else "zip"

    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())

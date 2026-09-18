#!/usr/bin/env bash
# req-guard 本机多目标 Release 构建脚本（Windows 宿主版）
#
# 定位：供 **Windows 开发机** 直接产出发布包所需的全部二进制；
#       与 scripts/build-release.sh（Linux CI 版，5 目标 × 2 变体）互补。
#
# 产物矩阵（受本机已安装 target 限制，缺件自动跳过并在结尾汇总）：
#   ┌ 目标                       平台            变体
#   ├ x86_64-pc-windows-msvc     Windows amd64   cli / ui / gui   （宿主原生构建）
#   ├ x86_64-pc-windows-gnu      Windows amd64   cli / ui         （需 mingw-w64，缺则跳过）
#   ├ x86_64-unknown-linux-musl  Linux amd64     cli / ui         （rust-lld 静态链接）
#   └ aarch64-unknown-linux-musl Linux arm64     cli / ui         （rust-lld 静态链接）
#
# 变体说明：
#   cli  默认特性，零 UI 依赖（core + cli）
#   ui   --features tui，CLI + 终端界面（ratatui）
#   gui  --features gui，CLI + 桌面界面（eframe，仅原生构建；无法交叉编译）
#
# GUI 只能原生构建：eframe/wgpu 依赖 X11/Wayland/GTK 系统库，交叉编译必然失败，
# 与 scripts/build-release.sh 的策略一致（GUI 由各平台原生执行机产出）。
#
# 用法：
#   bash scripts/build-release-local.sh              # 全量构建
#   bash scripts/build-release-local.sh --only msvc  # 只构建 Windows 宿主目标
#   bash scripts/build-release-local.sh --only musl  # 只构建两个 Linux musl 目标
#   bash scripts/build-release-local.sh msvc         # 位置参数等价写法
set -euo pipefail

# 始终以仓库根为工作目录（本地/CI 均可在任意 cwd 下调用）
cd "$(dirname "$0")/.."

# ---- 环境准备 ----
# Git Bash 里 cargo 通常不在 PATH；MSVC 的 link.exe 又会被 GNU coreutils 的 `link` 遮蔽。
export PATH="$HOME/.cargo/bin:$PATH"

MSVC_VER="${MSVC_VER:-14.44.35207}"
MSVC_ROOT="/c/Program Files/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC/${MSVC_VER}"
KITS_ROOT="/c/Program Files (x86)/Windows Kits/10/Lib/10.0.22621.0"
if [ -d "${MSVC_ROOT}/bin/Hostx64/x64" ]; then
  export PATH="${MSVC_ROOT}/bin/Hostx64/x64:$PATH"
  export LIB="C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\VC\\Tools\\MSVC\\${MSVC_VER}\\lib\\x64;C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\10.0.22621.0\\um\\x64;C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\10.0.22621.0\\ucrt\\x64"
fi

OUT_DIR="dist/raw"
PKG="req-guard"

# ---- 参数：--only <msvc|musl|gnu|macos>，也接受位置参数等价写法 ----
ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --only)    ONLY="$2"; shift 2 ;;
    --only=*)  ONLY="${1#--only=}"; shift ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)         ONLY="$1"; shift ;;
  esac
done
case "${ONLY}" in msvc|musl|gnu|macos|"") ;; *) echo "未知的 --only 取值：${ONLY}" >&2; exit 2 ;; esac

VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
echo "==> req-guard v${VERSION} 本机多目标构建"
rustc --version
cargo --version
echo "==> 已安装 target："
rustup target list --installed | sed 's/^/    /'

mkdir -p "${OUT_DIR}/cli" "${OUT_DIR}/ui" "${OUT_DIR}/gui"
declare -a BUILT=()
declare -a SKIPPED=()

# build_one <target> <variant> <features>
build_one() {
  local target="$1" variant="$2" features="$3"
  local ext=""
  case "${target}" in *windows*) ext=".exe" ;; esac
  local src="target/${target}/release/${PKG}${ext}"
  local dst="${OUT_DIR}/${variant}/${PKG}-${variant}-${target}${ext}"

  # windows-msvc 默认**动态**链接 CRT（依赖 VCRUNTIME140.dll），用户机器上没有 VC++ 运行库
  # 就会 "找不到 VCRUNTIME140.dll" 直接跑不起来。发布包要求"下载即运行"，故强制静态链接 CRT。
  # （musl / gnu 目标本来就是静态的，不需要这个 flag。）
  local rustflags=""
  case "${target}" in
    *windows-msvc*) rustflags="-C target-feature=+crt-static" ;;
  esac

  echo "---- [${variant}] ${target} (features='${features}'${rustflags:+, RUSTFLAGS='$rustflags'})"
  rm -f "${src}"
  if RUSTFLAGS="${rustflags}" cargo build --release -p "${PKG}" --target "${target}" ${features:+--features "${features}"}; then
    if [ -f "${src}" ]; then
      cp -f "${src}" "${dst}"
      echo "     -> ${dst} ($(du -h "${dst}" | cut -f1))"
      BUILT+=("${variant}/${target}")
      return 0
    fi
  fi
  echo "     !! 跳过（构建失败或产物缺失）"
  SKIPPED+=("${variant}/${target}")
  return 0
}

# ---- Windows 宿主（MSVC）：唯一能产出 GUI 的目标 ----
if [ "${ONLY}" = "" ] || [ "${ONLY}" = "msvc" ]; then
  build_one x86_64-pc-windows-msvc cli ""
  build_one x86_64-pc-windows-msvc ui  "tui"
  build_one x86_64-pc-windows-msvc gui "gui"
fi

# ---- Linux 静态链接（musl）：CLI + TUI 依赖链零 C 代码，rust-lld 即可链接 ----
if [ "${ONLY}" = "" ] || [ "${ONLY}" = "musl" ]; then
  build_one x86_64-unknown-linux-musl   cli ""
  build_one x86_64-unknown-linux-musl   ui  "tui"
  build_one aarch64-unknown-linux-musl  cli ""
  build_one aarch64-unknown-linux-musl  ui  "tui"
fi

# ---- Windows GNU（可选）：需要 mingw-w64 的 x86_64-w64-mingw32-gcc ----
if [ "${ONLY}" = "" ] || [ "${ONLY}" = "gnu" ]; then
  if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    build_one x86_64-pc-windows-gnu cli ""
    build_one x86_64-pc-windows-gnu ui  "tui"
  else
    echo "---- [gnu] 未检测到 x86_64-w64-mingw32-gcc，跳过 windows-gnu（改用 msvc 产物）"
    SKIPPED+=("cli/x86_64-pc-windows-gnu" "ui/x86_64-pc-windows-gnu")
  fi
fi

# ---- macOS（可选）：需 Zig + cargo-zigbuild，本机无 Zig 时跳过 ----
if [ "${ONLY}" = "" ] || [ "${ONLY}" = "macos" ]; then
  if command -v zig >/dev/null 2>&1 && command -v cargo-zigbuild >/dev/null 2>&1; then
    echo "---- [macos] 检测到 Zig，尝试 cargo-zigbuild（best-effort）"
    # 占位：Zig 可用时在此追加 x86_64-apple-darwin / aarch64-apple-darwin 构建
    echo "     （本脚本未内置 macOS 构建步骤，请用 scripts/build-release.sh 在 Linux 执行机产出）"
  else
    echo "---- [macos] 无 Zig，跳过（best-effort，不阻断发布）"
  fi
fi

echo
echo "==> 构建完成"
echo "    成功：${#BUILT[@]} 项"
for b in "${BUILT[@]:-}"; do [ -n "$b" ] && echo "      - $b"; done
echo "    跳过：${#SKIPPED[@]} 项"
for s in "${SKIPPED[@]:-}"; do [ -n "$s" ] && echo "      - $s"; done
echo "    产物目录：${OUT_DIR}/"

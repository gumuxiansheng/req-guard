#!/usr/bin/env bash
# req-guard 多平台 Release 二进制构建脚本
# （供 CNB tag_push 流水线调用，也可在任意 Linux x86_64 机器上手动复现）
#
# 产物矩阵：5 个目标 × 2 个变体
#   目标                       说明
#   ─────────────────────────  ────────────────────────────────────────────
#   x86_64-unknown-linux-musl  Linux amd64，静态链接（无 glibc 依赖）
#   aarch64-unknown-linux-musl Linux arm64，静态链接
#   x86_64-pc-windows-gnu      Windows amd64（mingw-w64 交叉链接）
#   x86_64-apple-darwin        macOS Intel（cargo-zigbuild）
#   aarch64-apple-darwin       macOS Apple Silicon（cargo-zigbuild）
#
#   变体                       产物名                        失败策略
#   ─────────────────────────  ────────────────────────────  ────────────
#   默认（零依赖 CLI）         req-guard-<target>[.exe]     必需，失败即整体失败
#   CLI + TUI（--features tui）req-guard-ui-<target>[.exe]  必需（可用环境变量降级）
#
# 与 sql-guard（scripts/build-release.sh）的差异 —— 均为 req-guard 的实际情况所决定：
#   1. **不需要** Zig 充当交叉 C 编译器：req-guard 的 CLI/TUI 依赖链里没有任何
#      需要编译 C 的 crate（无 cc/psm/stacker），因此 sql-guard 里那套
#      make_zig_cc 封装脚本在这里是多余的。Zig 只用于 macOS 目标的 cargo-zigbuild。
#   2. **最低工具链 1.88.0**（不是 sql-guard 的 1.80）：ratatui 0.30.2 的
#      rust-version = 1.88.0，故 .cnb.yml 的镜像与 RUSTUP_TOOLCHAIN 都锚定 1.88。
#   3. **GUI（eframe/wgpu）不参与交叉编译**：Linux 侧依赖 X11/Wayland/GTK 系统库，
#      只能由各平台原生执行机（GitHub Actions windows/macos）构建，见 README。
#   4. 额外产出 dist/SHA256SUMS，便于发版后校验附件完整性（可删）。
#
# macOS 目标为 best-effort：Zig/cargo-zigbuild 拿不到时自动跳过，不阻断
# Linux/Windows 产物发布（与 sql-guard 策略一致）。
set -euo pipefail

# 始终以仓库根为工作目录（本地/CI 均可在任意 cwd 下调用）
cd "$(dirname "$0")/.."

# ---- 可配置项 ----
ZIG_VERSION="0.13.0"
ZIGBUILD_VERSION="0.18.4"
TARGETS=(
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-pc-windows-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
)
# TUI 变体默认"必需"（fail-closed，避免发布缺件却看似完整的版本）；
# 置 REQGUARD_TUI_BEST_EFFORT=1 可降级为"跳过并告警"。
TUI_BEST_EFFORT="${REQGUARD_TUI_BEST_EFFORT:-0}"
# CI 里由 .cnb.yml 的 env 指定（1.88.0），本地缺省跟随 rust-toolchain.toml 的 stable
TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"

echo "==> Rust toolchain"
rustc --version
cargo --version

# ---- 版本一致性检查（tag 与 Cargo.toml 必须一致，否则产物版本会名不符实）----
CRATE_VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
TAG="${CNB_BRANCH:-}"
if [ -z "${TAG}" ]; then
  TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"
fi
echo "==> Cargo.toml 版本: ${CRATE_VERSION:-<未解析到>} / 触发 tag: ${TAG:-<无>}"
if [ -n "${TAG}" ]; then
  if [ "${TAG#v}" = "${CRATE_VERSION}" ]; then
    echo "    版本一致性检查通过"
  else
    echo "    warn: tag ${TAG} 与 Cargo.toml 版本 ${CRATE_VERSION} 不一致！"
    echo "          Release 附件将以 Cargo.toml 版本为准（req-guard --version 可验证）。"
  fi
fi

# ---- 系统依赖 ----
# binutils        : 宿主 strip（ELF/PE）
# binutils-aarch64: aarch64-linux-gnu-strip，宿主 strip 不认 ARM64 ELF
# gcc-mingw-w64   : windows-gnu 目标的链接器与 dlltool（rustc 的 windows-gnu
#                   链接流程会显式调用 dlltool，与选用哪个 linker 无关）
# xz-utils/unzip  : 解压 Zig wheel；grep 供镜像索引解析
# 注：与 sql-guard 相比不再需要 xz 之外的额外交叉 C 编译器（无 C 依赖）。
echo "==> 安装系统依赖（binutils / binutils-aarch64 / gcc-mingw-w64 / xz-utils / unzip / grep）"
apt-get update -qq
apt-get install -y -qq binutils binutils-aarch64-linux-gnu gcc-mingw-w64-x86-64 xz-utils unzip grep

# ---- Zig（仅 macOS 目标需要：cargo-zigbuild 用 Zig 自带的链接器与 macOS SDK）----
# best-effort：装不上就跳过 macOS 目标，不影响 Linux/Windows 产物。
# 国内/CNB 网络直连 ziglang.org 极慢（实测约 5KB/s，曾被平台 watchdog 按
# "10 分钟无输出"kill），故走清华 PyPI 镜像（实测约 6MB/s）。
# 另注：CI 非 tty，curl 默认静默无进度输出，务必带 --progress-bar(-#) 防误杀。
ZIG_READY=0
echo "==> 安装 Zig ${ZIG_VERSION}（macOS 目标需要；失败则跳过 macOS）"
ZIG_WHEEL="ziglang-${ZIG_VERSION}-py3-none-manylinux_2_12_x86_64.manylinux2010_x86_64.musllinux_1_1_x86_64.whl"
ZIG_WHEEL_URL=$(curl -fsSL --connect-timeout 15 --max-time 60 --retry 2 --retry-delay 2 \
  "https://pypi.tuna.tsinghua.edu.cn/simple/ziglang/" \
  | grep -oE "href=\"[^\"]*${ZIG_WHEEL}[^\"]*\"" | head -1 \
  | sed -E 's/^href="//; s/"$//; s#^\.\./\.\./#https://pypi.tuna.tsinghua.edu.cn/#') || true
echo "    wheel url: ${ZIG_WHEEL_URL:-<未解析到>}"
if [ -n "${ZIG_WHEEL_URL}" ] \
   && curl -fL# --connect-timeout 15 --max-time 180 --retry 2 --retry-delay 2 \
        "${ZIG_WHEEL_URL}" -o "/tmp/${ZIG_WHEEL}" \
   && unzip -oq "/tmp/${ZIG_WHEEL}" -d /tmp/ziglang-wheel \
   && ln -sf "/tmp/ziglang-wheel/ziglang/zig" /usr/local/bin/zig \
   && chmod +x /usr/local/bin/zig \
   && zig version; then
  ZIG_READY=1
else
  echo "    warn: Zig 安装失败，macOS 目标将被跳过（Linux/Windows 不受影响）"
fi

# ---- cargo-zigbuild（仅 macOS 目标需要）----
if [ "${ZIG_READY}" = "1" ]; then
  echo "==> 安装 cargo-zigbuild ${ZIGBUILD_VERSION}"
  # 优先取 GitHub 预编译二进制（~1MB，秒级），失败再回退 cargo install（编译较慢）
  CZB_URL="https://github.com/rust-cross/cargo-zigbuild/releases/download/v${ZIGBUILD_VERSION}/cargo-zigbuild-v${ZIGBUILD_VERSION}.x86_64-unknown-linux-musl.tar.gz"
  if curl -fL# --connect-timeout 15 --max-time 120 --retry 1 "${CZB_URL}" -o /tmp/cargo-zigbuild.tar.gz \
     && tar -xzf /tmp/cargo-zigbuild.tar.gz -C /usr/local/bin \
     && chmod +x /usr/local/bin/cargo-zigbuild; then
    cargo-zigbuild --version
  elif ! cargo install "cargo-zigbuild" --version "${ZIGBUILD_VERSION}" --locked; then
    echo "    warn: cargo-zigbuild 安装失败，macOS 目标将被跳过"
    ZIG_READY=0
  fi
fi

# 交叉环境下 cargo 自带的 strip 可能找不到目标平台的 strip 程序，
# 统一关闭它，改为构建后用 binutils strip 处理（见下方 STRIP 分支）。
export CARGO_PROFILE_RELEASE_STRIP=false

echo "==> 添加 Rust 目标（toolchain: ${TOOLCHAIN}）"
rustup target add --toolchain "${TOOLCHAIN}" "${TARGETS[@]}"

# ---- 准备干净产物目录 ----
DIST="$(pwd)/dist"
if [ "${DIST}" = "/dist" ] || [ -z "${DIST}" ]; then
  echo "error: dist 目录解析异常（${DIST}），拒绝继续" >&2
  exit 1
fi
rm -rf "${DIST}"
mkdir -p "${DIST}"

# 构建单个目标的单个变体；args 追加到 cargo 命令行
cargo_for_target() {
  local T="$1"; shift
  if [ "${T}" = *apple-darwin ]; then
    # macOS：必须由 zigbuild 提供链接器 + SDK
    MACOSX_DEPLOYMENT_TARGET=10.12 cargo zigbuild --release --target "${T}" --locked "$@"
  else
    cargo build --release --target "${T}" --locked "$@"
  fi
}

copy_and_strip() {
  local T="$1" SRC="$2" DST="$3" STRIP_TOOL="$4"
  cp "${SRC}" "${DST}"
  if [ -z "${STRIP_TOOL}" ]; then
    echo "    -> ${DST} （strip 跳过：Mach-O 需在 macOS 环境剥离）"
  elif command -v "${STRIP_TOOL}" >/dev/null 2>&1 && ${STRIP_TOOL} "${DST}"; then
    echo "    -> ${DST} (stripped by ${STRIP_TOOL})"
  else
    echo "    warn: ${STRIP_TOOL} 不可用或失败，保留未剥离的 ${DST}"
  fi
}

for T in "${TARGETS[@]}"; do
  echo "==> 构建目标: ${T}"

  case "${T}" in
    *windows*) BINEXT=".exe" ;;
    *)         BINEXT="" ;;
  esac

  # 宿主 strip 是 x86_64 版，认不出 ARM64 ELF，故 aarch64 走 aarch64-linux-gnu-strip；
  # macOS（Mach-O）在 Linux 下无法用 GNU strip 处理，直接跳过。
  case "${T}" in
    aarch64-unknown-linux-musl) STRIP_TOOL="aarch64-linux-gnu-strip" ;;
    *apple-darwin)             STRIP_TOOL="" ;;
    *)                         STRIP_TOOL="strip" ;;
  esac

  case "${T}" in
    *apple-darwin)
      if [ "${ZIG_READY}" != "1" ]; then
        echo "    跳过：Zig/cargo-zigbuild 不可用（macOS 为 best-effort）"
        continue
      fi
      # macOS 整体 best-effort：失败只告警，不阻断后面的目标
      if ! cargo_for_target "${T}" -p req-guard; then
        echo "    warn: macOS 目标 ${T} 构建失败，跳过（best-effort）"
        continue
      fi
      ;;
    *)
      cargo_for_target "${T}" -p req-guard
      ;;
  esac

  # ---- 变体 1：默认构建（零依赖 CLI）----
  copy_and_strip "${T}" "target/${T}/release/req-guard${BINEXT}" \
                 "${DIST}/req-guard-${T}${BINEXT}" "${STRIP_TOOL}"

  # ---- 变体 2：CLI + TUI（--features tui，写往同一路径，故必须先拷贝再覆盖）----
  echo "    构建 TUI 变体（--features tui）"
  if cargo_for_target "${T}" -p req-guard --features tui; then
    copy_and_strip "${T}" "target/${T}/release/req-guard${BINEXT}" \
                   "${DIST}/req-guard-ui-${T}${BINEXT}" "${STRIP_TOOL}"
  elif [ "${TUI_BEST_EFFORT}" = "1" ]; then
    echo "    warn: ${T} 的 TUI 变体构建失败，已跳过（REQGUARD_TUI_BEST_EFFORT=1）"
  else
    echo "error: ${T} 的 TUI 变体构建失败。若确认要放行，请设 REQGUARD_TUI_BEST_EFFORT=1 重跑。" >&2
    exit 1
  fi
done

# ---- 校验和（便于发版后核对附件完整性；随 dist/* 一起上传）----
echo "==> 生成 SHA256SUMS"
# 注意用 ./req-guard* 而非 ./*：重定向会先创建 SHA256SUMS 文件，
# 若用 ./* 会把这个空文件也哈希进去、并出现在自己的清单里。
( cd "${DIST}" && sha256sum ./req-guard* > SHA256SUMS )

echo "==> 构建产物:"
ls -lh "${DIST}"
echo "==> SHA256SUMS:"
cat "${DIST}/SHA256SUMS"

# 至少要有 x86_64 Linux 产物，否则视为失败（fail-closed 兜底）
if [ ! -f "${DIST}/req-guard-x86_64-unknown-linux-musl" ]; then
  echo "error: 关键产物 req-guard-x86_64-unknown-linux-musl 缺失" >&2
  exit 1
fi

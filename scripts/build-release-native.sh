#!/usr/bin/env bash
# req-guard 本机（原生）Release 构建脚本 —— 只想在本机跑、要一份可直接分发的二进制时用。
#
# 与 scripts/build-release.sh 的分工（**不要混淆二者**）：
#   build-release.sh         CI 用：5 个交叉目标 × 2 变体，会 apt-get 装 mingw/binutils、
#                            下载 Zig 与 cargo-zigbuild 来交叉链接 macOS 目标。
#                            Debian 容器专用，在 macOS 上跑会直接卡在 apt-get。
#   build-release-native.sh  本机用：只编 host 三元组，**不装任何东西、不联网**，
#                            产物命名与 CI 矩阵严格对齐，可直接丢进 dist/ 上传。
#
# 用法：
#   ./scripts/build-release-native.sh
#     → dist/req-guard-<host-triple>[.exe]
#       dist/req-guard-ui-<host-triple>[.exe]
#       dist/SHA256SUMS
#
# 产物命名与 build-release.sh 完全一致（都是 req-guard[-ui]-<target>[.exe]），
# 所以本机产物与其它平台产物放同一个 dist/ 不会互相覆盖、也不会被误当成同一平台的件。
# 需要 GUI 变体时（GUI 无法交叉编译，本机才能编）用：
#   cargo build --release -p req-guard --features full
#   cp target/release/req-guard dist/req-guard-gui-<host-triple>
#
# 与 sql-guard 的差异：CLI/TUI 依赖链零 C 代码，本机构建无需 Zig 当 C 编译器。
set -euo pipefail

# 始终以仓库根为工作目录（本地/CI 均可在任意 cwd 下调用）
cd "$(dirname "$0")/.."

# ---- 本机目标三元组：从 rustc 自报的 host 取，绝不写死 ----
# 写死会在 Apple Silicon 上链接出 x86_64（Rosetta 壳）而在 Intel Mac 上跑不动。
HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p' || true)"
if [ -z "${HOST_TRIPLE}" ]; then
  echo "error: 无法从 rustc -vV 解析 host 三元组，拒绝继续" >&2
  exit 1
fi
case "${HOST_TRIPLE}" in
  *windows*) BINEXT=".exe" ;;
  *)         BINEXT="" ;;
esac
echo "==> 本机目标: ${HOST_TRIPLE}${BINEXT}（rustc: $(rustc --version)）"

# ---- 版本一致性检查（产物版本必须来自根 Cargo.toml，否则名不符实）----
# 这里刻意用 awk 而不是 `sed -n 's/.../\1/p'`（build-release.sh 里那个写法）：
# 本机 sed 是 **toybox 0.8.13** 而非 GNU sed，其 BRE 里 `[[:space:]]` 与 `v\?`
# 组合解析异常，会静默返回空串 → 版本自证假失败。awk 在 GNU/BSD/toybox 上行为一致。
CRATE_VERSION="$(awk -F'"' '$1 ~ /^version[[:space:]]*=/ { print $2; exit }' Cargo.toml)"
if [ -z "${CRATE_VERSION}" ]; then
  echo "error: 未从根 Cargo.toml 解析到 version" >&2
  exit 1
fi
echo "==> Cargo.toml 版本: ${CRATE_VERSION}"

# ---- 准备产物目录 ----
DIST="$(pwd)/dist"
if [ "${DIST}" = "/dist" ] || [ -z "${DIST}" ]; then
  echo "error: dist 目录解析异常（${DIST}），拒绝继续" >&2
  exit 1
fi
mkdir -p "${DIST}"
# 注意：这里**故意不清理 dist/** —— 里面可能有其它平台的产物和上游 CI 产物，
# 只覆盖本机这两个文件名，旧文件交由使用者自行决定。

# 本机产物用宿主自带的 strip 即可（交叉脚本里"aarch64 走 arm strip / Mach-O 跳过"
# 那些分支在本机场景都不成立，宿主的 strip 认得自己的目标格式）。
copy_and_strip() {
  local SRC="$1" DST="$2"
  cp "${SRC}" "${DST}"
  local STRIP_TOOL="strip"
  if command -v "${STRIP_TOOL}" >/dev/null 2>&1 && "${STRIP_TOOL}" "${DST}"; then
    echo "    -> ${DST} (stripped by ${STRIP_TOOL})"
  else
    echo "    -> ${DST} （未剥离符号，宿主无可用 strip）"
  fi
}

# ---- 变体 1：默认（零依赖 CLI）----
echo "==> 构建变体 1：默认（零依赖 CLI）"
cargo build --release -p req-guard --locked
copy_and_strip "target/release/req-guard${BINEXT}" \
               "${DIST}/req-guard-${HOST_TRIPLE}${BINEXT}"

# ---- 变体 2：CLI + TUI（--features tui）----
# 写往同一路径，故必须先拷贝再覆盖，与 build-release.sh 的顺序一致。
echo "==> 构建变体 2：CLI + TUI"
cargo build --release -p req-guard --locked --features tui
copy_and_strip "target/release/req-guard${BINEXT}" \
               "${DIST}/req-guard-ui-${HOST_TRIPLE}${BINEXT}"

# ---- 产物版本自证：跑一遍二进制，确认它报的就是 Cargo.toml 的版本 ----
echo "==> 产物版本自证"
PROVED_BIN="${DIST}/req-guard-${HOST_TRIPLE}${BINEXT}"
ACTUAL="$("${PROVED_BIN}" --version 2>&1 | awk '{ print $NF; exit }')"
if [ "${ACTUAL}" = "${CRATE_VERSION}" ]; then
  echo "    通过：产物版本 ${ACTUAL} == Cargo.toml ${CRATE_VERSION}"
else
  echo "error: 产物版本自证失败 —— 二进制报 ${ACTUAL:-<空>}，Cargo.toml 是 ${CRATE_VERSION}" >&2
  exit 1
fi

# ---- 校验和（与 build-release.sh 同格式，可直接用于附件完整性核对）----
echo "==> 生成 SHA256SUMS"
# 用 ./req-guard* 而非 ./*：重定向会先创建空文件，./* 会把它连哈希一起写进自己清单。
( cd "${DIST}" && sha256sum "./req-guard"* > SHA256SUMS )

echo "==> 构建产物:"
ls -lh "${DIST}"
echo "==> SHA256SUMS:"
cat "${DIST}/SHA256SUMS"
echo "==> 完成：本机变体已在 dist/ 下，可直接分发（不含 GUI 变体，见脚本头注释）"

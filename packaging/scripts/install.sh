#!/usr/bin/env bash
# req-guard 一键安装脚本（Linux / macOS / Windows Git Bash）
#
# 做四件事：
#   1) 识别平台与架构，挑出 bin/ 下对应的二进制
#   2) 复制到安装目录并赋执行位
#   3) 校验 `req-guard -V` 与本包 VERSION 一致（防止装错版本/装错包）
#   4) 可选：把安装目录写进 shell 配置文件的 PATH、顺手在项目里 init 门禁
#
# 用法：
#   bash scripts/install.sh [选项]
#
# 选项：
#   --dir <目录>        安装目录，默认 $HOME/.local/bin（无写权限时回退）
#   --variant <cli|ui|gui>   变体，默认 cli（gui 仅 Windows 提供）
#   --bin-name <名字>   落盘文件名，默认随变体（req-guard / req-guard-ui / req-guard-gui）
#   --no-path           不修改 shell 配置文件
#   --init              安装后对当前目录执行 `req-guard init`
#   --force             覆盖已存在的同名文件
#   -h, --help          显示帮助
#
# Windows 原生 PowerShell 请用 scripts\install.ps1（它会写用户级 PATH，命令行与 PowerShell 都可用）。
set -euo pipefail

# 定位发布包根目录（本脚本位于 <包根>/scripts/）
PKG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ---- 默认参数 ----
VARIANT="cli"
INSTALL_DIR="${HOME}/.local/bin"
BIN_NAME=""
NO_PATH=0
DO_INIT=0
FORCE=0

# ---- 帮助 ----
usage() { sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dir)      INSTALL_DIR="$2"; shift 2 ;;
    --variant)  VARIANT="$2";    shift 2 ;;
    --bin-name) BIN_NAME="$2";   shift 2 ;;
    --no-path)  NO_PATH=1;       shift ;;
    --init)     DO_INIT=1;       shift ;;
    --force)    FORCE=1;         shift ;;
    -h|--help)  usage; exit 0 ;;
    *) echo "未知参数：$1（用 --help 查看用法）" >&2; exit 2 ;;
  esac
done

# ---- 版本信息（来自 VERSION 文件，缺失则以包目录名兜底）----
VERSION="$(sed -n 's/^version *= *"\?\([^" ]*\)"\?.*/\1/p' "${PKG_ROOT}/VERSION" 2>/dev/null | head -1)"
[ -n "${VERSION}" ] || VERSION="$(basename "${PKG_ROOT}" | sed 's/^req-guard-v//')"

# ---- 平台与架构识别 ----
OS="$(uname -s)"
ARCH_RAW="$(uname -m)"
case "${ARCH_RAW}" in
  x86_64|amd64)   ARCH="x86_64" ;;
  aarch64|arm64)  ARCH="aarch64" ;;
  *)              echo "不支持的架构：${ARCH_RAW}（本包仅提供 x86_64 / aarch64）" >&2; exit 1 ;;
esac

case "${OS}" in
  Linux*)                       PLATFORM="linux-${ARCH}";   EXE="" ;;
  Darwin*)                      PLATFORM="macos-${ARCH}";   EXE="" ;;
  MINGW*|MSYS*|CYGWIN*)         PLATFORM="windows-${ARCH}"; EXE=".exe" ;;
  *) echo "不支持的系统：${OS}" >&2; exit 1 ;;
esac

# ---- 变体 → 文件名 ----
case "${VARIANT}" in
  cli) [ -n "${BIN_NAME}" ] || BIN_NAME="req-guard" ;;
  ui)  [ -n "${BIN_NAME}" ] || BIN_NAME="req-guard-ui" ;;
  gui) [ -n "${BIN_NAME}" ] || BIN_NAME="req-guard-gui" ;;
  *) echo "--variant 只能是 cli / ui / gui" >&2; exit 2 ;;
esac

SRC_DIR="${PKG_ROOT}/bin/${PLATFORM}"
SRC="${SRC_DIR}/${BIN_NAME}${EXE}"

if [ ! -f "${SRC}" ]; then
  echo "找不到二进制：${SRC}" >&2
  echo "本包提供的平台与变体：" >&2
  find "${PKG_ROOT}/bin" -mindepth 1 -maxdepth 2 -type f 2>/dev/null | sed "s|^|  |" >&2
  [ "${PLATFORM}" = "macos-${ARCH}" ] && echo "提示：macOS 无预编译产物，请从源码构建（见 docs/安装指南.md §7）" >&2
  exit 1
fi

# ---- 安装目录可写性检查 ----
if [ ! -d "${INSTALL_DIR}" ]; then
  if ! mkdir -p "${INSTALL_DIR}" 2>/dev/null; then
    echo "无法创建安装目录 ${INSTALL_DIR}，请换一个（--dir）或用 sudo" >&2
    exit 1
  fi
fi
if [ ! -w "${INSTALL_DIR}" ]; then
  echo "安装目录不可写：${INSTALL_DIR}（改用 --dir 指定，或用 sudo）" >&2
  exit 1
fi

DEST="${INSTALL_DIR}/${BIN_NAME}${EXE}"
if [ -f "${DEST}" ] && [ "${FORCE}" -eq 0 ]; then
  echo "已存在：${DEST}（加 --force 覆盖）" >&2
  exit 1
fi

# ---- 复制 + 执行位 ----
cp -f "${SRC}" "${DEST}"
chmod 0755 "${DEST}"
echo "✅ 已安装：${DEST}"
echo "   来源  : bin/${PLATFORM}/${BIN_NAME}${EXE}"
echo "   大小  : $(du -h "${DEST}" | cut -f1)"

# ---- 版本自证 ----
GOT="$("${DEST}" -V 2>&1 | head -1 || true)"
if [ -n "${VERSION}" ]; then
  case "${GOT}" in
    *"${VERSION}"*) echo "   版本  : ${GOT}（与 VERSION 一致 ✅）" ;;
    *) echo "⚠️  版本不一致：二进制输出「${GOT}」，本包 VERSION 为 ${VERSION}" >&2
       echo "    请确认没有混装其它版本的发布包。" >&2
       exit 1 ;;
  esac
else
  echo "   版本  : ${GOT}"
fi

# ---- PATH ----
add_path_to() {
  local rc="$1" line="export PATH=\"${INSTALL_DIR}:\$PATH\""
  [ -f "${rc}" ] || return 0
  if grep -Fq "${INSTALL_DIR}" "${rc}" 2>/dev/null; then return 0; fi
  printf '\n# req-guard\n%s\n' "${line}" >> "${rc}"
  echo "   已把 PATH 写入 ${rc}"
}

IN_PATH=0
case ":${PATH}:" in *":${INSTALL_DIR}:"*) IN_PATH=1 ;; esac

if [ "${NO_PATH}" -eq 0 ] && [ "${IN_PATH}" -eq 0 ]; then
  add_path_to "${HOME}/.bashrc"
  add_path_to "${HOME}/.zshrc" 2>/dev/null || true
  add_path_to "${HOME}/.profile" 2>/dev/null || true
  echo
  echo "   当前会话请先执行：export PATH=\"${INSTALL_DIR}:\$PATH\""
  echo "   （新开终端自动生效）"
fi

# ---- 可选：初始化门禁 ----
if [ "${DO_INIT}" -eq 1 ]; then
  echo
  echo "==> 在当前项目初始化门禁"
  "${DEST}" init
fi

cat <<EOF

下一步：
  1) 确认命令可用：${BIN_NAME} -V
  2) 接入项目    ：cd your-project && ${BIN_NAME} init
  3) 校验就位    ：${BIN_NAME} install --verify
  4) 冒烟自检    ：bash ${PKG_ROOT}/scripts/selfcheck.sh
  文档：${PKG_ROOT}/docs/安装指南.md
EOF

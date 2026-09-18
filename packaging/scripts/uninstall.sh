#!/usr/bin/env bash
# req-guard 卸载脚本（Linux / macOS / Windows Git Bash）
#
# 只移除二进制与安装脚本写入的 PATH 条目；**不会**删除项目里的 .gates/（那是你的审核资产）。
#
# 用法：
#   bash scripts/uninstall.sh [选项]
#
# 选项：
#   --dir <目录>        安装目录，默认 $HOME/.local/bin
#   --bin-name <名字>   要删的文件名，默认 req-guard（同时清理 req-guard-ui / req-guard-gui）
#   --all               删除该目录下全部 req-guard* 文件（含 ui / gui 变体）
#   --purge-path        同时从 shell 配置文件移除 PATH 条目
#   -h, --help          显示帮助
set -euo pipefail

PKG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="${HOME}/.local/bin"
BIN_NAME="req-guard"
ALL=0
PURGE_PATH=0

usage() { sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dir)        INSTALL_DIR="$2"; shift 2 ;;
    --bin-name)   BIN_NAME="$2";   shift 2 ;;
    --all)        ALL=1;           shift ;;
    --purge-path) PURGE_PATH=1;    shift ;;
    -h|--help)    usage; exit 0 ;;
    *) echo "未知参数：$1（用 --help 查看用法）" >&2; exit 2 ;;
  esac
done

REMOVED=0

# 先收集**确实存在**的文件再删：不对不存在的路径调 rm，
# 免得在带删除保护/回收站代理的环境里产生无关的失败噪音。
if [ "${ALL}" -eq 1 ]; then
  CANDIDATES=("${INSTALL_DIR}"/req-guard "${INSTALL_DIR}"/req-guard.exe \
              "${INSTALL_DIR}"/req-guard-ui "${INSTALL_DIR}"/req-guard-ui.exe \
              "${INSTALL_DIR}"/req-guard-gui "${INSTALL_DIR}"/req-guard-gui.exe)
else
  CANDIDATES=("${INSTALL_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}.exe")
fi

for f in "${CANDIDATES[@]}"; do
  [ -f "${f}" ] || continue
  if rm -f "${f}" 2>/dev/null; then
    echo "已删除：${f}"
    REMOVED=$((REMOVED + 1))
  else
    echo "删除失败（权限或系统保护）：${f}" >&2
  fi
done

if [ "${REMOVED}" -eq 0 ]; then
  echo "未找到待删除文件（安装目录：${INSTALL_DIR}）"
  if [ "${ALL}" -eq 0 ]; then
    echo "提示：若装的是界面变体，请用 --bin-name req-guard-ui（或直接 --all 清理全部变体）"
  fi
fi

if [ "${PURGE_PATH}" -eq 1 ]; then
  for rc in "${HOME}/.bashrc" "${HOME}/.zshrc" "${HOME}/.profile"; do
    [ -f "${rc}" ] || continue
    if grep -Fq "${INSTALL_DIR}" "${rc}" 2>/dev/null; then
      # 删除 req-guard 注释行与其后的 PATH 行（脚本写入时是成对出现的）
      grep -v -F -e "${INSTALL_DIR}" -e "# req-guard" "${rc}" > "${rc}.tmp" || true
      mv -f "${rc}.tmp" "${rc}"
      echo "已清理 PATH 条目：${rc}"
    fi
  done
fi

cat <<EOF

卸载完成。
  · 项目里的 .gates/（需求清单、评论、审计）**未被删除**，如需撤掉门禁请手动处理：
      rm -rf <项目>/.gates
      还原 <项目>/.git/hooks/pre-commit
      移除 AI 工具配置中的 req-guard hook 段
  · 重新装回来：bash ${PKG_ROOT}/scripts/install.sh
EOF

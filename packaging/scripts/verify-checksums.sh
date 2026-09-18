#!/usr/bin/env bash
# 校验发布包内所有文件的 SHA-256（与 SHA256SUMS.txt 比对）
#
# 用法：
#   bash scripts/verify-checksums.sh                 # 全量校验
#   bash scripts/verify-checksums.sh --ignore-missing # 只校验存在的文件（分平台包用）
#
# 退出码：0 = 全部一致；1 = 有不一致或缺失；2 = 用法/环境问题
set -uo pipefail

PKG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PKG_ROOT}"

IGNORE_MISSING=0
[ "${1:-}" = "--ignore-missing" ] && IGNORE_MISSING=1

SUMS="SHA256SUMS.txt"
if [ ! -f "${SUMS}" ]; then
  echo "找不到 ${SUMS}（应在发布包根目录）" >&2
  exit 2
fi

# 选一个可用的 sha256 工具
if command -v sha256sum >/dev/null 2>&1; then
  HASH() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  HASH() { shasum -a 256 "$1" | awk '{print $1}'; }
elif command -v openssl >/dev/null 2>&1; then
  HASH() { openssl dgst -sha256 "$1" | awk '{print $NF}'; }
else
  echo "没有可用的 sha256 工具（sha256sum / shasum / openssl 任一即可）" >&2
  exit 2
fi

OK=0; FAILED=0; MISSING=0; TOTAL=0
FAILED_LIST=""

while IFS= read -r line || [ -n "${line}" ]; do
  # 跳过空行与注释
  [ -z "${line}" ] && continue
  case "${line}" in \#*) continue ;; esac

  # 格式：<摘要><空白><路径>（sha256sum 标准输出用两个空格，BSD 用一个空格+*标记）
  # 末尾 tr -d '\r' 是必需的：清单文件可能在 Windows 上被编辑过而带 CRLF，
  # 残留的 \r 会让 [ -f "$FILE" ] 永远为假（表现为"所有文件 MISSING"）。
  EXPECT="$(printf '%s' "${line}" | awk '{print $1}' | tr -d '\r')"
  FILE="$(printf '%s' "${line}" | sed -E 's/^[^ ]+[ ]+[*]?//' | tr -d '\r')"
  [ -z "${EXPECT}" ] && continue

  TOTAL=$((TOTAL + 1))
  if [ ! -f "${FILE}" ]; then
    MISSING=$((MISSING + 1))
    if [ "${IGNORE_MISSING}" -eq 0 ]; then
      echo "MISSING  ${FILE}"
      FAILED_LIST="${FAILED_LIST}"$'\n'"  MISSING  ${FILE}"
    fi
    continue
  fi

  GOT="$(HASH "${FILE}")"
  if [ "${GOT}" = "${EXPECT}" ]; then
    OK=$((OK + 1))
  else
    FAILED=$((FAILED + 1))
    echo "FAILED   ${FILE}"
    echo "         期望 ${EXPECT}"
    echo "         实际 ${GOT}"
    FAILED_LIST="${FAILED_LIST}"$'\n'"  FAILED   ${FILE}"
  fi
done < "${SUMS}"

echo
echo "================ 校验结果 ================"
echo "总计 ${TOTAL} 项：OK ${OK} / FAILED ${FAILED} / MISSING ${MISSING}"
echo "包目录：${PKG_ROOT}"

if [ "${FAILED}" -eq 0 ] && { [ "${MISSING}" -eq 0 ] || [ "${IGNORE_MISSING}" -eq 1 ]; }; then
  echo "✅ 校验通过：包内文件完整、未被篡改"
  exit 0
fi

echo "❌ 校验失败，请勿使用本包（重新下载或联系分发方）"
[ -n "${FAILED_LIST}" ] && printf '\n问题文件：%s\n' "${FAILED_LIST}"
exit 1

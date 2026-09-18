#!/usr/bin/env bash
# req-guard 安装后冒烟自检
#
# 在临时目录里跑一遍完整链路，验证"放行"与"拦截"两条路径都可用：
#   1) 版本自证            -V
#   2) init 接入门禁
#   3) create 建需求
#   4) 未批准 → check 必须 **拦截**（退出码 1）   ← 拦不住就是装了个假门禁
#   5) 三段 approve 后 → check 必须 **放行**（退出码 0）
#   6) 加阻塞评论 → check 必须 **拦截**
#   7) resolve 后 → check **放行**
#   8) install --verify → 资产/hook/pre-commit 全部就位
#
# 用法：
#   bash scripts/selfcheck.sh                 # 用 PATH 里的 req-guard
#   bash scripts/selfcheck.sh --bin ./req-guard
#   bash scripts/selfcheck.sh --keep          # 保留临时目录（排查用）
set -uo pipefail

BIN=""
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --bin)  BIN="$2"; shift 2 ;;
    --keep) KEEP=1;   shift ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

# ---- 找二进制 ----
if [ -z "${BIN}" ]; then
  if [ -n "${REQ_GUARD_BIN:-}" ]; then
    BIN="${REQ_GUARD_BIN}"
  elif command -v req-guard >/dev/null 2>&1; then
    BIN="req-guard"
  elif command -v req-guard.exe >/dev/null 2>&1; then
    BIN="req-guard.exe"
  else
    echo "找不到 req-guard：请先安装，或用 --bin <路径> 指定（也可用环境变量 REQ_GUARD_BIN）" >&2
    exit 2
  fi
fi
if ! "${BIN}" -V >/dev/null 2>&1; then
  echo "二进制无法执行：${BIN}" >&2
  exit 2
fi

PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS + 1)); echo "  ✅ $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  ❌ $1"; }
warn() { SKIP=$((SKIP + 1)); echo "  ⚠️  $1"; }
step() { echo; echo "── $1"; }

TMP="$(mktemp -d 2>/dev/null || echo "/tmp/req-guard-selfcheck-$$")"
mkdir -p "${TMP}"
cleanup() {
  if [ "${KEEP}" -eq 0 ]; then rm -rf "${TMP}" 2>/dev/null || true; else echo "（保留临时目录：${TMP}）"; fi
}
trap cleanup EXIT

step "1. 版本自证"
VER="$("${BIN}" -V 2>&1 | head -1)"
echo "     ${VER}"
[ -n "${VER}" ] && ok "-V 输出版本号" || bad "-V 无输出"

step "2. 准备临时 git 仓库：${TMP}"
if command -v git >/dev/null 2>&1 && (cd "${TMP}" && git init -q . 2>/dev/null) && [ -d "${TMP}/.git" ]; then
  ok "git init"
else
  # 未装 git 时，pre-commit 注入需要 .git 存在；建最小桩让「注入 + --verify」链路仍可验证。
  # 注意：这只证明注入链路通，真实仓库请自己 git init 后再执行 req-guard install。
  mkdir -p "${TMP}/.git/hooks"
  warn "未找到可用的 git，改用最小 .git 桩验证注入链路"
fi

step "3. init 接入门禁"
if "${BIN}" init -p "${TMP}" >/dev/null 2>&1; then ok "init 成功"; else bad "init 失败"; fi

step "4. create 建需求"
OUT="$("${BIN}" create -p "${TMP}" -t "自检需求" 2>&1)"
echo "     $(printf '%s' "${OUT}" | head -1)"
case "${OUT}" in *REQ-001*) ok "创建 REQ-001" ;; *) bad "未创建 REQ-001：${OUT}" ;; esac

step "5. 未批准时应拦截（期望退出码 1）"
"${BIN}" check -p "${TMP}" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 1 ] && ok "check 拦截（exit 1）" || bad "check 应拦截却返回 ${RC}"

step "6. 三段全部批准"
for s in decomposition solution testplan; do
  if "${BIN}" approve -p "${TMP}" REQ-001 --step "${s}" --reviewer 自检 >/dev/null 2>&1; then
    ok "approve ${s}"
  else
    bad "approve ${s} 失败"
  fi
done

step "7. 全批后应放行（期望退出码 0）"
"${BIN}" check -p "${TMP}" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 0 ] && ok "check 放行（exit 0）" || bad "check 应放行却返回 ${RC}"

step "8. 阻塞评论应重新拦截"
if "${BIN}" comment -p "${TMP}" REQ-001 --step solution --author 自检 --blocking --text "自检：阻塞评论" >/dev/null 2>&1; then
  ok "添加阻塞评论"
else
  bad "添加阻塞评论失败"
fi
"${BIN}" check -p "${TMP}" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 1 ] && ok "阻塞评论生效（exit 1）" || bad "阻塞评论未拦截（exit ${RC}）"

step "9. resolve 后恢复放行"
if "${BIN}" resolve -p "${TMP}" REQ-001 C001 --author 自检 >/dev/null 2>&1; then ok "resolve C001"; else bad "resolve 失败"; fi
"${BIN}" check -p "${TMP}" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 0 ] && ok "恢复放行（exit 0）" || bad "resolve 后应放行却返回 ${RC}"

step "10. install --verify 校验就位"
if "${BIN}" install -p "${TMP}" --verify >/dev/null 2>&1; then ok "资产/hook/pre-commit 就位"; else bad "存在缺口，请按 docs/常见问题.md 排查"; fi

echo
echo "================ 自检结果 ================"
echo "PASS ${PASS} / FAIL ${FAIL} / SKIP ${SKIP}"
if [ "${FAIL}" -eq 0 ]; then
  echo "✅ req-guard 安装正常：拦截与放行两条路径均可用"
  exit 0
fi
echo "❌ 有 ${FAIL} 项失败，请对照 docs/常见问题.md 排查"
exit 1

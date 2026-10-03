#!/usr/bin/env sh
# req-guard — 变更范围契约校验（pre-commit 专用）
#
# 判定「实际改动 ⊆ GATE:TOUCH 声明的并集」（core/src/touch.rs）。退出码：0 放行 / 1 拦截。
set -u

PROJECT_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "[req-guard] 非 git 仓库，跳过变更范围校验（无可比对对象）" >&2
  exit 0
}
cd "$PROJECT_ROOT" || exit 1

# fail-closed：判定在 core，脚本只取参。二进制不在 PATH 就拦，不得静默放行 ——
# "看起来在拦、其实没拦"是本工具最危险的失效模式（见《AI工具合规保证规范.md》§4.2）。
if ! command -v req-guard >/dev/null 2>&1; then
  echo "[req-guard] ✗ 变更范围无法校验：req-guard 不在 PATH，提交已被阻止。" >&2
  echo "          请把 req-guard 加入 PATH 后重试；确需跳过本次：git commit --no-verify" >&2
  exit 1
fi

# 已暂存文件集交给 core 判定（换行分隔）。用 -z 拿原始路径，避免带空格/非 ASCII
# 的路径被 git 加引号后与真实路径对不上。
HOOK_STAGED_FILES="$(git diff --cached --name-only --diff-filter=ACMRD -z 2>/dev/null | tr '\0' '\n')"
export HOOK_STAGED_FILES
req-guard touch-check || exit 1

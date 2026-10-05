#!/usr/bin/env sh
# req-guard — AI 需求门禁硬拦截脚本（POSIX）
#
# 用法：
#   1) AI 工具 PreToolUse hook：拦截 Write/Edit 等写操作（AI 写不了文件）
#   2) git pre-commit：兜底，未过审不得提交
# 退出码：0 放行；1 拦截（stderr 会被 AI 工具回显，从而阻止其继续写代码）
set -u

REQ_DIR=".gates/requirements"
AUDIT_LOG=".gates/audit/gate-audit.log"

log() {
  mkdir -p "$(dirname "$AUDIT_LOG")" 2>/dev/null || true
  printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo '-')" "$1" >>"$AUDIT_LOG" 2>/dev/null || true
}

# ---------- 0) AI 禁止直接修改评论文件（★ 必须先于应急绕过：逃逸阀不覆盖证据完整性） ----------
if [ ! -t 0 ]; then
  STDIN_DATA=$(cat 2>/dev/null || true)
  if [ -n "$STDIN_DATA" ]; then
    # 优先交给 req-guard 用 Rust **真解析** JSON：AI 工具 payload 允许 Unicode 转义
    # （".gates\u002f…comments.md" 与 ".gates/…comments.md" 完全等价），正则匹配不到
    # 会静默放过——那正是本工具最坏的失效：看着在拦，其实没拦。
    if command -v req-guard >/dev/null 2>&1; then
      # 拦截时 req-guard 已把原因打到 stderr（AI 看得见），这里只接退出码
      OUT=$(printf '%s' "$STDIN_DATA" | req-guard hook-check) || exit 1
      case "$OUT" in
        # 清单正文（状态行未改动）：放行本次写，不再要求三段已批准
        *REQ_GUARD_ALLOW_DOC=1*) exit 0 ;;
      esac
    else
      # 兜底：二进制不在 PATH（受限环境）时退回正则粗判。
      # 只保留证据保护，**不**放行清单正文（无法校验状态行 → 宁可维持 fail-closed）
      FP=$(printf '%s' "$STDIN_DATA" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
      case "$FP" in
        *.comments.md)
          log "BLOCK-AI-WRITE-COMMENTS $FP"
          echo "[req-guard] ⛔ 拦截：审核评论文件禁止 AI 直接修改（嫌疑人不得修改证据）。" >&2
          echo "            AI 回复请用：req-guard comment <需求ID> --author ai --reply C001 --text \"...\"" >&2
          exit 1
          ;;
      esac
    fi
  fi
fi

# ---------- 1) 裁决（判定在 core；脚本只取参与渲染） ----------
#
# 为什么第 1 段之后的一切都被删掉了：三段检查、阻塞评论、评论摘要、内容冻结，
# 现在全部由 `req-guard check` 判定（core/src/resolve.rs）。本脚本**不含任何裁决分支** ——
# ① 判定散落到第二处的那一刻起，它就与 core 版本漂移，而门禁最坏的失效不是"报错"
# 而是"看着在拦、其实没拦"；② 任何加进本脚本的判定都必须在 HOOK_PS1 里逐行镜像一遍，
# 那是纯负债（本次改造就是为了消掉这份镜像）。
#
# 三个上下文各取一种变更集，**不得混用**（两个变更集混判必然产生无法解释的裁决）：
#   有 payload（AI PreToolUse）→ --stdin：路径由 Rust 真解析 JSON，不在 sh 里 sed 抠
#   无 payload（pre-commit / 人工）→ --staged：已暂存文件集
if ! command -v req-guard >/dev/null 2>&1; then
  log "BLOCK no-binary"
  echo "[req-guard] ⛔ 拦截：无法裁决（req-guard 不在 PATH），本次写/提交已被阻止。" >&2
  echo "          判定在 core，缺二进制即无从判定 —— fail-closed，不猜。" >&2
  echo "          请把 req-guard 加入 PATH 后重试（安装见 req-guard install）。" >&2
  echo "          确需本次放行：git commit --no-verify / req-guard bypass --reason \"<原因>\"（应急绕过，须人类凭据）" >&2
  exit 1
fi
if [ -n "${STDIN_DATA:-}" ]; then
  printf '%s' "$STDIN_DATA" | req-guard check --stdin || exit 1
else
  req-guard check --staged || exit 1
fi

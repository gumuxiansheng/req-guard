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
BYPASS_FILE=".gates/.bypass"

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

# ---------- 1) 应急绕过窗口（有痕、有时效） ----------
if [ -f "$BYPASS_FILE" ]; then
  EXP=$(sed -n 's/.*expires_epoch=\([0-9]*\).*/\1/p' "$BYPASS_FILE" 2>/dev/null | head -1)
  case "$EXP" in ''|*[!0-9]*) EXP="";; esac
  NOW=$(date '+%s' 2>/dev/null || echo 0)
  case "$NOW" in ''|*[!0-9]*) NOW=0;; esac
  if [ -n "$EXP" ] && [ "$NOW" -lt "$EXP" ]; then
    log "BYPASS-HIT expires_epoch=$EXP"
    echo "[req-guard] 警告：命中应急绕过窗口，本次放行（已记审计日志）" >&2
    # 机器可读标记：供 req-guard check 判定"本次放行靠绕过"（勿改，与 BYPASS_MARKER 对应）
    echo "REQ_GUARD_BYPASS=1"
    exit 0
  fi
fi

# ---------- 2) 定位当前活跃需求（跳过已归档 done 的） ----------
ACTIVE=""
if [ -d "$REQ_DIR" ]; then
  for F in $(ls "$REQ_DIR" 2>/dev/null | grep '\.md$' | grep -v '\.comments\.md$' | sort -r); do
    ST=$(sed -n 's/.*GATE:HEAD .*status=\([a-z_]*\).*/\1/p' "$REQ_DIR/$F" 2>/dev/null | head -1)
    if [ "$ST" != "done" ]; then
      ACTIVE="$F"
      break
    fi
  done
fi

if [ -z "$ACTIVE" ]; then
  log "BLOCK no-requirement"
  echo "[req-guard] ⛔ 拦截：未找到待开发的需求清单。" >&2
  echo "          AI 在编写代码前，必须先创建并走完三段清单审核：" >&2
  echo "            req-guard create -t \"<需求标题>\"" >&2
  exit 1
fi

# ---------- 3) 三段步骤必须全部 approved ----------
FAILED=""
for STEP in decomposition solution testplan; do
  LINE=$(grep 'GATE:STEP' "$REQ_DIR/$ACTIVE" 2>/dev/null | grep "name=$STEP " | head -1)
  ST=$(printf '%s' "$LINE" | sed -n 's/.*status=\([a-z_]*\).*/\1/p')
  if [ "$ST" != "approved" ]; then
    FAILED="$FAILED\n    - $STEP 未通过审核（当前: ${ST:-pending}）"
  fi
done

if [ -n "$FAILED" ]; then
  log "BLOCK $ACTIVE"
  printf "[req-guard] ⛔ 拦截：需求 %s 尚未通过审核，AI 不得编写/修改源码。\n" "$ACTIVE" >&2
  printf "          未完成步骤：%b\n" "$FAILED" >&2
  echo "          请补齐清单后由审核人执行：" >&2
  echo "            req-guard approve <需求ID> --step <步骤> --reviewer <姓名>" >&2
  echo "          步骤顺序：decomposition(需求分解) → solution(技术方案) → testplan(测试计划)" >&2
  exit 1
fi

# ---------- 4) 阻塞性评论必须全部 resolved ----------
COMMENTS="$REQ_DIR/${ACTIVE%.md}.comments.md"
if [ -f "$COMMENTS" ] && grep -q 'blocking=true' "$COMMENTS" 2>/dev/null; then
  OPEN=$(grep 'GATE:COMMENT' "$COMMENTS" 2>/dev/null | grep 'blocking=true' | grep 'state=open')
  if [ -n "$OPEN" ]; then
    log "BLOCK-COMMENT $ACTIVE"
    echo "[req-guard] ⛔ 拦截：存在未解决的阻塞性评论，需审核人 resolve 后才可编码" >&2
    exit 1
  fi
fi

# ---------- 5) 评论摘要（每次都提示，保证 AI 必然看到） ----------
if [ -f "$COMMENTS" ]; then
  N=$(grep 'GATE:COMMENT' "$COMMENTS" 2>/dev/null | grep -c 'state=open')
  case "$N" in ''|*[!0-9]*) N=0;; esac
  if [ "$N" -gt 0 ]; then
    echo "[req-guard] 提示：有 ${N} 条 open 评论，执行 req-guard comments ${ACTIVE} 查看" >&2
  fi
fi

log "PASS $ACTIVE"
exit 0

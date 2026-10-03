#!/usr/bin/env sh
# req-guard deny 包装：把 check.sh 的 exit 1（拦截）转成工具能识别的拒绝（exit 2）。
# 仅 Codex/Cursor 需要：它们把非 exit 0 视为 fail-open（继续执行）。
# 用法（作为 AI 工具 PreToolUse hook 的 command）：sh .gates/hooks/req-guard-deny.sh
set -u

sh .gates/hooks/req-guard-check.sh
RC=$?
if [ "$RC" -ne 0 ]; then
  echo "[req-guard] ⛔ 门禁拦截（原因见上方；以 exit 2 交付，编码为工具可识别的拒绝）" >&2
  exit 2
fi
exit 0

# req-guard deny 包装（Windows PowerShell）
# 把 check.ps1 的 exit 1（拦截）转成 Codex/Cursor 能识别的拒绝（exit 2）
& (Join-Path $PSScriptRoot 'req-guard-check.ps1')
if ($LASTEXITCODE -ne 0) {
  Write-Error "[req-guard] 门禁拦截（原因见上方；以 exit 2 交付，编码为工具可识别的拒绝）"
  exit 2
}
exit 0

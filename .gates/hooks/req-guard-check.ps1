# req-guard — AI 需求门禁硬拦截脚本（Windows PowerShell）
# 退出码：0 放行；1 拦截
$ErrorActionPreference = 'Continue'

$REQ_DIR = ".gates/requirements"
$AUDIT_LOG = ".gates/audit/gate-audit.log"

function Write-GateAudit([string]$msg) {
  $dir = Split-Path -Parent $AUDIT_LOG
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
  $line = "{0} {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $msg
  Add-Content -Path $AUDIT_LOG -Value $line -Encoding UTF8
}

# ---------- 0) AI 禁止直接修改评论文件（★ 必须先于应急绕过） ----------
if (-not [Console]::IsInputRedirected) {
  $stdinData = ''
} else {
  $stdinData = [Console]::In.ReadToEnd()
}
if ($stdinData.Trim()) {
  # 优先交给 req-guard 用 Rust **真解析**（正则会被 Unicode 转义绕过，详见 HOOK_SH 第 0 段）
  if (Get-Command req-guard -ErrorAction SilentlyContinue) {
    $out = $stdinData | req-guard hook-check
    if ($LASTEXITCODE -ne 0) { exit 1 }
    # 清单正文（状态行未改动）：放行本次写，不再要求三段已批准
    if ($out -match 'REQ_GUARD_ALLOW_DOC=1') { exit 0 }
  } else {
    # 兜底：只保留证据保护，**不**放行清单正文（无法校验状态行 → fail-closed）
    $m0 = [regex]::Match($stdinData, '"file_path"\s*:\s*"([^"]+)"')
    if ($m0.Success -and $m0.Groups[1].Value -like '*.comments.md') {
      Write-GateAudit "BLOCK-AI-WRITE-COMMENTS $($m0.Groups[1].Value)"
      Write-Error "[req-guard] 拦截：审核评论文件禁止 AI 直接修改（嫌疑人不得修改证据）。AI 回复请用：req-guard comment <需求ID> --author ai --reply C001 --text ""..."""
      exit 1
    }
  }
}

# ---------- 1) 裁决（判定在 core；脚本只取参与渲染） ----------
# 与 HOOK_SH 同构：本地 sh 是唯一真实部署面，本文件是它的逐行镜像（Windows 资产）。
# 镜像负债之所以只剩这一处，正是因为第 1 段之后的一切裁决都已下沉到 core。
if (-not (Get-Command req-guard -ErrorAction SilentlyContinue)) {
    Write-GateAudit "BLOCK no-binary"
    Write-Error "[req-guard] ⛔ 拦截：无法裁决（req-guard 不在 PATH），本次写/提交已被阻止。判定在 core，缺二进制即无从判定 —— fail-closed，不猜。请把 req-guard 加入 PATH 后重试（安装见 req-guard install）。确需本次放行：git commit --no-verify / .gates/.bypass 应急窗口。"
    exit 1
}
if ($stdinData.Trim()) {
    # 有 payload（AI PreToolUse）→ --stdin：路径由 Rust 真解析 JSON
    $stdinData | req-guard check --stdin | Out-Host
    if ($LASTEXITCODE -ne 0) { exit 1 }
} else {
    # 无 payload（pre-commit / 人工）→ --staged
    req-guard check --staged | Out-Host
    if ($LASTEXITCODE -ne 0) { exit 1 }
}
exit 0

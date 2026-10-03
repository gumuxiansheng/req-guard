# req-guard — AI 需求门禁硬拦截脚本（Windows PowerShell）
# 退出码：0 放行；1 拦截
$ErrorActionPreference = 'Continue'

$REQ_DIR = ".gates/requirements"
$AUDIT_LOG = ".gates/audit/gate-audit.log"
$BYPASS_FILE = ".gates/.bypass"

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

# ---------- 1) 应急绕过窗口 ----------
if (Test-Path $BYPASS_FILE) {
  $txt = [string](Get-Content $BYPASS_FILE -Raw -ErrorAction SilentlyContinue)
  $m = [regex]::Match($txt, 'expires_epoch=(\d+)')
  if ($m.Success) {
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    if ($now -lt [int64]$m.Groups[1].Value) {
      Write-GateAudit "BYPASS-HIT expires_epoch=$($m.Groups[1].Value)"
      Write-Output "[req-guard] 警告：命中应急绕过窗口，本次放行（已记审计日志）"
      # 机器可读标记：供 req-guard check 判定"本次放行靠绕过"（勿改，与 BYPASS_MARKER 对应）
      Write-Output "REQ_GUARD_BYPASS=1"
      exit 0
    }
  }
}

# ---------- 2) 定位当前活跃需求 ----------
$active = $null
if (Test-Path $REQ_DIR) {
  $files = Get-ChildItem -Path $REQ_DIR -Filter *.md -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notlike '*.comments.md' } |
    Sort-Object Name -Descending
  foreach ($f in $files) {
    $head = Get-Content $f.FullName -TotalCount 20 -ErrorAction SilentlyContinue | Where-Object { $_ -match 'GATE:HEAD' } | Select-Object -First 1
    $st = ''
    if ($head -match 'status=([a-z_]+)') { $st = $Matches[1] }
    if ($st -ne 'done') { $active = $f; break }
  }
}

if ($null -eq $active) {
  Write-GateAudit "BLOCK no-requirement"
  Write-Error "[req-guard] 拦截：未找到待开发的需求清单。请先执行 req-guard create -t ""<需求标题>"""
  exit 1
}

# ---------- 3) 三段步骤必须全部 approved ----------
$failed = @()
foreach ($step in @('decomposition', 'solution', 'testplan')) {
  $l = Get-Content $active.FullName -ErrorAction SilentlyContinue | Where-Object { $_ -match 'GATE:STEP' -and $_ -match "name=$step " } | Select-Object -First 1
  $st = ''
  if ($l -match 'status=([a-z_]+)') { $st = $Matches[1] }
  if ($st -ne 'approved') {
    if (-not $st) { $st = 'pending' }
    $failed += "$step 未通过审核（当前: $st）"
  }
}

if ($failed.Count -gt 0) {
  Write-GateAudit "BLOCK $($active.Name)"
  Write-Error "[req-guard] 拦截：需求 $($active.Name) 尚未通过审核，AI 不得编写/修改源码。未完成步骤： $($failed -join '; ')"
  exit 1
}

# ---------- 4) 阻塞性评论必须全部 resolved ----------
$commentsFile = Join-Path $REQ_DIR ($active.BaseName + ".comments.md")
if (Test-Path $commentsFile) {
  $blockingOpen = Get-Content $commentsFile -ErrorAction SilentlyContinue |
    Where-Object { $_ -match 'GATE:COMMENT' -and $_ -match 'blocking=true' -and $_ -match 'state=open' }
  if ($blockingOpen) {
    Write-GateAudit "BLOCK-COMMENT $($active.Name)"
    Write-Error "[req-guard] 拦截：存在未解决的阻塞性评论，需审核人 resolve 后才可编码"
    exit 1
  }
  $openN = @(Get-Content $commentsFile -ErrorAction SilentlyContinue |
    Where-Object { $_ -match 'GATE:COMMENT' -and $_ -match 'state=open' }).Count
  if ($openN -gt 0) {
    Write-Output "[req-guard] 提示：有 ${openN} 条 open 评论，执行 req-guard comments $($active.Name) 查看"
  }
}

Write-GateAudit "PASS $($active.Name)"
exit 0

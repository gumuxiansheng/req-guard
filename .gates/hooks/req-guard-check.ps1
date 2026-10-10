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

# ---------- 0) 定位 req-guard 二进制（REQ-022；与 HOOK_SH 第 0 段逐条镜像） ----------
# 变量优先于 PATH：PATH 是**继承**来的，GUI 应用与终端拿到的常常不是同一份，
# 只认 PATH 时「明明装了却报找不到」无从自证。定位失败**不在本段拦截** ——
# 第 1 段要靠「$rgBin 为空」走正则兜底，保住「证据被篡改」那条精确归因。
# 刻意**不**往 PATH 追加任何目录：那等于允许替换裁决者（理由见 HOOK_SH 第 0 段）。
# Windows 没有 POSIX 执行位，故只判「是文件」（`Leaf`）—— 与 POSIX 侧唯一的语义差。
$rgBin = ''
$rgBad = ''
if ($env:REQ_GUARD_BIN) {
  if (Test-Path -LiteralPath $env:REQ_GUARD_BIN -PathType Leaf) {
    $rgBin = $env:REQ_GUARD_BIN
  } else {
    $rgBad = $env:REQ_GUARD_BIN
  }
} elseif (Get-Command req-guard -ErrorAction SilentlyContinue) {
  $rgBin = 'req-guard'
}

# ---------- 1) AI 禁止直接修改评论文件（★ 必须先于应急绕过） ----------
if (-not [Console]::IsInputRedirected) {
  $stdinData = ''
} else {
  $stdinData = [Console]::In.ReadToEnd()
}
if ($stdinData.Trim()) {
  # 优先交给 req-guard 用 Rust **真解析**（正则会被 Unicode 转义绕过，详见 HOOK_SH 第 1 段）
  if ($rgBin) {
    $out = $stdinData | & $rgBin hook-check
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

# ---------- 2) 裁决（判定在 core；脚本只取参与渲染） ----------
# 与 HOOK_SH 同构：本地 sh 是唯一真实部署面，本文件是它的逐行镜像（Windows 资产）。
# 镜像负债之所以只剩这一处，正是因为第 2 段之外的一切裁决都已下沉到 core。
if ($rgBin) {
    if ($stdinData.Trim()) {
        # 有 payload（AI PreToolUse）→ --stdin：路径由 Rust 真解析 JSON
        $stdinData | & $rgBin check --stdin | Out-Host
        if ($LASTEXITCODE -ne 0) { exit 1 }
    } else {
        # 无 payload（pre-commit / 人工）→ --staged
        & $rgBin check --staged | Out-Host
        if ($LASTEXITCODE -ne 0) { exit 1 }
    }
} elseif ($rgBad) {
    # 变量设了却不可用 → **不回退 PATH**（理由见 HOOK_SH 第 2 段）
    Write-GateAudit "BLOCK no-binary-badenv"
    Write-Error "[req-guard] ⛔ 拦截：REQ_GUARD_BIN 指向的文件不可用（$rgBad），本次写/提交已被阻止。请确认该路径存在且是文件；清空该环境变量即回到 PATH 查找。"
    exit 1
} else {
    Write-GateAudit "BLOCK no-binary"
    Write-Error "[req-guard] ⛔ 拦截：无法裁决（req-guard 不在 PATH），本次写/提交已被阻止。判定在 core，缺二进制即无从判定 —— fail-closed，不猜。请把 req-guard 加入 PATH 后重试（安装见 req-guard install），亦可用 REQ_GUARD_BIN=<绝对路径> 显式指定（GUI 应用与终端的 PATH 常常不是同一份）。确需本次放行：git commit --no-verify / req-guard bypass --reason \"<原因>\"（应急绕过，须人类凭据）。"
    exit 1
}
exit 0

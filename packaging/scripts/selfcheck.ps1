#Requires -Version 5.1
<#
.SYNOPSIS
    req-guard 安装后冒烟自检（Windows PowerShell）

.DESCRIPTION
    在临时目录里跑一遍完整链路，验证"放行"与"拦截"两条路径都可用：
      1) 版本自证
      2) init 接入门禁
      3) create 建需求
      4) 未批准 → check 必须拦截（退出码 1）
      5) 三段 approve 后 → check 必须放行（退出码 0）
      6) 加阻塞评论 → 必须重新拦截
      7) resolve 后 → 恢复放行
      8) install --verify → 资产/hook/pre-commit 全部就位

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\selfcheck.ps1
    powershell -ExecutionPolicy Bypass -File scripts\selfcheck.ps1 -Bin C:\tools\req-guard\req-guard.exe
#>
[CmdletBinding()]
param(
    [string]$Bin = '',
    [switch]$Keep
)

$ErrorActionPreference = 'Continue'

# ---- 找二进制 ----
if (-not $Bin) {
    if ($env:REQ_GUARD_BIN) { $Bin = $env:REQ_GUARD_BIN }
    else {
        $cmd = Get-Command req-guard -ErrorAction SilentlyContinue
        if ($cmd) { $Bin = $cmd.Source }
    }
}
if (-not $Bin -or -not (Test-Path $Bin)) {
    Write-Host '找不到 req-guard：请先安装，或用 -Bin <路径> 指定（也可用环境变量 REQ_GUARD_BIN）' -ForegroundColor Red
    exit 2
}

$script:Pass = 0
$script:Fail = 0
$script:Skip = 0
function Ok   { param($m) $script:Pass++; Write-Host "  ✅ $m" -ForegroundColor Green }
function Bad  { param($m) $script:Fail++; Write-Host "  ❌ $m" -ForegroundColor Red }
function Warn { param($m) $script:Skip++; Write-Host "  ⚠️  $m" -ForegroundColor Yellow }
function Step { param($m) Write-Host ''; Write-Host "── $m" }

$tmp = Join-Path $env:TEMP ("req-guard-selfcheck-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

function Invoke-Guard {
    param([string[]]$GuardArgs)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Bin
    # ArgumentList 是 Collection[string]，没有 AddRange（只有 List[T] 才有）——必须逐个 Add
    foreach ($a in $GuardArgs) { $psi.ArgumentList.Add($a) }
    $psi.WorkingDirectory = $tmp
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEnd()
    $err = $p.StandardError.ReadToEnd()   # 必须读走 stderr，否则缓冲区满会卡死
    $p.WaitForExit()
    return @{ Code = $p.ExitCode; Out = $out; Err = $err }
}

Step '1. 版本自证'
$r = Invoke-Guard @('-V')
Write-Host "     $($r.Out.Trim())"
if ($r.Out.Trim()) { Ok '-V 输出版本号' } else { Bad '-V 无输出' }

Step "2. 准备临时目录：$tmp"
# git 不一定在 PATH（受限会话常见）；找不到就跳过——pre-commit 层不参与，其余仍可验证
$git = Get-Command git -ErrorAction SilentlyContinue
if (-not $git) {
    $gitPaths = @('C:\Program Files\Git\bin\git.exe', 'C:\Program Files\Git\cmd\git.exe')
    foreach ($gp in $gitPaths) { if (Test-Path $gp) { $git = Get-Command $gp; break } }
}
$gitInited = $false
if ($git) {
    # 原生命令不能用 "2>$null | Out-Null"（PowerShell 会把它当文档处理），统一用 2>&1 收集。
    # 判定以「结果落盘」为准而不是 $LASTEXITCODE：不同 git 版本/包装脚本的退出码传递并不可靠。
    $gitOut = & $git.Source init -q $tmp 2>&1
    if (Test-Path (Join-Path $tmp '.git')) { Ok 'git init'; $gitInited = $true }
    else { Warn "git 不可用（$gitOut），改用最小 .git 桩" }
} else {
    Warn '未找到 git，改用最小 .git 桩'
}
if (-not $gitInited) {
    # 受限会话 / 未装 git 时，req-guard 的 pre-commit 注入需要 .git 存在；
    # 建一个最小 .git\hooks 桩，让「注入 + --verify」这条链路仍可被完整验证。
    # 注意：这只证明注入链路通，真实仓库请自己 git init 后再跑一次 install。
    New-Item -ItemType Directory -Path (Join-Path $tmp '.git\hooks') -Force | Out-Null
    Warn '本次用最小 .git 桩验证注入链路；真实仓库请先 git init 再执行 req-guard install'
}

Step '3. init 接入门禁'
$r = Invoke-Guard @('init', '-p', $tmp)
if ($r.Code -eq 0) { Ok 'init 成功' } else { Bad "init 失败（exit $($r.Code)）" }

Step '4. create 建需求'
$r = Invoke-Guard @('create', '-t', '自检需求', '-p', $tmp)
Write-Host "     $($r.Out.Split("`n")[0].Trim())"
if ($r.Out -match 'REQ-001') { Ok '创建 REQ-001' } else { Bad '未创建 REQ-001' }

Step '5. 未批准时应拦截（期望退出码 1）'
$r = Invoke-Guard @('check', '-p', $tmp)
if ($r.Code -eq 1) { Ok 'check 拦截（exit 1）' } else { Bad "check 应拦截却返回 $($r.Code)" }

Step '6. 三段全部批准'
foreach ($s in @('decomposition', 'solution', 'testplan')) {
    $r = Invoke-Guard @('approve', 'REQ-001', '--step', $s, '--reviewer', '自检', '-p', $tmp)
    if ($r.Code -eq 0) { Ok "approve $s" } else { Bad "approve $s 失败（exit $($r.Code)）" }
}

Step '7. 全批后应放行（期望退出码 0）'
$r = Invoke-Guard @('check', '-p', $tmp)
if ($r.Code -eq 0) { Ok 'check 放行（exit 0）' } else { Bad "check 应放行却返回 $($r.Code)" }

Step '8. 阻塞评论应重新拦截'
$r = Invoke-Guard @('comment', 'REQ-001', '--step', 'solution', '--author', '自检', '--blocking', '--text', 'selfcheck', '-p', $tmp)
if ($r.Code -eq 0) { Ok '添加阻塞评论' } else { Bad '添加阻塞评论失败' }
$r = Invoke-Guard @('check', '-p', $tmp)
if ($r.Code -eq 1) { Ok '阻塞评论生效（exit 1）' } else { Bad "阻塞评论未拦截（exit $($r.Code)）" }

Step '9. resolve 后恢复放行'
$r = Invoke-Guard @('resolve', 'REQ-001', 'C001', '--author', '自检', '-p', $tmp)
if ($r.Code -eq 0) { Ok 'resolve C001' } else { Bad 'resolve 失败' }
$r = Invoke-Guard @('check', '-p', $tmp)
if ($r.Code -eq 0) { Ok '恢复放行（exit 0）' } else { Bad "resolve 后应放行却返回 $($r.Code)" }

Step '10. install --verify 校验就位'
$r = Invoke-Guard @('install', '--verify', '-p', $tmp)
if ($r.Code -eq 0) { Ok '资产/hook/pre-commit 就位' } else { Bad '存在缺口，请按 docs\常见问题.md 排查' }

if ($Keep) { Write-Host "（保留临时目录：$tmp）" } else { Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue }

Write-Host ''
Write-Host '================ 自检结果 ================'
Write-Host "PASS $($script:Pass) / FAIL $($script:Fail) / SKIP $($script:Skip)"
if ($script:Fail -eq 0) {
    Write-Host '✅ req-guard 安装正常：拦截与放行两条路径均可用' -ForegroundColor Green
    exit 0
}
Write-Host "❌ 有 $($script:Fail) 项失败，请对照 docs\常见问题.md 排查" -ForegroundColor Red
exit 1

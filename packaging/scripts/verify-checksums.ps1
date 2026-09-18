#Requires -Version 5.1
<#
.SYNOPSIS
    校验发布包内所有文件的 SHA-256（与 SHA256SUMS.txt 比对）

.DESCRIPTION
    逐个文件计算 SHA-256 并与清单比对，结尾输出 OK / FAILED / MISSING 计数。
    任一文件不一致或缺失即返回退出码 1。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\verify-checksums.ps1
#>
[CmdletBinding()]
param(
    # 只校验实际存在的文件（分平台包或只解压了部分内容时用）
    [switch]$IgnoreMissing
)

$ErrorActionPreference = 'Stop'
$PkgRoot = Split-Path -Parent $PSScriptRoot
Set-Location $PkgRoot

$sumsFile = Join-Path $PkgRoot 'SHA256SUMS.txt'
if (-not (Test-Path $sumsFile)) {
    Write-Host '找不到 SHA256SUMS.txt（应在发布包根目录）' -ForegroundColor Red
    exit 2
}

$ok = 0; $failed = 0; $missing = 0; $total = 0
$problemList = @()

foreach ($line in (Get-Content -Path $sumsFile -Encoding UTF8)) {
    $line = $line.Trim()
    if (-not $line -or $line.StartsWith('#')) { continue }

    # 格式：<摘要><空白><路径>
    $parts = $line -split '\s+', 2
    if ($parts.Count -lt 2) { continue }
    $expect = $parts[0].ToLower()
    $file = $parts[1].TrimStart('*').Replace('/', [IO.Path]::DirectorySeparatorChar)

    $total++
    $full = Join-Path $PkgRoot $file
    if (-not (Test-Path $full)) {
        $missing++
        if (-not $IgnoreMissing) {
            Write-Host "MISSING  $file" -ForegroundColor Yellow
            $problemList += "MISSING  $file"
        }
        continue
    }

    $got = (Get-FileHash -Algorithm SHA256 -Path $full).Hash.ToLower()
    if ($got -eq $expect) {
        $ok++
    } else {
        $failed++
        Write-Host "FAILED   $file" -ForegroundColor Red
        Write-Host "         期望 $expect"
        Write-Host "         实际 $got"
        $problemList += "FAILED   $file"
    }
}

Write-Host ''
Write-Host '================ 校验结果 ================'
Write-Host "总计 $total 项：OK $ok / FAILED $failed / MISSING $missing"
Write-Host "包目录：$PkgRoot"

if ($failed -eq 0 -and ($missing -eq 0 -or $IgnoreMissing)) {
    Write-Host '✅ 校验通过：包内文件完整、未被篡改' -ForegroundColor Green
    exit 0
}

Write-Host '❌ 校验失败，请勿使用本包（重新下载或联系分发方）' -ForegroundColor Red
if ($problemList.Count -gt 0) {
    Write-Host ''
    Write-Host '问题文件：'
    $problemList | ForEach-Object { "  $_" }
}
exit 1

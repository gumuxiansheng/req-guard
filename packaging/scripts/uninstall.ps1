#Requires -Version 5.1
<#
.SYNOPSIS
    req-guard 卸载脚本（Windows PowerShell）

.DESCRIPTION
    删除安装目录中的 req-guard 二进制，可选从用户级 PATH 移除该目录。
    不会删除项目里的 .gates\（需求清单、评论、审计是你的资产）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1
    powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1 -InstallDir C:\tools\req-guard -PurgePath
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'req-guard\bin'),
    [string]$BinName = '',
    [switch]$All,
    [switch]$PurgePath
)

$ErrorActionPreference = 'Stop'
$PkgRoot = Split-Path -Parent $PSScriptRoot

$targets = @()
if ($All) {
    $targets += Get-ChildItem -Path $InstallDir -Filter 'req-guard*.exe' -File -ErrorAction SilentlyContinue
} elseif ($BinName) {
    $p = Join-Path $InstallDir "$BinName.exe"
    if (Test-Path $p) { $targets += Get-Item $p }
} else {
    $targets += Get-ChildItem -Path $InstallDir -Filter 'req-guard*.exe' -File -ErrorAction SilentlyContinue
}

if ($targets.Count -eq 0) {
    Write-Host "未找到待删除文件（安装目录：$InstallDir）" -ForegroundColor Yellow
} else {
    foreach ($t in $targets) {
        Remove-Item $t.FullName -Force
        Write-Host "已删除：$($t.FullName)"
    }
}

if ($PurgePath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = $userPath -split ';' | Where-Object { $_ -and ($_ -ne $InstallDir) }
    if ($parts.Count -ne (($userPath -split ';' | Where-Object { $_ }).Count)) {
        [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
        Write-Host "已清理 PATH 条目：$InstallDir"
    } else {
        Write-Host "PATH 中未包含 $InstallDir"
    }
}

Write-Host ''
Write-Host '卸载完成。'
Write-Host '  · 项目里的 .gates\（需求清单、评论、审计）未被删除；如需撤掉门禁请手动处理：'
Write-Host '      删除 <项目>\.gates'
Write-Host '      还原 <项目>\.git\hooks\pre-commit'
Write-Host '      移除 AI 工具配置中的 req-guard hook 段'
Write-Host "  · 重新装回来：powershell -File $PkgRoot\scripts\install.ps1"

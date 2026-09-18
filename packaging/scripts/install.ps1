#Requires -Version 5.1
<#
.SYNOPSIS
    req-guard 一键安装脚本（Windows PowerShell）

.DESCRIPTION
    做四件事：
      1) 识别架构，从 bin\windows-<arch> 挑出对应二进制
      2) 复制到安装目录
      3) 校验 `req-guard -V` 与本包 VERSION 一致
      4) 可选：把安装目录写进用户级 PATH、顺手在当前项目 init 门禁

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -AddPath
    powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -Variant ui -InstallDir C:\tools\req-guard
    powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -Init
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'req-guard\bin'),
    [ValidateSet('cli', 'ui', 'gui')]
    [string]$Variant = 'cli',
    [string]$BinName = '',
    [switch]$AddPath,
    [switch]$Init,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# 发布包根目录（本脚本位于 <包根>\scripts\）
$PkgRoot = Split-Path -Parent $PSScriptRoot

# ---- 版本信息 ----
$Version = ''
$versionFile = Join-Path $PkgRoot 'VERSION'
if (Test-Path $versionFile) {
    $m = Select-String -Path $versionFile -Pattern '^version\s*=\s*"?([0-9][0-9A-Za-z.\-]*)"?'
    if ($m) { $Version = $m.Matches[0].Groups[1].Value }
}
if (-not $Version) {
    $Version = (Split-Path -Leaf $PkgRoot) -replace '^req-guard-v', ''
}

# ---- 架构 ----
# $env:PROCESSOR_ARCHITECTURE 在部分宿主（受限会话 / 非交互进程）里取不到值，
# 因此按"环境变量 → 进程/机器级环境变量 → .NET 运行时信息"三级兜底探测。
$arch = $env:PROCESSOR_ARCHITECTURE
if (-not $arch) { $arch = [System.Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE', 'Process') }
if (-not $arch) { $arch = [System.Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE', 'Machine') }
if (-not $arch) {
    try {
        $arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
            'X64'   { 'AMD64' }
            'Arm64' { 'ARM64' }
            default { 'OTHER' }
        }
    } catch { $arch = 'OTHER' }
}
Write-Host "   架构  : $arch"
switch ($arch) {
    'AMD64' { $platform = 'windows-x86_64' }
    'x64'   { $platform = 'windows-x86_64' }
    default {
        Write-Host "不支持的架构：$arch（本包仅提供 windows-x86_64）" -ForegroundColor Red
        Write-Host 'ARM64 / 其它架构请从源码构建：cargo build --release（见 docs\安装指南.md §7）'
        exit 1
    }
}

# ---- 变体 → 文件名 ----
if (-not $BinName) {
    $BinName = switch ($Variant) {
        'cli' { 'req-guard' }
        'ui'  { 'req-guard-ui' }
        'gui' { 'req-guard-gui' }
    }
}
$src = Join-Path (Join-Path $PkgRoot "bin\$platform") "$BinName.exe"
if (-not (Test-Path $src)) {
    Write-Host "找不到二进制：$src" -ForegroundColor Red
    Write-Host '本包提供的文件：'
    Get-ChildItem -Path (Join-Path $PkgRoot 'bin') -Recurse -File | ForEach-Object { "  $($_.FullName.Substring($PkgRoot.Length + 1))" }
    exit 1
}

# ---- 安装目录 ----
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}
$dest = Join-Path $InstallDir "$BinName.exe"
if ((Test-Path $dest) -and -not $Force) {
    Write-Host "已存在：$dest（加 -Force 覆盖）" -ForegroundColor Yellow
    exit 1
}

Copy-Item -Path $src -Destination $dest -Force
Write-Host "✅ 已安装：$dest"
Write-Host "   来源  : bin\$platform\$BinName.exe"
Write-Host "   大小  : $([math]::Round((Get-Item $dest).Length / 1MB, 2)) MB"

# ---- 版本自证 ----
$got = (& $dest -V 2>$null | Select-Object -First 1)
if ($Version -and ($got -notlike "*$Version*")) {
    Write-Host "⚠️  版本不一致：二进制输出「$got」，本包 VERSION 为 $Version" -ForegroundColor Red
    Write-Host '    请确认没有混装其它版本的发布包。'
    exit 1
}
Write-Host "   版本  : $got（与 VERSION 一致）"

# ---- PATH（用户级，不碰系统变量）----
if ($AddPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -split ';' -contains $InstallDir) {
        Write-Host "   PATH  : 已包含 $InstallDir"
    } else {
        $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "   PATH  : 已写入用户级 PATH（重开终端后全局可用）"
    }
}

# ---- 可选：初始化门禁 ----
if ($Init) {
    Write-Host ''
    Write-Host '==> 在当前项目初始化门禁'
    & $dest init
}

Write-Host ''
Write-Host '下一步：'
Write-Host "  1) 确认命令可用：$BinName -V"
Write-Host "  2) 接入项目    ：cd your-project && $BinName init"
Write-Host "  3) 校验就位    ：$BinName install --verify"
Write-Host "  4) 冒烟自检    ：powershell -File scripts\selfcheck.ps1"
Write-Host "  文档：$PkgRoot\docs\安装指南.md"

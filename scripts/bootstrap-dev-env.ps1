# bootstrap-dev-env.ps1
# ============================================================================
# 在 Windows 上自动下载、安装、配置本项目（NemesisBot）的开发环境：
#   MSVC C++ 构建工具 + Rust + Node.js + Go (+ Git，可选)
# 全程走国内源，确保脚本跑完后能直接编译本项目。
#
# 运行方式（任选其一）：
#   1) 右键此文件 → "使用 PowerShell 运行"
#   2) 双击同目录的 bootstrap-dev-env.bat
#   3) powershell -NoProfile -ExecutionPolicy Bypass -File scripts\bootstrap-dev-env.ps1
#
# 说明：
#   - 脚本会自动请求 UAC 提权（装系统级软件需要管理员）。
#   - 幂等：已装的工具会跳过，可安全重复运行。
#   - MSVC 构建工具这步最大最慢（下载 GB 级，可能 10-30 分钟），其余都很快。
#   - 默认按 x64 下载安装包。
# ============================================================================

#Requires -Version 5.1

# ---------- 0. 全局设置 ----------
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false   # PS7+：原生命令不在 Stop 下因 stderr 抛错
$ProgressPreference    = 'SilentlyContinue'   # 关掉 Invoke-WebRequest 进度条，大幅提速
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$WorkDir = Join-Path $env:TEMP 'nemesisbot-dev-env'
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null

function Write-Step($m) { Write-Host "`n==> $m" -ForegroundColor Cyan }
function Write-Ok($m)   { Write-Host "  [OK] $m" -ForegroundColor Green }
function Write-Info($m) { Write-Host "  ..   $m" -ForegroundColor DarkGray }
function Write-Warn2($m){ Write-Host "  [!]  $m" -ForegroundColor Yellow }
function Die($m)        { Write-Host "  [X]  $m" -ForegroundColor Red; Read-Host "`n按回车关闭"; exit 1 }

function Test-Cmd($n) { return [bool](Get-Command $n -ErrorAction SilentlyContinue) }

# 刷新当前会话的 PATH（合并 机器级 + 用户级）
function Refresh-Path {
    $m = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $u = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = ($m + ';' + $u)
}

# ---------- 1. 自提权（UAC）----------
$cur = [Security.Principal.WindowsIdentity]::GetCurrent()
$isAdmin = ([Security.Principal.WindowsPrincipal]$cur).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "需要管理员权限，正在请求提权（请在 UAC 弹窗里点“是”）..." -ForegroundColor Yellow
    Start-Process -FilePath 'powershell.exe' `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"") `
        -Verb RunAs
    exit
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " NemesisBot 开发环境一键安装（国内源）" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ---------- 2. 架构检查 ----------
Write-Step '检查系统架构'
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    Write-Warn2 "检测到 $($env:PROCESSOR_ARCHITECTURE)，本脚本按 x64 下载安装包；ARM64 请手动调整。继续尝试..."
} else {
    Write-Ok 'x64'
}

# ---------- 3. Git（可选，通常已装）----------
Write-Step '检查 Git'
if (Test-Cmd git) {
    Write-Ok "Git 已安装：$(git --version)"
} else {
    if (Test-Cmd winget) {
        Write-Info '未检测到 Git，用 winget 安装...'
        winget install --id Git.Git -e --source winget --accept-package-agreements --accept-source-agreements --silent | Out-Null
        Refresh-Path
        if (Test-Cmd git) { Write-Ok "Git 安装完成：$(git --version)" }
        else { Write-Warn2 'Git 仍未就绪，请手动安装：https://git-scm.com（非必须，但推荐）' }
    } else {
        Write-Warn2 '未检测到 Git 且无 winget，请手动安装：https://git-scm.com（非必须，但推荐）'
    }
}

# ---------- 4. MSVC C++ 构建工具（Rust 链接器必需）----------
Write-Step '检查 MSVC C++ 构建工具（Rust 链接必需）'
# ProgramFiles(x86) 用 $env: 取不到（括号会破坏解析），改用 GetEnvironmentVariable
$pf86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)', 'Process')
if (-not $pf86) { $pf86 = 'C:\Program Files (x86)' }
$vswhere = Join-Path $pf86 'Microsoft Visual Studio\Installer\vswhere.exe'
function Test-Msvc {
    if (-not (Test-Path $vswhere)) { return $null }
    # 不重定向 stderr：PS 5.1 下 2>$null 配合 ErrorActionPreference=Stop 会把 vswhere 当出错抛
    return & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
$vcPath = Test-Msvc
if ($vcPath) {
    Write-Ok "已安装：$vcPath"
} else {
    Write-Warn2 '未检测到 MSVC，开始下载 VS Build Tools（C++ 工作负载）。这是最慢的一步（下载 GB 级，可能 10-30 分钟）...'
    $boot = Join-Path $WorkDir 'vs_BuildTools.exe'
    Write-Info '下载 vs_BuildTools.exe引导程序...'
    Invoke-WebRequest 'https://aka.ms/vs/17/release/vs_BuildTools.exe' -OutFile $boot -UseBasicParsing
    Write-Info '静默安装中（--passive --wait），请耐心等安装器跑完...'
    $vsArgs = @(
        '--passive','--wait','--norestart',
        '--add','Microsoft.VisualStudio.Workload.VCTools',
        '--add','Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
        '--add','Microsoft.VisualStudio.Component.Windows11SDK.22621',
        '--includeRecommended'
    )
    $p = Start-Process -FilePath $boot -Wait -PassThru -ArgumentList $vsArgs
    # 0=成功，3010=成功但需重启，1641=已发起重启
    if ($p.ExitCode -notin @(0, 3010, 1641)) { Die "VS Build Tools 安装失败（退出码 $($p.ExitCode)）" }
    $vcPath = Test-Msvc
    if ($vcPath) { Write-Ok "MSVC 安装完成：$vcPath" }
    else { Write-Warn2 'MSVC 安装后仍未检测到，cargo build 可能失败——请检查 VS Installer' }
}

# ---------- 5. Rust（rustup，走 TUNA 镜像）----------
Write-Step '安装 Rust（rustup，走清华 TUNA 镜像）'
$env:RUSTUP_DIST_SERVER  = 'https://mirrors.tuna.tsinghua.edu.cn/rustup'
$env:RUSTUP_UPDATE_ROOT  = 'https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup'
# 持久化，方便以后 rustup 自更新也走镜像
[Environment]::SetEnvironmentVariable('RUSTUP_DIST_SERVER', $env:RUSTUP_DIST_SERVER, 'User')
[Environment]::SetEnvironmentVariable('RUSTUP_UPDATE_ROOT', $env:RUSTUP_UPDATE_ROOT, 'User')

if (Test-Cmd rustc) {
    Write-Ok "Rust 已安装：$(rustc --version)"
} else {
    $init = Join-Path $WorkDir 'rustup-init.exe'
    Write-Info '下载 rustup-init.exe（win.rustup.rs，小文件）...'
    Invoke-WebRequest 'https://win.rustup.rs/x86_64' -OutFile $init -UseBasicParsing
    Write-Info '安装 Rust stable（MSVC 工具链）...'
    & $init -y --default-toolchain stable --profile default
    Refresh-Path
}
if (-not (Test-Cmd cargo)) { Die 'rustup 装完仍找不到 cargo，请检查 PATH' }
Write-Ok "cargo：$(cargo --version)"

# cargo 国内源（rsproxy 稀疏索引）
$cargoHome = $env:CARGO_HOME; if (-not $cargoHome) { $cargoHome = Join-Path $env:USERPROFILE '.cargo' }
New-Item -ItemType Directory -Force -Path $cargoHome | Out-Null
$cfg = Join-Path $cargoHome 'config.toml'
$cargoConf = @"
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy]
registry = "https://rsproxy.cn/cratesio-index"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[registries.rsproxy]
index = "https://rsproxy.cn/cratesio-index"

[net]
git-fetch-with-cli = true
"@
if (-not (Test-Path $cfg)) {
    # 无 BOM 的 UTF-8 写入（PS 5.1 的 Set-Content -Encoding UTF8 会加 BOM，cargo 读 config.toml 可能受影响）
    [System.IO.File]::WriteAllText($cfg, $cargoConf, (New-Object System.Text.UTF8Encoding($false)))
    Write-Ok "已写入 cargo 镜像配置：$cfg"
} elseif ((Get-Content $cfg -Raw) -match 'rsproxy|crates-io') {
    Write-Ok "cargo 镜像配置已存在：$cfg"
} else {
    Write-Warn2 "$cfg 已存在但无镜像配置，未覆盖（避免破坏你的自定义）。如需走 rsproxy 请手动合并。"
}

# ---------- 6. Node.js（走 npmmirror）----------
Write-Step '安装 Node.js（走 npmmirror 镜像）'
if (Test-Cmd node) {
    Write-Ok "Node 已安装：$(node --version)"
} else {
    $nodeVer = $null
    try {
        $idx = Invoke-RestMethod 'https://nodejs.org/dist/index.json' -UseBasicParsing
        $nodeVer = ($idx | Where-Object { $_.lts } | Select-Object -First 1).version
    } catch { Write-Warn2 '无法获取 Node 版本列表，回退到固定版本' }
    if (-not $nodeVer) { $nodeVer = 'v22.11.0' }
    Write-Info "目标版本（最新 LTS）：$nodeVer"
    $msi = Join-Path $WorkDir "node-$nodeVer-x64.msi"
    $url = "https://cdn.npmmirror.com/binaries/node/$nodeVer/node-$nodeVer-x64.msi"
    Write-Info "下载 $url"
    Invoke-WebRequest $url -OutFile $msi -UseBasicParsing
    Write-Info '静默安装 Node.js...'
    $p = Start-Process msiexec -Wait -PassThru -ArgumentList '/i', $msi, '/qn', '/norestart'
    if ($p.ExitCode -notin @(0, 3010)) { Die "Node.js 安装失败（退出码 $($p.ExitCode)）" }
    Refresh-Path
    Write-Ok 'Node.js 安装完成'
}
if (-not (Test-Cmd node)) { Die 'Node 装完仍找不到 node' }
Write-Ok "node：$(node --version) ；npm：$(npm --version)"
npm config set registry https://registry.npmmirror.com
Write-Ok 'npm registry -> https://registry.npmmirror.com'

# ---------- 7. Go（走 golang.google.cn）----------
Write-Step '安装 Go（走 golang.google.cn 镜像）'
if (Test-Cmd go) {
    Write-Ok "Go 已安装：$(go version)"
} else {
    $goVer = $null
    try {
        $gj = Invoke-RestMethod 'https://golang.google.cn/dl/?mode=json' -UseBasicParsing
        $goVer = ($gj | Where-Object { $_.stable } | Select-Object -First 1).version
    } catch { Write-Warn2 '无法获取 Go 版本列表，回退到固定版本' }
    if (-not $goVer) { $goVer = 'go1.23.4' }
    Write-Info "目标版本（最新稳定版）：$goVer"
    $msi = Join-Path $WorkDir "$goVer.windows-amd64.msi"
    $url = "https://golang.google.cn/dl/$goVer.windows-amd64.msi"
    Write-Info "下载 $url"
    Invoke-WebRequest $url -OutFile $msi -UseBasicParsing
    Write-Info '静默安装 Go...'
    $p = Start-Process msiexec -Wait -PassThru -ArgumentList '/i', $msi, '/qn', '/norestart'
    if ($p.ExitCode -notin @(0, 3010)) { Die "Go 安装失败（退出码 $($p.ExitCode)）" }
    Refresh-Path
    Write-Ok 'Go 安装完成'
}
if (-not (Test-Cmd go)) { Die 'Go 装完仍找不到 go' }
Write-Ok "go：$(go version)"
go env -w GO111MODULE=on
go env -w GOPROXY=https://goproxy.cn,direct
Write-Ok 'GOPROXY -> https://goproxy.cn,direct'

# ---------- 8. 最终工具链验证 ----------
Write-Step '最终工具链验证'
$allOk = $true
foreach ($t in @('rustc', 'cargo', 'node', 'npm', 'go')) {
    if (Test-Cmd $t) { Write-Ok "$t 就绪" } else { Write-Warn2 "$t 缺失"; $allOk = $false }
}
if (Test-Msvc) { Write-Ok 'MSVC 就绪' } else { Write-Warn2 'MSVC 缺失'; $allOk = $false }
if (-not $allOk) { Die '部分工具缺失，请按上面提示排查后重跑（脚本幂等，可重跑）' }

# ---------- 9. 项目编译自检 ----------
Write-Step '验证项目可编译（cargo check，首次会拉依赖，可能数分钟）'
$ProjectRoot = Split-Path -Parent $PSScriptRoot
if (Test-Path (Join-Path $ProjectRoot 'Cargo.toml')) {
    Push-Location $ProjectRoot
    try {
        # cargo 把进度/报错都写 stderr，用 2>&1 捕获。但 PS 5.1 下 Stop + 2>&1 会把第一行
        # stderr 当终止错误抛出，这里临时把 ErrorActionPreference 降级为 Continue。
        $prevEAP = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $out = & cargo check -p nemesisbot 2>&1
        $rc = $LASTEXITCODE
        $ErrorActionPreference = $prevEAP
        $out | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
        if ($rc -eq 0) {
            Write-Ok 'cargo check 通过——开发环境就绪，本项目可正确编译！'
        } else {
            Write-Warn2 "cargo check 报错（退出码 $rc）。环境工具已装好，但项目自检未过——通常是某个依赖/链接问题，请看上方输出。"
        }
    } finally { Pop-Location }
} else {
    Write-Warn2 "未在 $ProjectRoot 找到 Cargo.toml（此脚本不在仓库 scripts/ 目录下？），跳过项目自检。"
}

# ---------- 10. 完成 ----------
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host " 开发环境就绪！" -ForegroundColor Green
Write-Host " 编译本项目（release 全量）："
Write-Host "   scripts\build-windows.bat" -ForegroundColor White
Write-Host "========================================`n" -ForegroundColor Cyan
Write-Host “提示：新装的工具需要新开一个终端，PATH 才会自动生效。” -ForegroundColor DarkGray
Read-Host "按回车关闭本窗口"

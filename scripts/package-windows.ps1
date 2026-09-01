param(
    [ValidateSet("x64", "arm64")]
    [string]$Architecture = "x64",
    [string]$OutDir = "dist/windows",
    [switch]$RequireInstaller,
    [string]$CertificateThumbprint = $env:HARBOR_LIGHT_CERT_THUMBPRINT,
    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows) {
    throw "package-windows.ps1 必须在 Windows 上运行；MSVC 和 Inno Setup 不能在 macOS 上可靠生成。"
}

$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $projectRoot

$version = (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^\"]+)"').Matches[0].Groups[1].Value
$target = if ($Architecture -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$absoluteOut = [System.IO.Path]::GetFullPath((Join-Path $projectRoot $OutDir))
$stage = Join-Path $absoluteOut "stage-$Architecture"
$portableName = "HarborLight-$version-windows-$Architecture.zip"

New-Item -ItemType Directory -Force -Path $absoluteOut | Out-Null
if (Test-Path $stage) {
    Remove-Item -Recurse -Force $stage
}
New-Item -ItemType Directory -Force -Path $stage | Out-Null

rustup target add $target
cargo test
cargo build --release --target $target

$builtExe = Join-Path $projectRoot "target/$target/release/harbor-light.exe"
$stagedExe = Join-Path $stage "HarborLight.exe"
Copy-Item -Force $builtExe $stagedExe
Copy-Item -Force "README.md" (Join-Path $stage "README.md")

function Invoke-CodeSign([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
        Write-Host "未设置 HARBOR_LIGHT_CERT_THUMBPRINT，产物将保持未签名。"
        return
    }
    $signTool = Get-Command "signtool.exe" -ErrorAction SilentlyContinue
    if (-not $signTool) {
        throw "已配置证书指纹，但找不到 signtool.exe（请安装 Windows SDK）。"
    }
    & $signTool.Source sign /sha1 $CertificateThumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 $Path
}

Invoke-CodeSign $stagedExe

$portablePath = Join-Path $absoluteOut $portableName
if (Test-Path $portablePath) {
    Remove-Item -Force $portablePath
}
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $portablePath -CompressionLevel Optimal

$isccCandidates = @(
    (Get-Command "ISCC.exe" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
    (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6/ISCC.exe"),
    (Join-Path $env:ProgramFiles "Inno Setup 6/ISCC.exe")
) | Where-Object { $_ -and (Test-Path $_) }
$iscc = $isccCandidates | Select-Object -First 1

$installerPath = $null
if ($iscc) {
    & $iscc "/DSourceExe=$stagedExe" "/DOutputDir=$absoluteOut" "/DArchitecture=$Architecture" "/DAppVersion=$version" "installer/windows/HarborLight.iss"
    $installerPath = Join-Path $absoluteOut "HarborLight-$version-windows-$Architecture-setup.exe"
    if (-not (Test-Path $installerPath)) {
        throw "Inno Setup 已运行，但没有生成预期文件：$installerPath"
    }
    Invoke-CodeSign $installerPath
} elseif ($RequireInstaller) {
    throw "找不到 Inno Setup 6。请安装后重试，或去掉 -RequireInstaller 只生成便携 ZIP。"
} else {
    Write-Warning "未找到 Inno Setup 6，仅生成便携 ZIP。"
}

$artifacts = @($portablePath)
if ($installerPath) { $artifacts += $installerPath }
foreach ($artifact in $artifacts) {
    $hash = (Get-FileHash -Algorithm SHA256 $artifact).Hash.ToLowerInvariant()
    $name = Split-Path -Leaf $artifact
    Set-Content -Encoding ascii -NoNewline -Path "$artifact.sha256" -Value "$hash  $name`n"
    Write-Host "已生成 $artifact"
}

<#
.SYNOPSIS
  Builds the Windows release binary, a portable zip and an unsigned MSI.

.DESCRIPTION
  One-command local packaging for the Windows targets in .github/workflows/release.yml.
  The MSI needs the WiX v4/v5 command line (dotnet tool install --global wix); when it is
  missing the script installs it. Nothing is code signed, so SmartScreen warns on first run.

.EXAMPLE
  ./scripts/package.ps1
  ./scripts/package.ps1 -Target aarch64-pc-windows-msvc
#>
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$manifest = Get-Content Cargo.toml -Raw
$version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'could not read the version from Cargo.toml' }
# Windows Installer accepts only major.minor.build, so 0.1.0-alpha.1 installs as 0.1.0.
$msiVersion = [regex]::Match($version, '^\d+\.\d+\.\d+').Value
$arch = if ($Target.StartsWith('aarch64')) { 'arm64' } else { 'x64' }
$exe = "target\$Target\release\filetree233.exe"
$asset = "filetree233-windows-$arch"

if (-not $SkipBuild) {
    cargo build --release --locked --features gui --target $Target
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
if (-not (Test-Path $exe)) { throw "expected $exe to exist; build did not produce it" }

New-Item -ItemType Directory -Force -Path dist | Out-Null

# Portable zip: the same binary the release page publishes, plus the docs.
$staging = "dist\$asset"
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory -Force -Path $staging | Out-Null
Copy-Item (Resolve-Path $exe) -Destination $staging
foreach ($document in 'README.md', 'CHANGELOG.md', 'RELEASE_NOTES.md') {
    if (Test-Path $document) { Copy-Item $document -Destination $staging }
}
Compress-Archive -Path "$staging\*" -DestinationPath "dist\$asset.zip" -Force
Remove-Item -Recurse -Force $staging

# MSI through the WiX command line.
$wix = Get-Command wix.exe -ErrorAction SilentlyContinue
if (-not $wix) {
    # A freshly installed dotnet tool is not on this session's PATH, so resolve it by file.
    $local = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
    if (-not (Test-Path $local)) {
        Write-Warning 'wix.exe not found; installing the WiX toolset as a dotnet tool'
        dotnet tool install --global wix
        if ($LASTEXITCODE -ne 0) {
            throw 'install WiX v4 or later (dotnet tool install --global wix) and run this script again'
        }
    }
    if (-not (Test-Path $local)) { throw 'wix.exe is still missing; install WiX v4 or later and retry' }
    $wix = Get-Command $local
}
& $wix.Source build 'packaging\windows\filetree233.wxs' `
    -arch $arch `
    -o "dist\$asset.msi" `
    -d "ProductVersion=$msiVersion" `
    -d "SourcePath=$(Resolve-Path $exe)" `
    -pdbtype none
if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }

Write-Host "Built for $Target (version $version, MSI version $msiVersion):"
foreach ($file in Get-ChildItem dist -File | Where-Object Name -Like "*$asset*") {
    $hash = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    '{0}  {1,8:N0} KiB  {2}' -f $hash, ($file.Length / 1KB), $file.Name
}
Write-Host 'Unsigned: Windows may show SmartScreen; right-click the MSI and choose Run anyway, or install with msiexec /i.'

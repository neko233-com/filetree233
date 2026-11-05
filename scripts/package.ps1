<#
.SYNOPSIS
  Builds the Windows release binary, a portable zip and an unsigned MSI.

.DESCRIPTION
  One-command local packaging for the Windows targets in .github/workflows/release.yml.
  The MSI uses a pinned WiX v5 toolset under %LOCALAPPDATA%\filetree233\wix, installed on
  demand; v6 and later demand acceptance of the Open Source Maintenance Fee EULA.
  Nothing is code signed, so SmartScreen warns on first run.

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
# Windows Installer accepts only major.minor.build, so 0.1.0 installs as 0.1.0.
$msiVersion = [regex]::Match($version, '^\d+\.\d+\.\d+').Value
$arch = if ($Target.StartsWith('aarch64')) { 'arm64' } else { 'x64' }
$exe = "target\$Target\release\filetree233.exe"
$launcher = "target\$Target\release\filetree233-gui.exe"
$asset = "filetree233-windows-$arch"

if (-not $SkipBuild) {
    cargo build --release --locked --features gui --target $Target
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
if (-not (Test-Path $exe)) { throw "expected $exe to exist; build did not produce it" }
if (-not (Test-Path $launcher)) { throw "expected $launcher to exist; build did not produce it" }
if (-not (Test-Path 'packaging\windows\filetree233.ico')) { throw 'packaging\windows\filetree233.ico is missing' }
$info = (Get-Item $exe).VersionInfo
if ($info.ProductName -ne 'filetree233') { throw 'the executable is missing its Windows version resources' }

New-Item -ItemType Directory -Force -Path dist | Out-Null

# Portable zip: the same binary the release page publishes, plus the docs.
$staging = "dist\$asset"
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory -Force -Path $staging | Out-Null
Copy-Item (Resolve-Path $exe) -Destination $staging
Copy-Item (Resolve-Path $launcher) -Destination $staging
foreach ($document in 'README.md', 'CHANGELOG.md', 'RELEASE_NOTES.md') {
    if (Test-Path $document) { Copy-Item $document -Destination $staging }
}
Compress-Archive -Path "$staging\*" -DestinationPath "dist\$asset.zip" -Force
Remove-Item -Recurse -Force $staging

# MSI through the pinned WiX command line. v6 and later refuse to run until the
# Open Source Maintenance Fee EULA is accepted, so builds stay on v5. Keep the tool
# outside the repository: a build cache can restore it without its NuGet assets.
$wixRoot = Join-Path $env:LOCALAPPDATA 'filetree233\wix'
$wix = Join-Path $wixRoot 'wix.exe'
if (-not (Test-Path $wix)) {
    if (Test-Path $wixRoot) { Remove-Item -Recurse -Force $wixRoot }
    Write-Warning 'wix.exe not found; installing the WiX v5 toolset into the user profile'
    dotnet tool install wix --version '5.*' --tool-path $wixRoot
    if ($LASTEXITCODE -ne 0) { throw 'could not install the WiX v5 toolset; install the .NET SDK and retry' }
}
if (-not (Test-Path $wix)) { throw 'wix.exe is still missing after installation; retry the script' }
& $wix build 'packaging\windows\filetree233.wxs' `
    -arch $arch `
    -o "dist\$asset.msi" `
    -d "ProductVersion=$msiVersion" `
    -d "SourcePath=$(Resolve-Path $exe)" `
    -d "GuiSourcePath=$(Resolve-Path $launcher)" `
    -pdbtype none
if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }

Write-Host "Built for $Target (version $version, MSI version $msiVersion):"
foreach ($file in Get-ChildItem dist -File | Where-Object Name -Like "*$asset*") {
    $hash = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    '{0}  {1,8:N0} KiB  {2}' -f $hash, ($file.Length / 1KB), $file.Name
}
Write-Host 'Unsigned: Windows may show SmartScreen; right-click the MSI and choose Run anyway, or install with msiexec /i.'

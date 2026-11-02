param([string]$InstallDir = "$env:LOCALAPPDATA\Programs\filetree233", [string]$Version = "v0.1.0-alpha.1")
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^[A-Za-z0-9._-]+$') { throw 'Invalid version.' }
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$name = switch ($arch) { 'X64' { 'filetree233-windows-x64.exe' } 'Arm64' { 'filetree233-windows-arm64.exe' } default { throw "Unsupported Windows architecture: $arch" } }
$base = "https://github.com/neko233-com/filetree233/releases/download/$Version"
$temp = Join-Path ([IO.Path]::GetTempPath()) ('filetree233-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    $binary = Join-Path $temp $name
    $sums = Join-Path $temp 'SHA256SUMS'
    Invoke-WebRequest "$base/$name" -OutFile $binary
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile $sums
    $line = Get-Content -LiteralPath $sums | Where-Object { $_ -match ('^[0-9a-fA-F]{64}  ' + [regex]::Escape($name) + '$') }
    if (@($line).Count -ne 1) { throw 'Missing or ambiguous checksum.' }
    $expected = ($line -split '\s+')[0]
    if ((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -ne $expected) { throw 'SHA-256 mismatch.' }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $InstallDir 'filetree233.exe') -Force
    Write-Host "Installed to $InstallDir\filetree233.exe"
    Write-Host 'Run the executable, or add this directory to your PATH.'
} finally { Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue }

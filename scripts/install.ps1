param([string]$InstallDir = "$env:LOCALAPPDATA\Programs\filetree233", [string]$Version = "v0.1.0-alpha.1")
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^[A-Za-z0-9._-]+$') { throw 'Invalid version.' }
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$name = switch ($arch) { 'X64' { 'filetree233-windows-x64.exe' } 'Arm64' { 'filetree233-windows-arm64.exe' } default { throw "Unsupported Windows architecture: $arch" } }
$releaseBase = if ($env:FILETREE233_RELEASE_BASE) { $env:FILETREE233_RELEASE_BASE.TrimEnd('/') } else { 'https://github.com/neko233-com/filetree233/releases/download' }
$releaseUri = $null
if (-not [Uri]::TryCreate($releaseBase, [UriKind]::Absolute, [ref]$releaseUri) -or $releaseUri.UserInfo -or $releaseUri.Query -or $releaseUri.Fragment -or ($releaseUri.Scheme -ne 'https' -and -not ($releaseUri.Scheme -eq 'http' -and $releaseUri.Host -eq '127.0.0.1'))) { throw 'Release base must be HTTPS or loopback HTTP.' }
$base = "$releaseBase/$Version"
$webOptions = @{}
if ((Get-Command Invoke-WebRequest).Parameters.ContainsKey('UseBasicParsing')) { $webOptions.UseBasicParsing = $true }
if ($releaseUri.Host -eq '127.0.0.1' -and (Get-Command Invoke-WebRequest).Parameters.ContainsKey('NoProxy')) { $webOptions.NoProxy = $true }
function Get-ReleaseFile([string]$Url, [string]$OutputFile) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try { Invoke-WebRequest $Url -OutFile $OutputFile @webOptions; return }
        catch { if ($attempt -eq 3) { throw }; Start-Sleep -Seconds $attempt }
    }
}
$temp = Join-Path ([IO.Path]::GetTempPath()) ('filetree233-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    $binary = Join-Path $temp $name
    $sums = Join-Path $temp 'SHA256SUMS'
    Get-ReleaseFile "$base/$name" $binary
    Get-ReleaseFile "$base/SHA256SUMS" $sums
    $line = Get-Content -LiteralPath $sums | Where-Object { $_ -match ('^[0-9a-fA-F]{64}  ' + [regex]::Escape($name) + '$') }
    if (@($line).Count -ne 1) { throw 'Missing or ambiguous checksum.' }
    $expected = ($line -split '\s+')[0]
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $stream = [IO.File]::OpenRead($binary)
        try { $actual = [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '') }
        finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
    if ($actual -ne $expected) { throw 'SHA-256 mismatch.' }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $destinationDir = (Resolve-Path -LiteralPath $InstallDir).ProviderPath
    $destination = Join-Path $destinationDir 'filetree233.exe'
    $staged = Join-Path $destinationDir ('.filetree233-' + [guid]::NewGuid().ToString('N') + '.tmp')
    try {
        Copy-Item -LiteralPath $binary -Destination $staged
        if ([IO.File]::Exists($destination)) {
            $backup = Join-Path $destinationDir ('.filetree233-' + [guid]::NewGuid().ToString('N') + '.bak')
            [IO.File]::Replace($staged, $destination, $backup)
            Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
        }
        else { [IO.File]::Move($staged, $destination) }
    } finally { if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue } }
    Write-Host "Installed to $destination"
    Write-Host 'Run the executable, or add this directory to your PATH.'
} finally { Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue }

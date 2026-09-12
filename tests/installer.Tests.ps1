$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../get-nodeharbor.ps1')

if ((Get-NodeHarborTarget -Architecture 'X64') -ne 'x86_64-pc-windows-msvc') { throw 'Windows x64 target was not selected' }
foreach ($architecture in @('Arm64','X86')) {
    $rejected = $false
    try { Get-NodeHarborTarget -Architecture $architecture | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Unsupported Windows architecture was accepted: $architecture" }
}
$directory = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $directory | Out-Null
try {
    $package = Join-Path $directory 'nodeharbor.exe'
    Set-Content -Path $package -Value 'corrupt-package'
    $rejected = $false
    try { Test-NodeHarborChecksum -Path $package -Expected ('0' * 64) } catch { $rejected = $true }
    if (-not $rejected) { throw 'A corrupted Windows package was accepted' }
    Test-NodeHarborChecksum -Path $package -Expected (Get-FileHash -Algorithm SHA256 -Path $package).Hash.ToLowerInvariant()
} finally { Remove-Item -LiteralPath $directory -Recurse -Force }
Write-Host 'Windows installer contracts passed'

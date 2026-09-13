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

$transactionRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('nodeharbor-rollback-test-' + [guid]::NewGuid())
$application = Join-Path $transactionRoot 'app'
New-Item -ItemType Directory -Path $application -Force | Out-Null
try {
    $previous = Join-Path $application 'nodeharbor.exe'
    $settings = Join-Path $transactionRoot 'settings.json'
    Set-Content -LiteralPath $previous -Value 'previous-working-application'
    Set-Content -LiteralPath $settings -Value 'existing-device-credential'
    $failed = $false
    try {
        Invoke-NodeHarborInstallTransaction -InstallDirectory $application -Install {
            Set-Content -LiteralPath $previous -Value 'partial-new-application'
            throw 'simulated NSIS failure after replacing an existing file'
        }
    } catch { $failed = $true }
    if (-not $failed) { throw 'Partial installation unexpectedly succeeded' }
    if ((Get-Content -Raw -LiteralPath $previous).Trim() -ne 'previous-working-application') { throw 'Partial installer failure destroyed the previous application' }
    if ((Get-Content -Raw -LiteralPath $settings).Trim() -ne 'existing-device-credential') { throw 'Installer rollback changed device credentials' }
    Invoke-NodeHarborInstallTransaction -InstallDirectory $application -Install {
        Set-Content -LiteralPath $previous -Value 'verified-new-application'
    }
    if ((Get-Content -Raw -LiteralPath $previous).Trim() -ne 'verified-new-application') { throw 'Successful installer changes were rolled back' }
} finally { Remove-Item -LiteralPath $transactionRoot -Recurse -Force }
Write-Host 'Windows installation rollback contracts passed'

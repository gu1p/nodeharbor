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

foreach ($output in @('NodeHarbor 0.1.2 (' + ('a' * 40) + ')', 'NodeHarbor 0.1.20 (' + ('a' * 40) + ')', 'unreadable application', 'NodeHarbor 0.1.3 (development)')) {
    $rejected = $false
    try { Assert-NodeHarborInstalledVersion -VersionOutput $output -Expected '0.1.3' } catch { $rejected = $true }
    if (-not $rejected) { throw 'An incorrect or unidentifiable application was accepted after installation' }
}
Assert-NodeHarborInstalledVersion -VersionOutput ('NodeHarbor 0.1.3 (' + ('b' * 40) + ")`r`n") -Expected '0.1.3'
Write-Host 'Windows installed-version contracts passed'

$repairRoot = Join-Path ([IO.Path]::GetTempPath()) ('nodeharbor-native-repair-test-' + [guid]::NewGuid())
$repairApp = Join-Path $repairRoot 'application'
New-Item -ItemType Directory -Path $repairApp -Force | Out-Null
try {
    $repairBinary = Join-Path $repairApp 'nodeharbor.exe'
    Set-Content -LiteralPath $repairBinary -Value 'previous application'
    $script:registration = 'previous registration'
    $script:repairCalls = 0
    $failed = $false
    try {
        Invoke-NodeHarborInstallTransaction -InstallDirectory $repairApp -Install {
            Set-Content -LiteralPath $repairBinary -Value 'partly upgraded application'
            $script:registration = 'new registration'
            throw 'Upgrade could not verify its installed version'
        } -Restore {
            $script:repairCalls += 1
            Set-Content -LiteralPath $repairBinary -Value 'previous application'
            $script:registration = 'previous registration'
        }
    } catch { $failed = $true }
    if (-not $failed -or $script:repairCalls -ne 1) { throw 'A failed upgrade did not invoke the previous native installer exactly once' }
    if ($script:registration -ne 'previous registration') { throw 'Native installation registration was not restored' }
    if ((Get-Content -Raw -LiteralPath $repairBinary).Trim() -ne 'previous application') { throw 'Native restoration did not restore the previous application' }
} finally { Remove-Item -LiteralPath $repairRoot -Recurse -Force }
Write-Host 'Windows native installer recovery contracts passed'

$downloadRoot = Join-Path ([IO.Path]::GetTempPath()) ('nodeharbor-release-download-test-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $downloadRoot | Out-Null
try {
    $script:downloadCalls = @()
    $script:badChecksum = $false
    $script:metadataFault = ''
    $fixture = Join-Path $downloadRoot 'fixture'
    [IO.File]::WriteAllText($fixture, 'verified installer fixture')
    $script:fixtureHash = (Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash.ToLowerInvariant()
    function Invoke-RestMethod {
        param([string]$Uri, [hashtable]$Headers)
        $script:downloadCalls += $Uri
        if ($Uri -ne 'https://api.github.com/repos/gu1p/nodeharbor/releases/tags/v0.1.3') { throw 'Unexpected release metadata request' }
        $hash = if ($script:badChecksum) { '0' * 64 } else { $script:fixtureHash }
        $asset = @{ name = 'nodeharbor-v0.1.3-x86_64-pc-windows-msvc.exe'; digest = ('sha256:' + $hash); state = 'uploaded'; browser_download_url = 'https://github.com/gu1p/nodeharbor/releases/download/v0.1.3/nodeharbor-v0.1.3-x86_64-pc-windows-msvc.exe' }
        $release = @{ tag_name = 'v0.1.3'; draft = $false; assets = @($asset) }
        switch ($script:metadataFault) {
            'duplicate' { $release.assets = @($asset, $asset) }
            'missing' { $release.assets = @() }
            'digest' { $asset.digest = $null }
            'draft' { $release.draft = $true }
            'version' { $release.tag_name = 'v0.1.4' }
            'url' { $asset.browser_download_url = 'https://example.com/installer.exe' }
            'state' { $asset.state = 'starter' }
        }
        return $release
    }
    function Invoke-WebRequest {
        param([switch]$UseBasicParsing, [string]$Uri, [string]$OutFile)
        $script:downloadCalls += $Uri
        if ($Uri -ne 'https://github.com/gu1p/nodeharbor/releases/download/v0.1.3/nodeharbor-v0.1.3-x86_64-pc-windows-msvc.exe') { throw 'The installer requested an auxiliary release file' }
        [IO.File]::WriteAllText($OutFile, 'verified installer fixture')
    }
    $downloaded = Get-NodeHarborReleasePackage -Version '0.1.3' -Target 'x86_64-pc-windows-msvc' -Directory $downloadRoot
    if (-not (Test-Path -LiteralPath $downloaded)) { throw 'Verified native recovery package is unavailable' }
    if ($script:downloadCalls.Count -ne 2) { throw 'Recovery package must use only release API metadata and the native installer' }
    $script:badChecksum = $true
    $rejected = $false
    try { Get-NodeHarborReleasePackage -Version '0.1.3' -Target 'x86_64-pc-windows-msvc' -Directory $downloadRoot | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'Unverified recovery package was accepted' }
    $script:badChecksum = $false
    foreach ($fault in @('duplicate','missing','digest','draft','version','url','state')) {
        $script:metadataFault = $fault
        $script:downloadCalls = @()
        $rejected = $false
        try { Get-NodeHarborReleasePackage -Version '0.1.3' -Target 'x86_64-pc-windows-msvc' -Directory $downloadRoot | Out-Null } catch { $rejected = $true }
        if (-not $rejected -or $script:downloadCalls.Count -ne 1) { throw "Invalid metadata was accepted or downloaded a package: $fault" }
    }
} finally {
    Remove-Item Function:Invoke-WebRequest -ErrorAction SilentlyContinue
    Remove-Item Function:Invoke-RestMethod -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $downloadRoot -Recurse -Force
}
Write-Host 'Windows verified recovery-download contracts passed'

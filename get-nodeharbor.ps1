$ErrorActionPreference = 'Stop'

function Get-NodeHarborTarget {
    param([string] $Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString())
    if ($Architecture -ne 'X64') { throw "Unsupported Windows architecture: $Architecture. This release supports native Windows x64." }
    return 'x86_64-pc-windows-msvc'
}

function Test-NodeHarborChecksum {
    param([Parameter(Mandatory)][string] $Path, [Parameter(Mandatory)][string] $Expected)
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Expected -cnotmatch '^[0-9a-f]{64}$' -or $actual -cne $Expected) {
        throw 'Package checksum verification failed; the previous installation has been preserved.'
    }
}

function Assert-NodeHarborInstalledVersion {
    param([Parameter(Mandatory)][string] $VersionOutput, [Parameter(Mandatory)][string] $Expected)
    $pattern = '^NodeHarbor ' + [regex]::Escape($Expected) + ' \([0-9a-f]{40}\)$'
    if ($VersionOutput.Trim() -cnotmatch $pattern) {
        throw 'The installed application does not identify the requested release and source commit.'
    }
}

function Invoke-NodeHarborInstallTransaction {
    param([Parameter(Mandatory)][string] $InstallDirectory, [Parameter(Mandatory)][scriptblock] $Install, [scriptblock] $Restore)
    $backup = $null
    $completed = $false
    if (Test-Path -LiteralPath (Join-Path $InstallDirectory 'nodeharbor.exe')) {
        $parent = Split-Path -Parent ([IO.Path]::GetFullPath($InstallDirectory))
        if (-not $parent) { throw 'The application must have its own installation directory.' }
        $backup = Join-Path $parent ('.nodeharbor-backup-' + [guid]::NewGuid().ToString())
        Copy-Item -LiteralPath $InstallDirectory -Destination $backup -Recurse -Force
    }
    try {
        & $Install
        $completed = $true
    } catch {
        $failure = $_
        if ($backup) {
            $nativeRestored = $false
            $repairFailure = $null
            if ($Restore) {
                try { & $Restore; $nativeRestored = $true }
                catch { $repairFailure = $_.Exception.Message }
            }
            try {
                # Restore existing application files without deleting unrelated files
                # the owner may have placed in a custom installation directory.
                New-Item -ItemType Directory -Path $InstallDirectory -Force | Out-Null
                Get-ChildItem -LiteralPath $backup -Force | ForEach-Object {
                    Copy-Item -LiteralPath $_.FullName -Destination $InstallDirectory -Recurse -Force
                }
            } catch {
                throw "Installation and file restoration failed. The previous application is preserved at $backup."
            }
            if ($nativeRestored) {
                throw "Installation failed; the previous native installation and application files were restored. Backup retained at $backup. Cause: $($failure.Exception.Message)"
            }
            if ($repairFailure) {
                throw "Installation failed; previous application files were restored, but native installer registration could not be restored: $repairFailure. Backup retained at $backup."
            }
            throw "Installation failed; previous application files were restored. Backup retained at $backup. Cause: $($failure.Exception.Message)"
        }
        throw $failure
    } finally {
        if ($completed -and $backup) { Remove-Item -LiteralPath $backup -Recurse -Force }
    }
}

function Get-NodeHarborReleasePackage {
    param([Parameter(Mandatory)][string] $Version, [Parameter(Mandatory)][string] $Target, [Parameter(Mandatory)][string] $Directory)
    if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or $Target -cne 'x86_64-pc-windows-msvc') { throw 'The native installer requires a supported published release.' }
    $name = "nodeharbor-v$Version-$Target.exe"
    $base = "https://github.com/gu1p/nodeharbor/releases/download/v$Version"
    $package = Join-Path $Directory $name
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/gu1p/nodeharbor/releases/tags/v$Version" -Headers @{ Accept = 'application/vnd.github+json' }
    if ($release.tag_name -cne "v$Version" -or $release.draft -isnot [bool] -or $release.draft) { throw 'The requested release is not published.' }
    $matching = @($release.assets | Where-Object { $_.name -ceq $name })
    if ($matching.Count -ne 1 -or $matching[0].state -cne 'uploaded' -or $matching[0].browser_download_url -cne "$base/$name" -or $matching[0].digest -cnotmatch '^sha256:[0-9a-f]{64}$') {
        throw 'Release package checksum metadata is missing or invalid; the existing application has been preserved.'
    }
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $package
    Test-NodeHarborChecksum -Path $package -Expected $matching[0].digest.Substring(7)
    return $package
}

function Install-NodeHarborNativePackage {
    param([Parameter(Mandatory)][string] $Package, [Parameter(Mandatory)][string] $InstallDirectory, [Parameter(Mandatory)][string] $Version)
    # NSIS requires /D as the final unquoted argument.
    $process = Start-Process -FilePath $Package -ArgumentList "/S /D=$InstallDirectory" -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "The NodeHarbor installer failed with exit code $($process.ExitCode)." }
    $executable = Join-Path $InstallDirectory 'nodeharbor.exe'
    if (-not (Test-Path -LiteralPath $executable)) { throw 'The installer did not produce the expected application.' }
    $versionOutput = (& $executable --version | Out-String)
    if ($LASTEXITCODE -ne 0) { throw 'The installed application could not report its version.' }
    Assert-NodeHarborInstalledVersion -VersionOutput $versionOutput -Expected $Version
}

function Install-NodeHarbor {
    if ($env:OS -ne 'Windows_NT') { throw 'This installer runs on Windows. macOS and Linux use get-nodeharbor.sh.' }
    $target = Get-NodeHarborTarget
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $version = $env:NODEHARBOR_VERSION
    if (-not $version) {
        $response = Invoke-WebRequest -UseBasicParsing -Uri 'https://github.com/gu1p/nodeharbor/releases/latest'
        if ($response.BaseResponse.ResponseUri) { $url = $response.BaseResponse.ResponseUri.AbsoluteUri }
        else { $url = $response.BaseResponse.RequestMessage.RequestUri.AbsoluteUri }
        $version = $url.Split('/')[-1]
    }
    $version = $version -replace '^v',''
    if ($version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') { throw 'No published release was found, or NODEHARBOR_VERSION is invalid.' }
    $directory = Join-Path ([System.IO.Path]::GetTempPath()) ('nodeharbor-install-' + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $directory | Out-Null
    try {
        Write-Host "Downloading NodeHarbor $version for Windows x64..."
        $package = Get-NodeHarborReleasePackage -Version $version -Target $target -Directory $directory
        $install = $env:NODEHARBOR_INSTALL_DIR
        if (-not $install) { $install = Join-Path $env:LOCALAPPDATA 'NodeHarbor' }
        if (-not [IO.Path]::IsPathRooted($install) -or $install.Contains('"') -or $install.Contains("`n")) { throw 'Choose an absolute installation path without quotes or newlines.' }
        $existing = Join-Path $install 'nodeharbor.exe'
        $restore = $null
        if (Test-Path -LiteralPath $existing) {
            $previousOutput = (& $existing --version | Out-String).Trim()
            if ($LASTEXITCODE -ne 0 -or $previousOutput -cnotmatch '^NodeHarbor ((0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)) \([0-9a-f]{40}\)$') { throw 'The existing application does not identify a published version; its installation has been preserved.' }
            $previousVersion = $Matches[1]
            # Have a verified native recovery installer before stopping work or
            # changing files. NSIS owns registration and menu restoration.
            $previousPackage = Get-NodeHarborReleasePackage -Version $previousVersion -Target $target -Directory $directory
            $restore = { Install-NodeHarborNativePackage -Package $previousPackage -InstallDirectory $install -Version $previousVersion }
            $agent = Join-Path $install 'nodeharbor-agent.exe'
            if (-not (Test-Path -LiteralPath $agent)) { throw 'The existing app has no drain helper; quit it and move it aside before installing.' }
            & $agent prepare-update
            if ($LASTEXITCODE -ne 0) { throw 'The worker did not drain; the existing application has been preserved.' }
            & $existing --quit
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            do {
                $running = @(Get-Process -Name nodeharbor -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $existing })
                if ($running.Count -eq 0) { break }
                Start-Sleep -Seconds 1
            } while ([DateTime]::UtcNow -lt $deadline)
            if ($running.Count -gt 0) { throw 'NodeHarbor is still closing; the existing application has been preserved.' }
        }
        Invoke-NodeHarborInstallTransaction -InstallDirectory $install -Install {
            Install-NodeHarborNativePackage -Package $package -InstallDirectory $install -Version $version
        } -Restore $restore
        Write-Host "Installed NodeHarbor $version. Open it from the Start menu. Enrollment and resource limits were preserved; sharing stays paused after an update."
    } finally {
        Remove-Item -LiteralPath $directory -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($MyInvocation.InvocationName -ne '.') { Install-NodeHarbor }

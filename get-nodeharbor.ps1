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
    $name = "nodeharbor-v$version-$target.exe"
    $base = "https://github.com/gu1p/nodeharbor/releases/download/v$version"
    $directory = Join-Path ([System.IO.Path]::GetTempPath()) ('nodeharbor-install-' + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $directory | Out-Null
    try {
        $package = Join-Path $directory $name
        $sums = Join-Path $directory 'SHA256SUMS'
        Write-Host "Downloading NodeHarbor $version for Windows x64..."
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $package
        $matching = @(Get-Content -LiteralPath $sums | Where-Object { $_ -cmatch ('^[0-9a-f]{64}  ' + [regex]::Escape($name) + '$') })
        if ($matching.Count -ne 1) { throw 'Release checksum is missing or ambiguous; the existing application has been preserved.' }
        Test-NodeHarborChecksum -Path $package -Expected $matching[0].Substring(0,64)
        $install = $env:NODEHARBOR_INSTALL_DIR
        if (-not $install) { $install = Join-Path $env:LOCALAPPDATA 'NodeHarbor' }
        if (-not [IO.Path]::IsPathRooted($install) -or $install.Contains('"') -or $install.Contains("`n")) { throw 'Choose an absolute installation path without quotes or newlines.' }
        $existing = Join-Path $install 'nodeharbor.exe'
        if (Test-Path -LiteralPath $existing) {
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
        # NSIS expects /D to be the final, unquoted argument. The download has
        # already passed integrity checks and the running worker is stopped.
        $process = Start-Process -FilePath $package -ArgumentList "/S /D=$install" -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "The NodeHarbor installer failed with exit code $($process.ExitCode)." }
        if (-not (Test-Path -LiteralPath (Join-Path $install 'nodeharbor.exe'))) { throw 'The installer did not produce the expected application.' }
        Write-Host "Installed NodeHarbor $version. Open it from the Start menu. Enrollment and resource limits were preserved; sharing stays paused after an update."
    } finally {
        Remove-Item -LiteralPath $directory -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($MyInvocation.InvocationName -ne '.') { Install-NodeHarbor }

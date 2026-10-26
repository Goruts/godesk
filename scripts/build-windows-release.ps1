#requires -Version 5.1

<#
.SYNOPSIS
Builds and packages Godesk for Windows x64, optionally updating from Git.

.DESCRIPTION
Builds the current files, including local changes, without updating Git.
Use -Update to fast-forward a clean branch before building. Use -CheckOnly to
verify prerequisites without updating, installing dependencies, or building.
Use -PackageOnly to collect an already successful build after a packaging failure.
Creates the release application and NSIS installer and collects distributable
files under dist-windows/.
#>

[CmdletBinding()]
param(
    [switch]$Update,
    [switch]$PackageOnly,
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
if ($PackageOnly -and ($Update -or $CheckOnly)) {
    throw '-PackageOnly cannot be combined with -Update or -CheckOnly.'
}
if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue) {
    $PSNativeCommandUseErrorActionPreference = $false
}

function Invoke-GodeskCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Program,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Program $($Arguments -join ' ')"
    }
}

# Like Invoke-GodeskCommand, but retries a few times. `npm ci` deletes
# node_modules, and on Windows that fails (EPERM / exit -4048) when a native
# module such as rolldown's .node is still mapped by another process (a running
# `npm run dev`/Vite server, an editor, or an antivirus scan). A short wait
# usually lets the holder release the file.
function Invoke-GodeskCommandWithRetry {
    param(
        [Parameter(Mandatory = $true)][string]$Program,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [int]$Retries = 3,
        [int]$DelaySeconds = 4
    )

    for ($attempt = 1; ; $attempt++) {
        & $Program @Arguments
        if ($LASTEXITCODE -eq 0) { return }
        if ($attempt -gt $Retries) {
            throw "Command failed with exit code ${LASTEXITCODE} after $attempt attempts: $Program $($Arguments -join ' '). If this is a locked-file error (EPERM / unlink), close any 'npm run dev' server, code editors, or antivirus scan holding frontend\node_modules and run the build again."
        }
        Write-Host "Attempt $attempt failed (exit ${LASTEXITCODE}); a file may be locked (dev server, editor, or antivirus). Retrying in ${DelaySeconds}s..." -ForegroundColor Yellow
        Start-Sleep -Seconds $DelaySeconds
    }
}

function Require-GodeskCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$InstallHint
    )

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $command) {
        throw "$Name was not found. $InstallHint"
    }
    return $command.Source
}

if ($env:OS -ne 'Windows_NT') {
    throw 'This script builds the native Windows installer and must run on Windows.'
}

$repository = Split-Path -Parent $PSScriptRoot
$repository = [IO.Path]::GetFullPath($repository)
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$env:Path = "$machinePath;$userPath;$env:Path"

Push-Location -LiteralPath $repository
try {
    $git = Require-GodeskCommand 'git.exe' 'Install Git for Windows, then open a new PowerShell window.'
    $insideRepository = & $git rev-parse --is-inside-work-tree 2>$null
    if ($LASTEXITCODE -ne 0 -or ([string]$insideRepository).Trim() -ne 'true') {
        throw "$repository is not a Git worktree."
    }

    if ($Update) {
        $trackedChanges = @(& $git status --porcelain --untracked-files=no)
        if ($LASTEXITCODE -ne 0) { throw 'Git could not inspect the worktree.' }
        if ($trackedChanges.Count -gt 0) {
            throw 'Cannot update a checkout with tracked changes. Run without -Update to build your current files, or commit/stash them before updating.'
        }
        $branch = ([string](& $git branch --show-current)).Trim()
        if ($LASTEXITCODE -ne 0) { throw 'Git could not read the current branch.' }
        if (-not $branch) {
            throw 'Updating requires a branch. Run without -Update to build the current checkout.'
        }
        if (-not $CheckOnly) {
            Write-Host "Updating branch $branch..."
            Invoke-GodeskCommand $git @('pull', '--ff-only')
        }
    } else {
        if ($PackageOnly) { Write-Host 'Packaging the existing release artifacts.' }
        else { Write-Host 'Building the current checkout without git pull (local changes are included).' }
    }

    # The Tauri config runs the frontend build through npm, so npm (not pnpm)
    # installs both package-lock.json trees.
    $npm = Require-GodeskCommand 'npm.cmd' 'Install Node.js, then open a new PowerShell window.'

    # A repository-local toolchain is useful on machines prepared by a
    # bootstrap process; otherwise use the normal rustup installation on PATH.
    $localCargoBin = Join-Path $repository '.build-tools\cargo\bin'
    if (-not (Get-Command 'cargo.exe' -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath (Join-Path $localCargoBin 'cargo.exe'))) {
        $env:CARGO_HOME = Join-Path $repository '.build-tools\cargo'
        $env:RUSTUP_HOME = Join-Path $repository '.build-tools\rustup'
        $env:Path = "$localCargoBin;$env:Path"
    }
    $null = Require-GodeskCommand 'cargo.exe' 'Install rustup with the x86_64-pc-windows-msvc toolchain.'
    $rustc = Require-GodeskCommand 'rustc.exe' 'Install rustup with the x86_64-pc-windows-msvc toolchain.'
    $rustDetails = (& $rustc -vV) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw 'rustc could not report its toolchain.' }
    if ($rustDetails -notmatch '(?m)^host: x86_64-pc-windows-msvc$') {
        throw 'The active Rust host must be x86_64-pc-windows-msvc to create the Windows x64 installer.'
    }

    # Cargo discovers MSVC and the Windows SDK through Visual Studio. Checking
    # here gives a short prerequisite error instead of a late link failure.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw 'Microsoft C++ Build Tools were not found. Install the Desktop development with C++ workload.'
    }
    $buildToolsOutput = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    $buildTools = ($buildToolsOutput -join '').Trim()
    if ($LASTEXITCODE -ne 0 -or -not $buildTools) {
        throw 'Microsoft C++ Build Tools are incomplete. Install the Desktop development with C++ workload and Windows SDK.'
    }

    if ($CheckOnly) {
        Write-Host 'Windows build prerequisites are ready.' -ForegroundColor Green
        return
    }

    if (-not $PackageOnly) {
        Write-Host 'Installing locked dependencies...'
        # The root postinstall runs a non-locked install of the frontend;
        # skip it and install the frontend from its own lockfile instead.
        Invoke-GodeskCommandWithRetry $npm @('ci', '--ignore-scripts')
        Invoke-GodeskCommandWithRetry $npm @('ci', '--prefix', 'frontend')

        Write-Host 'Building the optimized Godesk application and NSIS installer...'
        Invoke-GodeskCommand $npm @('run', 'tauri', '--', 'build', '--bundles', 'nsis', '--', '--locked')
    }

    $configurationPath = Join-Path $repository 'src-tauri\tauri.conf.json'
    $configuration = Get-Content -LiteralPath $configurationPath -Raw | ConvertFrom-Json
    $version = [string]$configuration.version
    if (-not $version) { throw "No version was found in $configurationPath" }
    if ($version -notmatch '^[0-9A-Za-z][0-9A-Za-z.+-]*$') {
        throw "The configured version cannot be used in an artifact path: $version"
    }

    $releaseDirectory = Join-Path $repository 'src-tauri\target\release'
    $application = Join-Path $releaseDirectory 'godesk.exe'
    $installer = Join-Path $releaseDirectory "bundle\nsis\Godesk_${version}_x64-setup.exe"
    # The ConPTY that terminals run behind (src-tauri/conpty/README.md), which
    # Tauri copies beside the application.
    $conpty = @('conpty.dll', 'OpenConsole.exe') | ForEach-Object { Join-Path $releaseDirectory $_ }
    foreach ($requiredPath in @($application, $installer) + $conpty) {
        if (-not (Test-Path -LiteralPath $requiredPath)) {
            throw "The build completed without the expected artifact: $requiredPath"
        }
    }

    $deliveryRoot = [IO.Path]::GetFullPath((Join-Path $repository 'dist-windows'))
    $delivery = [IO.Path]::GetFullPath((Join-Path $deliveryRoot "Godesk-${version}-windows-x64"))
    if (Test-Path -LiteralPath $delivery) {
        $delivery = Join-Path $deliveryRoot ("Godesk-${version}-windows-x64-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
        Write-Host 'Keeping the previous delivery; collecting this build in a new folder.'
    }
    $deliveryRootPrefix = $deliveryRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $delivery.StartsWith($deliveryRootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to replace an artifact directory outside ${deliveryRoot}: $delivery"
    }
    $portable = Join-Path $delivery 'portable\Godesk'
    if (Test-Path -LiteralPath $delivery) { throw "The delivery folder already exists: $delivery" }
    New-Item -ItemType Directory -Path $portable -Force | Out-Null

    $deliveredInstaller = Join-Path $delivery "Godesk_${version}_x64-setup.exe"
    Copy-Item -LiteralPath $installer -Destination $deliveredInstaller
    Copy-Item -LiteralPath $application -Destination (Join-Path $portable 'godesk.exe')
    foreach ($file in $conpty) { Copy-Item -LiteralPath $file -Destination $portable }

    $portableZip = Join-Path $delivery "Godesk_${version}_windows-x64-portable.zip"
    Compress-Archive -LiteralPath $portable -DestinationPath $portableZip -CompressionLevel Optimal

    $instructions = @"
Godesk $version for Windows x64

Recommended installation:
  Run Godesk_${version}_x64-setup.exe. It installs Godesk for the current user.

Portable copy:
  Extract Godesk_${version}_windows-x64-portable.zip completely, keeping
  conpty.dll and OpenConsole.exe beside godesk.exe, then run godesk.exe.

Requirements when running Godesk:
  Windows 10/11 with the Microsoft Edge WebView2 runtime (included in
  Windows 11; the installer downloads it on Windows 10 when missing).
  Windows may warn about an unsigned application because this local build is
  not code-signed.
"@
    Set-Content -LiteralPath (Join-Path $delivery 'README.txt') -Value $instructions -Encoding UTF8

    $hashTargets = @($deliveredInstaller, $portableZip, (Join-Path $portable 'godesk.exe'))
    $hashLines = foreach ($target in $hashTargets) {
        $hash = Get-FileHash -LiteralPath $target -Algorithm SHA256
        '{0}  {1}' -f $hash.Hash.ToLowerInvariant(), (Split-Path -Leaf $target)
    }
    Set-Content -LiteralPath (Join-Path $delivery 'SHA256SUMS.txt') -Value $hashLines -Encoding ASCII

    Write-Host ''
    Write-Host 'Windows release is ready:' -ForegroundColor Green
    Write-Host "  Installer: $deliveredInstaller"
    Write-Host "  Portable:  $portableZip"
    Write-Host "  Folder:    $delivery"
} finally {
    Pop-Location
}

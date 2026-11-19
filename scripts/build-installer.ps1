<#
.SYNOPSIS
    Builds Facet's installer (target\installer\Facet-Setup-<version>.exe), and with -Install
    runs it to install or update Facet on this PC.

.DESCRIPTION
    1. cargo build --profile dist -p facet          (the shipping exe, fat LTO, ~5 min)
    2. cargo build --profile dist -p facet-setup    (the setup, with that exe embedded)
    3. copy it to target\installer
    4. -Install: run it (per-user, no admin); it replaces an existing install in place.

    The build is stamped with the current commit (plus "-modified" when the working tree has
    changes), shown in Facet's Settings footer and in Settings > Apps.

.EXAMPLE
    .\scripts\build-installer.ps1 -Install
#>
param(
    [switch]$Install,
    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Invoke-Checked([string]$what, [scriptblock]$command) {
    Write-Host "==> $what" -ForegroundColor Cyan
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)." }
}

# Build stamp: the commit, and whether the tree differs from it.
$build = 'local'
try {
    $hash = (git rev-parse --short HEAD 2>$null)
    if ($LASTEXITCODE -eq 0 -and $hash) {
        $build = $hash.Trim()
        $changes = (git status --porcelain 2>$null)
        if ($changes) { $build = "$build-modified" }
    }
} catch { }

$cargoToml = Get-Content (Join-Path $root 'Cargo.toml') -Raw
if ($cargoToml -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw 'Could not read the version from Cargo.toml [workspace.package].'
}
$version = $Matches[1]
Write-Host "Facet $version ($build)" -ForegroundColor Green

$env:FACET_BUILD_ID = $build
Invoke-Checked 'Building facet.exe (dist profile, takes a few minutes)' {
    cargo build --profile dist -p facet
}

$payload = Join-Path $root 'target\dist\facet.exe'
$env:FACET_SETUP_PAYLOAD = $payload
Invoke-Checked 'Building the setup' { cargo build --profile dist -p facet-setup }
Remove-Item Env:FACET_SETUP_PAYLOAD

$outDir = Join-Path $root 'target\installer'
New-Item -ItemType Directory -Force $outDir | Out-Null
$setup = Join-Path $outDir "Facet-Setup-$version.exe"
Copy-Item (Join-Path $root 'target\dist\facet-setup.exe') $setup -Force
Write-Host "Installer: $setup" -ForegroundColor Green

if ($Install) {
    Write-Host '==> Installing' -ForegroundColor Cyan
    $arguments = @()
    if ($Quiet) { $arguments += '--quiet' }
    $process = if ($arguments.Count) {
        Start-Process -FilePath $setup -ArgumentList $arguments -Wait -PassThru
    } else {
        Start-Process -FilePath $setup -Wait -PassThru
    }
    if ($process.ExitCode -ne 0) { throw "The setup failed (exit code $($process.ExitCode))." }
    Write-Host 'Done.' -ForegroundColor Green
}

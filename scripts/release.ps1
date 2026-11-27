<#
.SYNOPSIS
    Releases a new version of Facet: GitHub builds the installer and publishes it.

.DESCRIPTION
    1. sets [workspace.package] version in Cargo.toml (and the workspace crates in Cargo.lock)
    2. commits "Release <version>" and tags it v<version>
    3. pushes main and the tag to origin

    The tag starts .github/workflows/release.yml, which tests, builds Facet-Setup-<version>.exe
    and attaches it to a GitHub Release named "Facet <version>" (a few minutes later).

    With no version, re-tags the version already in Cargo.toml (for the first release, or when
    the version was set by hand).

.EXAMPLE
    .\scripts\release.ps1 0.2.0
#>
param(
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Invoke-Checked([string]$what, [scriptblock]$command) {
    Write-Host "==> $what" -ForegroundColor Cyan
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)." }
}

$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main') { throw "Releases are made from main; this is $branch." }
if (git status --porcelain) { throw 'The working tree has changes; commit or stash them first.' }

$cargoPath = Join-Path $root 'Cargo.toml'
$cargoToml = [IO.File]::ReadAllText($cargoPath)
$pattern = '(?ms)(^\[workspace\.package\].*?^version\s*=\s*")([^"]+)(")'
if ($cargoToml -notmatch $pattern) {
    throw 'Could not read the version from Cargo.toml [workspace.package].'
}
$current = $Matches[2]
if (-not $Version) { $Version = $current }
$Version = $Version.TrimStart('v')
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "'$Version' is not a version like 1.2.3."
}
$tag = "v$Version"

Invoke-Checked 'Fetching' { git fetch origin --tags }
if (git tag --list $tag) { throw "Tag $tag already exists." }

if ($Version -ne $current) {
    $updated = ([regex]$pattern).Replace($cargoToml, "`${1}$Version`${3}", 1)
    [IO.File]::WriteAllText($cargoPath, $updated, (New-Object Text.UTF8Encoding $false))
    Invoke-Checked 'Updating Cargo.lock' { cargo update --workspace }
    Invoke-Checked 'Committing' { git commit -am "Release $Version" }
}

Invoke-Checked "Tagging $tag" { git tag -a $tag -m "Facet $Version" }
Invoke-Checked 'Pushing' { git push --atomic origin main $tag }

$url = (git remote get-url origin).Trim() -replace '\.git$', '' -replace '//[^@/]+@', '//'
Write-Host "Released $tag. GitHub is building it: $url/actions" -ForegroundColor Green
Write-Host "The installer appears at $url/releases/tag/$tag when the run finishes." -ForegroundColor Green

<#
.SYNOPSIS
Fills the winget manifests in this folder for one LWSpot release.

.DESCRIPTION
Sets the version, installer URL, release date and SHA-256 hash of the
three manifests (BJDevelopes.LWSpot*.yaml) from the release's checksums.txt.
The x64 and arm64 entries both name the one installer, which installs the
build for the processor it runs on.
With -OutDir the filled manifests are written there, ready to copy into
manifests\b\BJDevelopes\LWSpot\<version>\ of a microsoft/winget-pkgs fork;
otherwise the files here are updated in place.

.EXAMPLE
.\update-manifests.ps1 -Version 0.1.0
Reads checksums.txt from the v0.1.0 GitHub release.

.EXAMPLE
.\update-manifests.ps1 -Version 0.1.0 -Checksums ..\..\dist\checksums.txt -OutDir ..\..\dist\winget
#>
[CmdletBinding()]
param(
    # The release version, without the leading v.
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,
    # A checksums.txt file or URL. Defaults to the release's own.
    [string]$Checksums,
    # The release date, yyyy-MM-dd. Defaults to today (UTC).
    [string]$ReleaseDate = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd'),
    # Where to write the filled manifests. Defaults to this folder.
    [string]$OutDir = $PSScriptRoot
)

$ErrorActionPreference = 'Stop'
$repository = 'https://github.com/BJDevelopes/lwspot'
$tag = "v$Version"

if (-not $Checksums) {
    $Checksums = "$repository/releases/download/$tag/checksums.txt"
}
if ($Checksums -match '^https?://') {
    $text = (Invoke-WebRequest -UseBasicParsing -Uri $Checksums).Content
    if ($text -is [byte[]]) { $text = [Text.Encoding]::UTF8.GetString($text) }
} else {
    $text = [IO.File]::ReadAllText((Resolve-Path $Checksums))
}

# sha256sum lines: "<64 hex digits>  <name>", or " *<name>" in binary mode.
$hashes = @{}
foreach ($line in $text -split "`r?`n") {
    if ($line -match '^([0-9A-Fa-f]{64}) [ *](.+)$') {
        $hashes[$Matches[2].Trim()] = $Matches[1].ToUpperInvariant()
    }
}

# One installer for every PC: it installs the build for the processor it
# runs on, so the x64 and arm64 entries name the same file.
$name = "lwspot-$tag-setup.exe"
if (-not $hashes.ContainsKey($name)) {
    throw "checksums.txt has no line for $name"
}
$installer = @{
    Url    = "$repository/releases/download/$tag/$name"
    Sha256 = $hashes[$name]
}
$installers = @{ x64 = $installer; arm64 = $installer }

if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir | Out-Null
}
# .NET resolves relative paths against the process's folder, not PowerShell's.
$OutDir = (Resolve-Path $OutDir).Path
$utf8 = New-Object Text.UTF8Encoding $false
foreach ($file in 'BJDevelopes.LWSpot.yaml', 'BJDevelopes.LWSpot.installer.yaml', 'BJDevelopes.LWSpot.locale.en-US.yaml') {
    $lines = [IO.File]::ReadAllLines((Join-Path $PSScriptRoot $file))
    $architecture = $null
    # The template's own comment does not go into the submission.
    $output = foreach ($line in $lines | Where-Object { $_ -notmatch '^# Template:' }) {
        if ($line -match '^PackageVersion:') {
            "PackageVersion: $Version"
        } elseif ($line -match '^ReleaseDate:') {
            "ReleaseDate: $ReleaseDate"
        } elseif ($line -match '^ReleaseNotesUrl:') {
            "ReleaseNotesUrl: $repository/releases/tag/$tag"
        } elseif ($line -match '^- Architecture: (\S+)') {
            $architecture = $Matches[1]
            if (-not $installers.ContainsKey($architecture)) {
                throw "$file lists an unknown architecture: $architecture"
            }
            $line
        } elseif ($line -match '^(\s+)InstallerUrl:') {
            "$($Matches[1])InstallerUrl: $($installers[$architecture].Url)"
        } elseif ($line -match '^(\s+)InstallerSha256:') {
            "$($Matches[1])InstallerSha256: $($installers[$architecture].Sha256)"
        } else {
            $line
        }
    }
    $target = Join-Path $OutDir $file
    [IO.File]::WriteAllText($target, (($output -join "`n") + "`n"), $utf8)
    Write-Host "Wrote $target"
}

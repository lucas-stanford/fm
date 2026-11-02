# Installs an fm release binary on Windows.
#
#   irm https://raw.githubusercontent.com/lucas-stanford/fm/main/install.ps1 | iex
#   .\install.ps1 -Dir 'C:\tools' -Version v0.1.0 -NoModifyPath
#
# Adds the install directory to the user PATH, once. It cannot change the PATH
# of the shell that is running it.

[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$Dir = (Join-Path $env:LOCALAPPDATA 'fm\bin'),
    [string]$Repo = $(if ($env:FM_REPO) { $env:FM_REPO } else { 'lucas-stanford/fm' }),
    [switch]$NoModifyPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:FM_NO_MODIFY_PATH -and $env:FM_NO_MODIFY_PATH -ne '0') {
    $NoModifyPath = $true
}

function Write-Step { param([string]$Message) Write-Host "  $Message" }

# --- resolve the release -----------------------------------------------------

$target = 'x86_64-pc-windows-msvc'
$api = if ($Version -eq 'latest') {
    "https://api.github.com/repos/$Repo/releases/latest"
} else {
    "https://api.github.com/repos/$Repo/releases/tags/$Version"
}

Write-Host "fm installer"
Write-Step "release: $Version"
$release = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'fm-installer' }
$tag = $release.tag_name
$asset = $release.assets | Where-Object { $_.name -like "*$target*.zip" } | Select-Object -First 1
if (-not $asset) {
    throw "no $target asset in release $tag"
}

# --- download and unpack -----------------------------------------------------

$staging = Join-Path ([IO.Path]::GetTempPath()) ("fm-install-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $staging | Out-Null
try {
    $archive = Join-Path $staging $asset.name
    Write-Step "downloading $($asset.name)"
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $archive -UseBasicParsing
    Expand-Archive -Path $archive -DestinationPath $staging -Force

    $binary = Get-ChildItem -Path $staging -Recurse -Filter 'fm.exe' | Select-Object -First 1
    if (-not $binary) { throw 'the archive contains no fm.exe' }

    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $destination = Join-Path $Dir 'fm.exe'
    Copy-Item -Path $binary.FullName -Destination $destination -Force
    Write-Step "installed to $destination"
}
finally {
    Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
}

# --- PATH --------------------------------------------------------------------

if ($NoModifyPath) {
    Write-Step 'PATH not modified (as requested)'
}
else {
    # Read the *user* PATH specifically. Reading $env:PATH and writing it back
    # would fold the machine PATH into the user one and duplicate every entry.
    $userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
    $entries = @()
    if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ -ne '' } }

    $normalise = { param($p) $p.TrimEnd('\').ToLowerInvariant() }
    $already = $entries | Where-Object { (& $normalise $_) -eq (& $normalise $Dir) }

    if ($already) {
        Write-Step 'PATH already contains the install directory'
    }
    else {
        # Append, preserving every existing entry in order.
        $updated = (@($entries) + $Dir) -join ';'
        [Environment]::SetEnvironmentVariable('PATH', $updated, 'User')
        Write-Step "added $Dir to your user PATH"
    }
}

# --- verification ------------------------------------------------------------

# Prepend for *this* process so the check below tests the binary just installed.
$env:PATH = "$Dir;$env:PATH"

$installedVersion = & $destination --version
$resolved = (Get-Command fm -ErrorAction SilentlyContinue).Source

Write-Host ''
Write-Host "  $installedVersion"
if ($resolved -and ((& $normalise $resolved) -ne (& $normalise $destination))) {
    Write-Warning "another fm earlier on PATH will run instead: $resolved"
    Write-Warning 'remove it, or put the install directory first.'
}

Write-Host ''
Write-Host 'Open a new terminal, or activate it in this one with:'
Write-Host "  `$env:PATH = `"$Dir;`$env:PATH`""
Write-Host ''
Write-Host 'Then:  fm render --help'

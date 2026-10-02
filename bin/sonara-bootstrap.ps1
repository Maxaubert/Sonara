#Requires -Version 5
<#
  Sonara runtime bootstrap (#202): needs only Windows PowerShell 5.1.

  Installs the runtime the plugin needs into
  %LOCALAPPDATA%\Sonara\runtime\<Version>\ :
    1. download sonara-runtime-win-x64-<Version>.zip and SHA256SUMS from the
       v<Version> GitHub release (SONARA_RELEASE_BASE_URL overrides the base
       URL, for tests),
    2. check the zip's SHA-256 against SHA256SUMS (a mismatch installs
       nothing),
    3. extract it into a staging folder next to the destination, then move
       it into place in one rename (a half-extracted runtime is never seen),
    4. start it (`sonara.exe start`, unless -NoStart or
       SONARA_BOOTSTRAP_START=0), which replaces a runtime of an older
       release that is still running,
    5. remove the folders of older releases (newer ones stay: upgrades
       go one way).
  bin/sonara-hook-launch runs it in the background from a hook (with -Lock,
  the lock folder it took, removed here when done); bin/sonara runs it in
  the foreground before a slash command. A failure is logged to
  <home>\logs\bootstrap.log and recorded in runtime\.bootstrap.failed, so
  hooks retry only after a few minutes.
#>
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$Lock = "",
  [switch]$NoStart
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
# .NET only for the download, the hash and the unzip: Get-FileHash and
# Expand-Archive are script modules that Windows PowerShell fails to load
# when a PowerShell 7 parent left its PSModulePath behind.
Add-Type -AssemblyName System.IO.Compression.FileSystem

$BaseUrl = "https://github.com/Maxaubert/Sonara/releases/download"
if ($env:SONARA_RELEASE_BASE_URL) { $BaseUrl = $env:SONARA_RELEASE_BASE_URL.TrimEnd("/") }
$Root    = Join-Path $env:LOCALAPPDATA "Sonara\runtime"
$Dest    = Join-Path $Root $Version
$Failed  = Join-Path $Root ".bootstrap.failed"
$HomeDir = if ($env:SONARA_HOME) { $env:SONARA_HOME } else { Join-Path $env:LOCALAPPDATA "Sonara" }
$LogPath = Join-Path $HomeDir "logs\bootstrap.log"
$Name    = "sonara-runtime-win-x64-$Version.zip"

function Write-Log([string]$Message) {
  Write-Host $Message
  try {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogPath) | Out-Null
    $line = "{0} {1}" -f (Get-Date).ToString("o"), $Message
    [IO.File]::AppendAllText($LogPath, $line + [Environment]::NewLine)
  } catch { }
}

function Get-Expected([string]$SumsPath, [string]$FileName) {
  # SHA256SUMS lines: "<64 hex>  <file name>" (sha256sum format; "*" marks binary mode).
  foreach ($line in [IO.File]::ReadAllLines($SumsPath)) {
    if ($line -match '^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $FileName) {
      return $Matches[1].ToLowerInvariant()
    }
  }
  return $null
}

function Test-OlderVersion([string]$Name, [string]$Than) {
  # Both "major.minor.patch"; anything else (dot folders, other names) is
  # not an older release.
  $pattern = '^\d+\.\d+\.\d+$'
  if ($Name -notmatch $pattern -or $Than -notmatch $pattern) { return $false }
  return ([version]$Name) -lt ([version]$Than)
}

function Save-Url([string]$Url, [string]$Path) {
  $client = New-Object System.Net.WebClient
  try { $client.DownloadFile($Url, $Path) } finally { $client.Dispose() }
}

function Get-Sha256([string]$Path) {
  $sha = [Security.Cryptography.SHA256]::Create()
  $stream = [IO.File]::OpenRead($Path)
  try {
    return ([BitConverter]::ToString($sha.ComputeHash($stream)) -replace "-", "").ToLowerInvariant()
  } finally {
    $stream.Dispose()
    $sha.Dispose()
  }
}

function Install-Runtime {
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
  New-Item -ItemType Directory -Force -Path $Root | Out-Null
  $staging = Join-Path $Root (".staging-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Path $staging | Out-Null
  try {
    $zip  = Join-Path $staging $Name
    $sums = Join-Path $staging "SHA256SUMS"
    $url  = "$BaseUrl/v$Version"
    Write-Log "Downloading $url/$Name"
    Save-Url "$url/SHA256SUMS" $sums
    Save-Url "$url/$Name" $zip
    $want = Get-Expected $sums $Name
    if (-not $want) { throw "SHA256SUMS of v$Version lists no $Name" }
    $got = Get-Sha256 $zip
    if ($got -ne $want) { throw "checksum mismatch for ${Name}: expected $want, got $got" }
    $unpacked = Join-Path $staging "unpacked"
    [IO.Compression.ZipFile]::ExtractToDirectory($zip, $unpacked)
    $hook = Get-ChildItem -Path $unpacked -Recurse -Filter "sonara-hook.exe" | Select-Object -First 1
    if (-not $hook) { throw "$Name holds no sonara-hook.exe" }
    if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }   # a broken earlier install
    [IO.Directory]::Move($hook.Directory.FullName, $Dest)
    Write-Log "Installed the Sonara runtime $Version in $Dest"
  } finally {
    Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
  }
}

$code = 0
try {
  if (-not (Test-Path (Join-Path $Dest "sonara-hook.exe"))) { Install-Runtime }
  Remove-Item -Force $Failed -ErrorAction SilentlyContinue
  $started = $true
  if (-not $NoStart -and $env:SONARA_BOOTSTRAP_START -ne "0") {
    # Native stderr lines are error records in Windows PowerShell: with
    # "Stop" in force the first one would end the script.
    $ErrorActionPreference = "Continue"
    $out = & (Join-Path $Dest "sonara.exe") start 2>&1
    $ErrorActionPreference = "Stop"
    foreach ($line in $out) { Write-Log "$line" }
    if ($LASTEXITCODE -ne 0) {
      Write-Log "sonara start exited with $LASTEXITCODE"
      $started = $false
    }
  }
  # Remove older releases once this one runs (a folder still in use stays
  # and the next `sonara start` removes it). Upgrades go one way: a newer
  # release's folder stays, since a session of an older plugin can run
  # this while a newer plugin's sessions use that runtime.
  $dirs = if ($started) { Get-ChildItem -Path $Root -Directory } else { @() }
  foreach ($old in $dirs) {
    if (-not (Test-OlderVersion $old.Name $Version)) { continue }
    try {
      Remove-Item -Recurse -Force $old.FullName
      Write-Log "Removed the old runtime $($old.Name)"
    } catch {
      Write-Log "Could not remove the old runtime $($old.Name) yet: $_"
    }
  }
} catch {
  $code = 1
  Write-Log "Sonara runtime $Version not installed: $_"
  try {
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    [IO.File]::WriteAllText($Failed, (Get-Date).ToString("o") + " $_")
  } catch { }
} finally {
  if ($Lock) { Remove-Item -Recurse -Force $Lock -ErrorAction SilentlyContinue }
}
exit $code

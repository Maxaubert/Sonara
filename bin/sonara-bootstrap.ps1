#Requires -Version 5
<#
  Sonara zero-prerequisite setup.
  Ensures a usable Python (provisioning a uv-managed CPython 3.12 if none is
  found), records the interpreter paths, then runs `sonara install`. PowerShell
  needs no Python, so this breaks the no-Python chicken-and-egg.
#>
$ErrorActionPreference = "Stop"

$SonaraDir  = Join-Path $env:USERPROFILE ".sonara"
$ToolsDir   = Join-Path $SonaraDir "tools"
$PluginRoot = Split-Path -Parent $PSScriptRoot          # ...\bin -> plugin root
$PySrc      = Join-Path $PluginRoot "src"
$UvVersion  = "0.11.23"                                  # pinned (Task 3 Step 1)
New-Item -ItemType Directory -Force -Path $SonaraDir | Out-Null

function Test-RealPython([string]$exe) {
  # True if $exe is a real CPython >= 3.9 (not a Microsoft Store stub).
  try { $real = & $exe -c "import sys; print(sys.executable)" 2>$null } catch { return $false }
  if (-not $real) { return $false }
  if ($real -match "WindowsApps") { return $false }      # Store stub
  try { $ok = & $exe -c "import sys; print(1 if sys.version_info[:2] >= (3,9) else 0)" 2>$null } catch { return $false }
  return ($ok -eq "1")
}

function Find-SystemPython {
  # Returns a console python.exe path, or $null. Prefers the py launcher.
  $cands = @()
  if (Get-Command py -ErrorAction SilentlyContinue) {
    $real = & py -3 -c "import sys; print(sys.executable)" 2>$null
    if ($real) { $cands += $real }
  }
  foreach ($n in @("python","python3")) {
    $c = Get-Command $n -ErrorAction SilentlyContinue
    if ($c) { $cands += $c.Source }
  }
  foreach ($c in $cands) { if (Test-RealPython $c) { return $c } }
  return $null
}

function Get-Uv {
  # Returns the path to uv.exe, downloading it to $ToolsDir if needed.
  $onPath = Get-Command uv -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  $local = Join-Path $ToolsDir "uv.exe"
  if (Test-Path $local) { return $local }
  New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null
  $zip = Join-Path $ToolsDir "uv.zip"
  $url = "https://github.com/astral-sh/uv/releases/download/$UvVersion/uv-x86_64-pc-windows-msvc.zip"
  Write-Host "Downloading uv $UvVersion..."
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
  Expand-Archive -Path $zip -DestinationPath $ToolsDir -Force
  Remove-Item $zip -Force
  if (-not (Test-Path $local)) { throw "uv.exe not found after extracting $url" }
  return $local
}

function Install-UvPython {
  # Installs a uv-managed CPython 3.12 and returns its python.exe path.
  $uv = Get-Uv
  Write-Host "Installing Python 3.12 via uv (this can take a minute)..."
  & $uv python install 3.12
  if ($LASTEXITCODE -ne 0) { throw "uv python install 3.12 failed" }
  $pyexe = & $uv python find 3.12 2>$null
  if (-not $pyexe -or -not (Test-Path $pyexe)) { throw "could not locate the uv-managed Python 3.12" }
  return $pyexe
}

# --- main -----------------------------------------------------------------
$python = Find-SystemPython
if (-not $python) {
  Write-Host "No usable Python found. Provisioning one for Sonara..."
  try {
    $python = Install-UvPython
  } catch {
    Write-Host "Could not provision Python automatically: $_"
    Write-Host "Install Python 3.9+ from https://www.python.org/downloads/windows/ and re-run /sonara:install."
    exit 1
  }
}

# Derive the windowless interpreter (pythonw.exe alongside python.exe).
$pythonw = Join-Path (Split-Path -Parent $python) "pythonw.exe"
if (-not (Test-Path $pythonw)) { $pythonw = $python }

# Record both for the shims + the daemon resolver.
Set-Content -Path (Join-Path $SonaraDir "python.path")  -Value $python  -NoNewline -Encoding ASCII
Set-Content -Path (Join-Path $SonaraDir "pythonw.path") -Value $pythonw -NoNewline -Encoding ASCII

# Hand off to the real installer under that interpreter.
$env:PYTHONPATH = $PySrc + ";" + $env:PYTHONPATH
& $python -m sonara.cli install
exit $LASTEXITCODE

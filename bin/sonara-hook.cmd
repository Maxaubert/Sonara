@echo off
rem Windows launcher for the Sonara plugin hook.
rem
rem Lets hooks/hooks.json's "${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook <Event>" resolve
rem to this .cmd on Windows (via PATHEXT) instead of the extensionless shebang
rem script, which cmd/PowerShell cannot execute directly. Runs the Python hook
rem entrypoint windowless (no console flash on every hook). The hook payload
rem arrives on stdin; sonara-hook self-bootstraps sys.path, so no PYTHONPATH.
rem
rem A hook must never fail LOUDLY (it would interrupt Claude Code), so we always
rem exit 0. But it must not fail SILENTLY either: a missing/wrong pythonw used to
rem mute Sonara with no trace (stderr went to nul). So we resolve a windowless
rem interpreter (pythonw, else the py launcher's windowless `pyw -3`) and append
rem stderr to ~/.sonara/hook.log for diagnosis instead of discarding it (M10).
setlocal
set "SONARA_DIR=%USERPROFILE%\.sonara"
set "SONARA_HOOK_LOG=%SONARA_DIR%\hook.log"
if not exist "%SONARA_DIR%\" mkdir "%SONARA_DIR%" >nul 2>nul

where pythonw >nul 2>nul
if %errorlevel%==0 (
  pythonw "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
) else (
  rem No pythonw on PATH: fall back to the windowless Python launcher.
  pyw -3 "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
)
exit /b 0

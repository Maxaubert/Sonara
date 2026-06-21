@echo off
rem Windows launcher for the Sonara plugin hook. Resolves a windowless interpreter
rem (pythonw, else pyw -3, else the recorded pythonw from /sonara:install) and
rem always exits 0 so a hook can never break the Claude session.
setlocal enabledelayedexpansion
set "SONARA_DIR=%USERPROFILE%\.sonara"
set "SONARA_HOOK_LOG=%SONARA_DIR%\hook.log"
if not exist "%SONARA_DIR%\" mkdir "%SONARA_DIR%" >nul 2>nul

where pythonw >nul 2>nul
if !errorlevel!==0 (
  pythonw "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
  exit /b 0
)
where pyw >nul 2>nul
if !errorlevel!==0 (
  pyw -3 "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
  exit /b 0
)
if exist "%SONARA_DIR%\pythonw.path" (
  set /p PW=<"%SONARA_DIR%\pythonw.path"
  "!PW!" "%~dp0sonara-hook" %* 2>>"%SONARA_HOOK_LOG%"
)
exit /b 0

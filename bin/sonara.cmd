@echo off
rem Windows launcher for the Sonara CLI. Uses the interpreter /sonara:install
rem recorded first (the one it validated), then a PATH python that is not the
rem WindowsApps Store alias, which runs nothing (E4), then the py launcher.
setlocal enabledelayedexpansion
set "PYTHONPATH=%~dp0..\src;%PYTHONPATH%"
rem Exit codes MUST be forwarded: `sonara doctor` returns 1 on a failed check and
rem a bare `sonara` returns 2, and callers branch on that. A plain `exit /b` here
rem returned 0 no matter what the CLI exited with, so every failure looked like a
rem success. !ERRORLEVEL! (not %ERRORLEVEL%) is required inside a parenthesised
rem block: %-expansion happens when the block is PARSED, i.e. before python runs.
set "PY="
set "REC=%USERPROFILE%\.sonara\python.path"
if exist "%REC%" (
  set /p PY=<"%REC%"
  if defined PY if not exist "!PY!" set "PY="
)
rem No usable record: a PATH python that is not the WindowsApps Store alias.
if not defined PY (
  for /f "delims=" %%P in ('where python 2^>nul') do (
    if not defined PY (
      echo %%P| findstr /i "WindowsApps" >nul || set "PY=%%P"
    )
  )
)
if defined PY (
  "!PY!" -m sonara.cli %*
  exit /b !ERRORLEVEL!
)
where py >nul 2>nul && ( py -3 -m sonara.cli %* & exit /b !ERRORLEVEL! )
echo No Python found. Run /sonara:install to set up Sonara.
exit /b 1

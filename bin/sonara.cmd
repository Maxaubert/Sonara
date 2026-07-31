@echo off
rem Windows launcher for the Sonara CLI. Prefers system python.exe; falls back to
rem the interpreter recorded by /sonara:install when none is on PATH (zero-Python).
setlocal enabledelayedexpansion
set "PYTHONPATH=%~dp0..\src;%PYTHONPATH%"
rem Exit codes MUST be forwarded: `sonara doctor` returns 1 on a failed check and
rem a bare `sonara` returns 2, and callers branch on that. A plain `exit /b` here
rem returned 0 no matter what the CLI exited with, so every failure looked like a
rem success. !ERRORLEVEL! (not %ERRORLEVEL%) is required inside a parenthesised
rem block: %-expansion happens when the block is PARSED, i.e. before python runs.
where python >nul 2>nul && ( python -m sonara.cli %* & exit /b !ERRORLEVEL! )
set "REC=%USERPROFILE%\.sonara\python.path"
if exist "%REC%" (
  set /p PY=<"%REC%"
  "!PY!" -m sonara.cli %*
  exit /b !ERRORLEVEL!
) else (
  echo No Python found. Run /sonara:install to set up Sonara.
  exit /b 1
)

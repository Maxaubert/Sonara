@echo off
rem Windows launcher for the Sonara CLI. Prefers system python.exe; falls back to
rem the interpreter recorded by /sonara:install when none is on PATH (zero-Python).
setlocal enabledelayedexpansion
set "PYTHONPATH=%~dp0..\src;%PYTHONPATH%"
where python >nul 2>nul && ( python -m sonara.cli %* & exit /b )
set "REC=%USERPROFILE%\.sonara\python.path"
if exist "%REC%" (
  set /p PY=<"%REC%"
  "!PY!" -m sonara.cli %*
) else (
  echo No Python found. Run /sonara:install to set up Sonara.
  exit /b 1
)

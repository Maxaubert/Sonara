@echo off
rem Windows launcher for the Sonara CLI (mirrors the macOS bin/sonara bash script).
rem Runs the plugin's own src with no installed 'sonara' on PATH. Uses python.exe
rem (console) so subcommands like `status` can print their output.
setlocal
set "PYTHONPATH=%~dp0..\src;%PYTHONPATH%"
python -m sonara.cli %*

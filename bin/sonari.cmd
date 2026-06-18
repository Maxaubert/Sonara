@echo off
rem Windows launcher for the Sonari CLI (mirrors the macOS bin/sonari bash script).
rem Runs the plugin's own src with no installed 'sonari' on PATH. Uses python.exe
rem (console) so subcommands like `status` can print their output.
setlocal
set "PYTHONPATH=%~dp0..\src;%PYTHONPATH%"
python -m sonari.cli %*

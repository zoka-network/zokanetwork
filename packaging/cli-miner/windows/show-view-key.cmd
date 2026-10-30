@echo off
rem Zoka Quantic CLI miner: view key
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" view-key %*
echo.
pause

@echo off
rem Zoka Quantic CLI miner: status
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" status %*
echo.
pause

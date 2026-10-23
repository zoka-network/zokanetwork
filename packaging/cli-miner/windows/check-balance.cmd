@echo off
rem Zoka Quantic CLI miner: balance
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" balance %*
echo.
pause

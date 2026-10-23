@echo off
rem Zoka Quantic CLI miner: mine
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" mine %*
echo.
pause

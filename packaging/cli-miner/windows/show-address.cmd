@echo off
rem Zoka Quantic CLI miner: address
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" address %*
echo.
pause

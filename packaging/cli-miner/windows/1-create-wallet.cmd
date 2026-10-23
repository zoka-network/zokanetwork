@echo off
rem Zoka Quantic CLI miner: create-wallet
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" create-wallet %*
echo.
pause

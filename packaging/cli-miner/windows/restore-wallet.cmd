@echo off
rem Zoka Quantic CLI miner: restore-wallet
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" restore-wallet %*
echo.
pause

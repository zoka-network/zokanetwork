@echo off
rem Zoka Quantic CLI miner: stop starting mining with Windows
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" autostart -Off %*
echo.
pause

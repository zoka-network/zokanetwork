@echo off
rem Zoka Quantic CLI miner: start mining when you sign in to Windows
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" autostart %*
echo.
pause

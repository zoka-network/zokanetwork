@echo off
rem Zoka Quantic CLI miner: node ids
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" node-id %*
echo.
pause

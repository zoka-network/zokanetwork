@echo off
rem Zoka Quantic CLI miner: xmrig-node
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" xmrig-node %*
echo.
pause

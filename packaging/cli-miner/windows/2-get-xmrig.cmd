@echo off
rem Zoka Quantic CLI miner: download XMRig 6.25.0 from its official releases and check its SHA-256
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0zoka-miner.ps1" get-xmrig %*
echo.
pause

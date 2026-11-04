@echo off
setlocal
cd /d "%~dp0"

:: Check for administrative privileges
net session >nul 2>&1
if %errorlevel% == 0 (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Uninstall.ps1" %*
) else (
    echo Elevating permissions with UAC...
    powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "Start-Process cmd.exe -ArgumentList '/c \"\"%~dp0Uninstall.cmd\" %*\"' -Verb RunAs"
)
exit /b %errorlevel%

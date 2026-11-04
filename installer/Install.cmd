@echo off
setlocal
cd /d "%~dp0"

:: Check for administrative privileges
net session >nul 2>&1
if %errorlevel% == 0 (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install.ps1" %*
) else (
    echo Elevating permissions with UAC...
    powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "Start-Process cmd.exe -ArgumentList '/c \"\"%~dp0Install.cmd\" %*\"' -Verb RunAs"
)
exit /b %errorlevel%

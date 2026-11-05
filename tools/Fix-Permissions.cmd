@echo off
setlocal
echo Requesting administrative privileges...
net session >nul 2>&1
if %errorLevel% neq 0 (
    powershell -Command "Start-Process cmd -ArgumentList '/c %~s0' -Verb RunAs"
    exit /b
)

echo Fixing Sumibi permissions...
icacls "C:\Program Files\Sumibi" /reset /t
icacls "C:\Program Files\Sumibi" /grant "*S-1-5-32-545:(OI)(CI)RX" /t
icacls "C:\Program Files\Sumibi" /grant "*S-1-15-2-1:(OI)(CI)RX" /t

echo.
echo Permissions fixed successfully.
pause

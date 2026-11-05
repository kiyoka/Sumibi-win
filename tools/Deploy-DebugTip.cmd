@echo off
setlocal enabledelayedexpansion
echo Requesting administrative privileges...
net session >nul 2>&1
if %errorLevel% neq 0 (
    powershell -Command "Start-Process cmd -ArgumentList '/c %~s0' -Verb RunAs"
    exit /b
)

:: Generate unique directory based on timestamp
set TIMESTAMP=%DATE:~0,4%%DATE:~5,2%%DATE:~8,2%_%TIME:~0,2%%TIME:~3,2%%TIME:~6,2%
set TIMESTAMP=%TIMESTAMP: =0%
set TARGET_DIR=C:\Program Files\Sumibi\versions\0.1.0-run-%TIMESTAMP%
set TARGET_DLL=%TARGET_DIR%\sumibi_tip.dll

echo Target: %TARGET_DLL%
if not exist "%TARGET_DIR%" mkdir "%TARGET_DIR%"

echo Copying newly built sumibi_tip.dll to unique version folder...
copy /y "C:\dev\Sumibi-win\target\release\sumibi_tip.dll" "%TARGET_DLL%"
if %errorLevel% neq 0 (
    echo [ERROR] Failed to copy DLL.
    pause
    exit /b %errorLevel%
)

echo Granting ALL APPLICATION PACKAGES access...
icacls "%TARGET_DLL%" /grant "*S-1-15-2-1:(RX)" /q

echo Updating COM InprocServer32 registry to point to new unique DLL...
reg add "HKLM\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\InprocServer32" /ve /t REG_SZ /d "%TARGET_DLL%" /f
reg add "HKCU\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\InprocServer32" /ve /t REG_SZ /d "%TARGET_DLL%" /f

echo Stopping notepad processes so new notepad will load new DLL...
taskkill /F /IM notepad.exe >nul 2>&1

echo.
echo ========================================================
echo Diagnostic DLL deployed successfully to:
echo %TARGET_DLL%
echo ========================================================
pause

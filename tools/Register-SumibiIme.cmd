@echo off
setlocal
echo Requesting administrative privileges...
net session >nul 2>&1
if %errorLevel% neq 0 (
    powershell -Command "Start-Process cmd -ArgumentList '/c %~s0' -Verb RunAs"
    exit /b
)

echo Registering Sumibi IME (System Scope)...
"C:\Program Files\Sumibi\bin\sumibi-setup.exe" install --dll "C:\Program Files\Sumibi\versions\0.1.0\sumibi_tip.dll" --scope system

echo Removing obsolete HKCU overrides if any...
reg delete "HKCU\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}" /f >nul 2>&1

echo Enabling Sumibi in User Language List...
powershell -ExecutionPolicy Bypass -File "%~dp0Enable-SumibiUserLanguage.ps1"

echo.
echo ========================================================
echo Sumibi IME registration complete!
echo ========================================================
pause

@echo off
setlocal
echo Requesting administrative privileges...
net session >nul 2>&1
if %errorLevel% neq 0 (
    powershell -Command "Start-Process cmd -ArgumentList '/c %~s0' -Verb RunAs"
    exit /b
)

echo Setting clean DLL path in COM registry (removing \\?\ prefix)...
reg add "HKLM\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\InprocServer32" /ve /t REG_SZ /d "C:\Program Files\Sumibi\versions\0.1.0\sumibi_tip.dll" /f
reg add "HKCU\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\InprocServer32" /ve /t REG_SZ /d "C:\Program Files\Sumibi\versions\0.1.0\sumibi_tip.dll" /f

echo.
echo COM registry updated to: C:\Program Files\Sumibi\versions\0.1.0\sumibi_tip.dll
echo.
pause

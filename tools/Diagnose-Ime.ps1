Write-Host "================ Sumibi IME Diagnostic ================" -ForegroundColor Cyan

# 1. Check running notepad processes
$notepads = Get-Process -Name notepad -ErrorAction SilentlyContinue
if ($notepads) {
    Write-Host "[OK] Notepad is running (PIDs: $($notepads.Id -join ', '))" -ForegroundColor Green
    foreach ($p in $notepads) {
        $mods = $p.Modules | Where-Object { $_.ModuleName -like "*sumibi*" }
        if ($mods) {
            foreach ($m in $mods) {
                Write-Host "  -> Sumibi DLL IS LOADED: $($m.FileName)" -ForegroundColor Green
            }
        } else {
            Write-Host "  -> sumibi_tip.dll is NOT loaded in Notepad PID $($p.Id)" -ForegroundColor Yellow
        }
    }
} else {
    Write-Host "[WARN] Notepad is NOT currently running. Please open Notepad." -ForegroundColor Yellow
}

# 2. Check ctfmon (TSF manager)
$ctfmon = Get-Process -Name ctfmon -ErrorAction SilentlyContinue
if ($ctfmon) {
    Write-Host "[OK] ctfmon.exe is running (PIDs: $($ctfmon.Id -join ', '))" -ForegroundColor Green
    foreach ($p in $ctfmon) {
        $mods = $p.Modules | Where-Object { $_.ModuleName -like "*sumibi*" }
        if ($mods) {
            Write-Host "  -> Sumibi DLL is loaded in ctfmon" -ForegroundColor Green
        }
    }
} else {
    Write-Host "[WARN] ctfmon.exe is NOT running!" -ForegroundColor Red
}

# 3. Check Language List
Write-Host "`n=== WinUserLanguageList InputMethodTips ===" -ForegroundColor Cyan
$langs = Get-WinUserLanguageList
foreach ($l in $langs) {
    Write-Host "Language: $($l.LanguageTag)"
    foreach ($tip in $l.InputMethodTips) {
        if ($tip -like "*E927D4AB*") {
            Write-Host "  [FOUND SUMIBI] $tip" -ForegroundColor Green
        } else {
            Write-Host "  $tip"
        }
    }
}

# 4. Check COM InprocServer32
Write-Host "`n=== Registry Path Check ===" -ForegroundColor Cyan
$hklm = (Get-ItemProperty 'HKLM:\Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\InprocServer32' -ErrorAction SilentlyContinue).'(default)'
Write-Host "HKLM InprocServer32: $hklm"
if (Test-Path $hklm) {
    Write-Host "  -> File exists on disk: YES" -ForegroundColor Green
} else {
    Write-Host "  -> File exists on disk: NO ($hklm)" -ForegroundColor Red
}

# 5. Check TSF Registration in HKLM
$tsfKey = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\LanguageProfile\0x00000411\{166150AD-12EF-4F52-A75B-CBB19FF97D27}"
if (Test-Path $tsfKey) {
    $desc = (Get-ItemProperty $tsfKey -ErrorAction SilentlyContinue).Description
    $enable = (Get-ItemProperty $tsfKey -ErrorAction SilentlyContinue).Enable
    Write-Host "TSF TIP registered: YES (Description: $desc, Enable: $enable)" -ForegroundColor Green
} else {
    Write-Host "TSF TIP registered: NO (Key not found)" -ForegroundColor Red
}

Write-Host "========================================================" -ForegroundColor Cyan

$tipId = "0411:{E927D4AB-2EC9-4DD3-A921-A4620B95A410}{166150AD-12EF-4F52-A75B-CBB19FF97D27}"
$list = Get-WinUserLanguageList
$ja = $list | Where-Object { $_.LanguageTag -like "ja*" }
if ($ja) {
    if (-not ($ja.InputMethodTips -contains $tipId)) {
        $ja.InputMethodTips.Add($tipId)
        Set-WinUserLanguageList $list -Force
        Write-Host "SUCCESS: Added Sumibi TIP ($tipId) to Windows user language list!" -ForegroundColor Green
    } else {
        Write-Host "Sumibi TIP is already present in Windows user language list." -ForegroundColor Yellow
    }
} else {
    Write-Warning "Japanese language (ja) not found in WinUserLanguageList."
}

# Sumibi app icon

`AppIcon.png` is the unmodified [Sumibi-mac artwork](https://github.com/kiyoka/Sumibi-mac/blob/a44e33d7fd98d2492ecb580c24c5eb1e9c531d89/App/Resources/AppIcon.png) by Kiyoka Nishiyama, covered by the repository's MIT license.

After changing the source image, run `powershell.exe -NoProfile -File tools/New-SumibiIcon.ps1` from the workspace root to regenerate `Sumibi.ico`. The ICO contains 16 through 256 pixel frames. `Sumibi.rc` embeds the icon in `sumibi-host.exe`, where the shell, Host windows, and tray can all load it.

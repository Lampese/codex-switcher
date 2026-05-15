param(
  [switch]$NoLaunch
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$installDir = Join-Path $env:LOCALAPPDATA "Codex Switcher"
$releaseDir = Join-Path $repoRoot "src-tauri\target\release"
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"

function New-Shortcut {
  param(
    [string]$ShortcutPath,
    [string]$TargetPath,
    [string]$WorkingDirectory
  )

  $parent = Split-Path -Parent $ShortcutPath
  New-Item -ItemType Directory -Path $parent -Force | Out-Null

  $shell = New-Object -ComObject WScript.Shell
  $shortcut = $shell.CreateShortcut($ShortcutPath)
  $shortcut.TargetPath = $TargetPath
  $shortcut.WorkingDirectory = $WorkingDirectory
  $shortcut.IconLocation = $TargetPath
  $shortcut.Save()
}

Push-Location $repoRoot
try {
  Get-Process -Name codex-switcher,codex-web -ErrorAction SilentlyContinue |
    Stop-Process -Force

  pnpm exec tauri build --no-bundle

  New-Item -ItemType Directory -Path $installDir -Force | Out-Null

  foreach ($name in @("codex-switcher.exe", "codex-web.exe")) {
    $source = Join-Path $releaseDir $name
    $target = Join-Path $installDir $name

    if (!(Test-Path -LiteralPath $source)) {
      throw "Missing release binary: $source"
    }

    if (Test-Path -LiteralPath $target) {
      Copy-Item -LiteralPath $target -Destination "$target.bak-$timestamp" -Force
    }

    Copy-Item -LiteralPath $source -Destination $target -Force
    $item = Get-Item -LiteralPath $target
    Write-Host "Installed $($item.FullName) ($($item.Length) bytes)"
  }

  $exe = Join-Path $installDir "codex-switcher.exe"
  $desktopShortcut = Join-Path ([Environment]::GetFolderPath("DesktopDirectory")) "Codex Switcher.lnk"
  $startMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Codex Switcher.lnk"

  New-Shortcut -ShortcutPath $desktopShortcut -TargetPath $exe -WorkingDirectory $installDir
  New-Shortcut -ShortcutPath $startMenuShortcut -TargetPath $exe -WorkingDirectory $installDir

  Write-Host "Updated shortcuts:"
  Write-Host "- $desktopShortcut"
  Write-Host "- $startMenuShortcut"

  if (!$NoLaunch) {
    $process = Start-Process -FilePath $exe -PassThru
    Start-Sleep -Seconds 2
    $running = Get-Process -Id $process.Id
    Write-Host "Launched $($running.ProcessName) PID $($running.Id)"
  }
}
finally {
  Pop-Location
}

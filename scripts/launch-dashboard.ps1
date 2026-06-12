param(
  [switch]$Server
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$LocalDir = Join-Path $RepoRoot ".codex-local"
$HostName = "127.0.0.1"
$Port = 3210
$Url = "http://${HostName}:${Port}/"
$HealthUrl = "http://${HostName}:${Port}/api/health"

New-Item -ItemType Directory -Force -Path $LocalDir | Out-Null

if ($Server) {
  Set-Location $RepoRoot
  pnpm lan
  exit $LASTEXITCODE
}

function Test-DashboardHealth {
  try {
    $response = Invoke-WebRequest -Uri $HealthUrl -UseBasicParsing -TimeoutSec 2
    return $response.StatusCode -eq 200
  } catch {
    return $false
  }
}

function Find-AppBrowser {
  $candidates = @(
    (Get-Command chrome.exe -ErrorAction SilentlyContinue).Source,
    (Get-Command msedge.exe -ErrorAction SilentlyContinue).Source,
    "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
    "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe",
    "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
  )

  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path $candidate)) {
      return $candidate
    }
  }

  return $null
}

if (-not (Test-DashboardHealth)) {
  if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    throw "pnpm was not found on PATH. Install pnpm or run this from a shell where pnpm works."
  }

  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found on PATH. Install Rust or run this from a shell where cargo works."
  }

  $outLog = Join-Path $LocalDir "dashboard.out.log"
  $errLog = Join-Path $LocalDir "dashboard.err.log"

  Start-Process `
    -FilePath "powershell.exe" `
    -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $PSCommandPath, "-Server") `
    -WorkingDirectory $RepoRoot `
    -WindowStyle Hidden `
    -RedirectStandardOutput $outLog `
    -RedirectStandardError $errLog | Out-Null

  $deadline = (Get-Date).AddSeconds(90)
  while ((Get-Date) -lt $deadline) {
    if (Test-DashboardHealth) {
      break
    }
    Start-Sleep -Milliseconds 500
  }

  if (-not (Test-DashboardHealth)) {
    throw "Dashboard did not become healthy. Check $outLog and $errLog."
  }
}

$browser = Find-AppBrowser
if ($browser) {
  Start-Process -FilePath $browser -ArgumentList @("--app=$Url", "--new-window")
} else {
  Start-Process $Url
}

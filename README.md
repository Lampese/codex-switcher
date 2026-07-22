<p align="center">
  <img src="src-tauri/icons/logo.svg" alt="Codex Switcher" width="128" height="128">
</p>

<h1 align="center">Codex Switcher</h1>

<p align="center">
  A Desktop Application for Managing Multiple OpenAI <a href="https://github.com/openai/codex">Codex</a> Accounts<br>
  Easily switch between accounts, monitor usage, schedule warm-ups, and stay in control of your quota
</p>

## Features

- **Multi-Account Management** – Add, rename, mask, import, export, and manage multiple Codex accounts in one place
- **Quick Switching** – Switch between accounts from the main window, native tray menu, or tray popup
- **Usage Stats** – View account usage stats for OAuth accounts, including lifetime tokens, daily buckets, streaks, activity insights, and top integrations
- **Manual Reset Credits** – See available manual reset credits beside each account plan badge, with the closest expiry highlighted as it approaches
- **Automatic Warm-Up** – Warm up one account or all accounts manually, after each 5-hour reset window, or at specific scheduled times of day
- **System Tray Controls** – Use the tray popup to switch accounts, inspect quota and active-account stats, refresh usage, open the main window, or quit the app
- **Tray Display Modes** – Choose between the app icon with session percentage, a text-only hourly/weekly percentage display, or a hidden tray icon
- **macOS Dock Control** – Keep Codex Switcher in the Dock or run it as a menu bar only app, with a first-close prompt and a tray fallback
- **Rate-Limit Monitoring** – View real-time 5-hour session and weekly usage, reset timing, credits, and subscription expiry
- **Blocked Switch Recovery** – Detect running Codex sessions and offer a force-close flow before retrying the account switch
- **Dual Login Mode** – Authenticate with ChatGPT OAuth or import existing `auth.json` files

## Installation

### Download a Release

The easiest way to install Codex Switcher is from the latest GitHub release:

[Download the latest release](https://github.com/Lampese/codex-switcher/releases/latest)

Choose the file for your platform:

- **macOS Apple Silicon:** `Codex.Switcher_*_aarch64.dmg`
- **macOS Intel:** `Codex.Switcher_*_x64.dmg`
- **Windows:** `Codex.Switcher_*_x64-setup.exe` or `Codex.Switcher_*_x64_en-US.msi`
- **Linux Debian/Ubuntu:** `Codex.Switcher_*_amd64.deb`
- **Linux AppImage:** `Codex.Switcher_*_amd64.AppImage`
- **Linux RPM:** `Codex.Switcher-*-1.x86_64.rpm`

> **macOS:** current release builds are not Apple-notarized. If macOS says the
> app is damaged, move it to `/Applications` and remove the quarantine flag:
>
> ```bash
> sudo xattr -dr com.apple.quarantine "/Applications/Codex Switcher.app"
> open "/Applications/Codex Switcher.app"
> ```

### Auto Updates

Codex Switcher checks the latest GitHub release on startup. When a newer signed
update package is available, the app shows an update prompt and can install it
from inside the app.

### Build from Source

#### Prerequisites

- [Node.js](https://nodejs.org/) (v18+)
- [pnpm](https://pnpm.io/)
- [Rust](https://rustup.rs/)

```bash
# Clone the repository
git clone https://github.com/Lampese/codex-switcher.git
cd codex-switcher

# Install dependencies
pnpm install

# Run in development mode
pnpm tauri dev

# Build for production
pnpm tauri build
```

> **Windows:** the `pnpm tauri` script runs through a POSIX shell wrapper
> (`sh ./scripts/tauri.sh`) and will not work in PowerShell/CMD. Use the
> `tauri:win` script instead: `pnpm tauri:win dev` and `pnpm tauri:win build`.

The built application will be in `src-tauri/target/release/bundle/`.

### Run the Dashboard in a Browser

You can also serve the built dashboard over HTTP instead of opening the Tauri shell.

```bash
# Build the frontend and start the web server on 0.0.0.0:3210
pnpm lan
```

Optional environment variables:

- `CODEX_SWITCHER_WEB_HOST` to override the bind host
- `CODEX_SWITCHER_WEB_PORT` to override the port

The browser dashboard serves the same UI and backend actions through `/api/invoke/*`, which makes it usable over LAN, Tailscale, or a remote host tunnel when you expose the chosen port safely.

## Usage and Reset Credits

Codex Switcher shows two kinds of account usage information:

- **Rate limits** – the account card shows the current 5-hour and weekly limit
  windows, remaining percentage, reset timing, credit balance, and subscription
  expiry when available.
- **Usage Stats** – ChatGPT OAuth accounts can expand the **Usage
  Stats** panel to view stats such as lifetime tokens,
  today, last 7 days, last 30 days, streaks, longest task, token activity,
  reasoning/activity insights, and most-used integrations. The active account
  opens this panel by default; other accounts keep it collapsed until needed.
- **Manual reset credits** – OAuth accounts with available reset credits show a
  compact badge next to the plan badge. It includes the available count and the
  closest expiry date, hides zero-count results, and turns amber within 10 days
  or red within 3 days of expiry.

The tray popup also includes compact active-account stats for today and
the last 7 days, while keeping the normal rate-limit refresh flow separate.

## macOS Dock and Menu Bar Mode

On macOS, Codex Switcher can either stay visible in the Dock or live only in the
menu bar. The first time you close the main window, the app asks which behavior
you want and lets you choose whether to show that prompt again.

You can change the same setting later from the tray popup or from the native
tray menu under **Dock Icon**. If you choose **Menu Bar Only**, the app keeps a
visible tray item so you can always reopen the main window or switch back to
Dock mode.

## Warm-Up

A warm-up sends one minimal request to an account so its current usage window
has activity before you need it.

- **Manual** – warm up a single or all accounts, from the main window or tray menu.
- **Automatic** – when enabled (per account or for all), the app tracks the
  5-hour window when available and warms it after each reset, as long as the
  weekly limit isn't exhausted. If only the weekly window is available, it
  warms once after the weekly reset and automatically returns to the 5-hour
  schedule if that window reappears.
- **Timed** – pick specific times of day (e.g. `08:00`, `13:00`, `18:00`) from
  the **Timed** control in the main window. At each time the app warms all
  accounts (skipping any whose weekly limit is exhausted), so you control when
  your 5-hour windows start instead of letting them drift.

Timed warm-up checks the schedule every 30 seconds, runs each configured minute
only once per day, and skips missed times if the machine was asleep instead of
warming accounts late.

On macOS you can keep the machine awake with the built-in `caffeinate` command,
which stops automatically when the app quits:

```bash
caffeinate -i -w "$(pgrep -x 'Codex Switcher')"
```

## Disclaimer

This tool is designed **exclusively for individuals who personally own multiple OpenAI/ChatGPT accounts**. It is intended to help users manage their own accounts more conveniently.

**This tool is NOT intended for:**

- Sharing accounts between multiple users
- Circumventing OpenAI's terms of service
- Any form of account pooling or credential sharing

By using this software, you agree that you are the rightful owner of all accounts you add to the application. The authors are not responsible for any misuse or violations of OpenAI's terms of service.

## Attribution

This project is based on [Lampese/codex-switcher](https://github.com/Lampese/codex-switcher).
Original author: Lampese.

## Versioning

Use the version bump helper to keep app versions in sync across Tauri, Cargo, and the frontend.

```bash
# Exact version
pnpm version:bump 0.2.1

# Semver bumps
pnpm version:patch
pnpm version:minor
pnpm version:major

# Prepare a release commit and tag
# This automatically runs the version bump first.
pnpm release patch

# Prepare and push a release
# This automatically runs the version bump first.
pnpm release patch -- --push
```

<p align="center">
  <img src="src-tauri/icons/logo.svg" alt="Codex Switcher" width="128" height="128">
</p>

<h1 align="center">Codex Switcher</h1>

<p align="center">
  A Desktop Application for Managing Multiple OpenAI <a href="https://github.com/openai/codex">Codex</a> Accounts<br>
  Easily switch between accounts, monitor usage limits, and stay in control of your quota
</p>

## Features

- **Multi-Account Management** – Add, rename, mask, import, export, and manage multiple Codex accounts in one place
- **Quick Switching** – Switch between accounts from the main window, native tray menu, or tray popup
- **Automatic Warm-Up** – Warm up one account or all accounts manually, or keep accounts fresh with automatic warm-up scheduling
- **System Tray Controls** – Use the tray popup to switch accounts, inspect quota, refresh usage, open the main window, or quit the app
- **Tray Display Modes** – Choose between the app icon with session percentage, a text-only hourly/weekly percentage display, or a hidden tray icon
- **Usage Monitoring** – View real-time 5-hour session and weekly usage, reset timing, credits, and subscription expiry
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

### Run the Interactive CLI

An interactive command-line interface is available for fast account switching and status checks directly from your terminal.

```bash
# Run the interactive CLI
pnpm cli
```

Alternatively, you can run it directly with Cargo:
```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin codex-switch --
```

**CLI Features:**
- **Interactive Menu**: Navigate with arrow keys (or `j`/`k`) and `Enter` to switch, add, rename, or delete accounts.
- **Real-Time Quota Display**: Displays progress bars for 5-hour and weekly usage limits, and shows remaining balance/credits.
- **OAuth Login flow**: Add new accounts by triggering a login URL which opens in your default browser, then completes securely back in the CLI.
- **Background Auto-Refresh**: Periodically updates account quotas in the background (frequency configured in the config directory's `cli-config.json`, defaults to 60 seconds). You can also press `r` to trigger a manual refresh.

## Disclaimer

This tool is designed **exclusively for individuals who personally own multiple OpenAI/ChatGPT accounts**. It is intended to help users manage their own accounts more conveniently.

**This tool is NOT intended for:**

- Sharing accounts between multiple users
- Circumventing OpenAI's terms of service
- Any form of account pooling or credential sharing

By using this software, you agree that you are the rightful owner of all accounts you add to the application. The authors are not responsible for any misuse or violations of OpenAI's terms of service.

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

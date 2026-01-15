<p align="center">
  <img src="src-tauri/icons/logo.svg" alt="Codex Switcher" width="128" height="128">
</p>

<h1 align="center">Codex Switcher</h1>

<p align="center">
  A Desktop Application for Managing Multiple OpenAI <a href="https://github.com/openai/codex">Codex CLI</a> Accounts<br>
  Easily switch between accounts, monitor usage limits, and stay in control of your quota
</p>

## Features

- **Multi-Account Management** – Add and manage multiple Codex accounts in one place
- **Quick Switching** – Switch between accounts with a single click
- **Usage Monitoring** – View real-time usage for both 5-hour and weekly limits
- **Dual Login Mode** – OAuth authentication or import existing `auth.json` files

## Installation

### Prerequisites

- [Node.js](https://nodejs.org/) (v18+)
- [pnpm](https://pnpm.io/)
- [Rust](https://rustup.rs/)

### Build from Source

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

The built application will be in `src-tauri/target/release/bundle/`.

## Disclaimer

This tool is designed **exclusively for individuals who personally own multiple OpenAI/ChatGPT accounts**. It is intended to help users manage their own accounts more conveniently.

**This tool is NOT intended for:**
- Sharing accounts between multiple users
- Circumventing OpenAI's terms of service
- Any form of account pooling or credential sharing

By using this software, you agree that you are the rightful owner of all accounts you add to the application. The authors are not responsible for any misuse or violations of OpenAI's terms of service.

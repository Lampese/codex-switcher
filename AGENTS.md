# Repository Guidelines

## Project Structure & Module Organization

Codex Switcher is a Tauri v2 desktop app with a React/TypeScript frontend and Rust backend.

- `src/` contains the Vite/React UI. Key areas include `components/`, `hooks/`, `lib/`, and `types/`.
- `src-tauri/src/` contains Rust backend code. Commands live in `commands/`, account persistence and switching in `auth/`, API calls in `api/`, and tray/app menu logic in `tray.rs` and `app_menu.rs`.
- `src-tauri/icons/` stores app icons. Built frontend output goes to `dist/`.
- `scripts/` contains versioning, release, and Tauri wrapper scripts.
- Rust tests are colocated in `#[cfg(test)]` modules inside `src-tauri/src/**`.

## Build, Test, and Development Commands

- `pnpm install` installs frontend dependencies.
- `pnpm tauri dev` runs the desktop app in development mode.
- `pnpm build` runs TypeScript checking and builds the Vite frontend.
- `cargo test --manifest-path src-tauri/Cargo.toml` runs Rust unit tests.
- `pnpm lan` builds the frontend and serves the LAN dashboard through the `codex-web` binary.
- `pnpm tauri build` builds production bundles/installers.

On Windows, use `pnpm tauri:win` instead of the POSIX wrapper.

## Coding Style & Naming Conventions

Use idiomatic Rust and TypeScript. Keep Rust modules focused by domain, and keep React components small enough to scan. Use `cargo fmt --manifest-path src-tauri/Cargo.toml` for Rust formatting. Frontend formatting follows the existing TypeScript/React style: 2-space indentation, PascalCase components, camelCase variables/functions, and explicit shared types in `src/types/`.

## Testing Guidelines

Prefer focused Rust unit tests near the code being changed. For storage, auth, token handling, and process logic, add regression tests for the exact failure mode. There is no frontend test runner configured; use `pnpm build` as the required frontend verification. For risky UI or app-flow changes, manually run `pnpm tauri dev`.

## Commit & Pull Request Guidelines

Git history uses short conventional-style messages, for example `fix: write auth files atomically` or `feat: add Codex access token accounts`. Keep commits scoped and descriptive.

Pull requests should include:

- Summary of behavior changed.
- Test plan with commands run.
- Screenshots for visible UI changes.
- Notes for auth, storage, updater, or release-impacting changes.

## Security & Configuration Tips

Never commit real tokens, account exports, `CODEX_ACCESS_TOKEN` values, or local credential files. `~/.codex-switcher/accounts.json` and `~/.codex/auth.json` contain secrets. When changing account persistence, preserve atomic writes and restrictive file permissions.

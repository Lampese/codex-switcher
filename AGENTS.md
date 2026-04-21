# Repository Guidelines

## Project Structure & Module Organization

This is a Tauri 2 desktop app with a React 19 + TypeScript frontend and Rust backend. Frontend code lives in `src/`: `components/` for UI, `hooks/` for React state logic, `lib/platform.ts` for Tauri/web backend calls, and `types/` for shared TypeScript shapes. Rust code lives in `src-tauri/src/`, grouped by `auth/`, `api/`, `commands/`, and `web.rs`. Icons and platform assets are under `src-tauri/icons/`; release/version helpers are in `scripts/`.

## Build, Test, and Development Commands

- `pnpm install`: install JavaScript dependencies from `pnpm-lock.yaml`.
- `pnpm tauri dev`: run the desktop app in development mode.
- `pnpm build`: run TypeScript checks and build the Vite frontend.
- `pnpm tauri build`: build the production desktop bundles under `src-tauri/target/release/bundle/`.
- `pnpm lan`: build the frontend and run the Rust web dashboard on `0.0.0.0:3210`.
- `cargo test --manifest-path src-tauri/Cargo.toml`: run Rust tests when backend tests are added.

## Coding Style & Naming Conventions

Use TypeScript strict mode as enforced by `tsconfig.json`: no unused locals or parameters, no implicit type looseness, and JSX via `react-jsx`. Match existing formatting: two-space indentation, double-quoted imports, semicolons, and named exports for components. Name React components in `PascalCase`, hooks as `useSomething`, and shared types/interfaces descriptively. For Rust, follow `rustfmt`, use `snake_case` for modules/functions, and keep Tauri command handlers in `src-tauri/src/commands/`.

## Testing Guidelines

There is no dedicated frontend test runner configured yet, so treat `pnpm build` as required validation for TypeScript and Vite changes. For backend logic, add focused Rust unit tests near the module under test or integration tests under `src-tauri/tests/`. Name tests by behavior, for example `refreshes_expired_token`. Manually verify UI changes with `pnpm tauri dev`; include platform checks when changing Tauri config, updater behavior, process handling, or file dialogs.

## Commit & Pull Request Guidelines

Recent history uses short imperative subjects, with Conventional-style prefixes for release chores, for example `chore: release 0.2.2` and `Add release script`. Keep commits focused and describe the user-visible change or maintenance task. Pull requests should include a summary, validation commands, linked issues when applicable, and screenshots or recordings for UI changes. For Tauri/Rust changes, note tested platforms and relevant environment variables such as `CODEX_SWITCHER_WEB_HOST` or `CODEX_SWITCHER_WEB_PORT`.

## Security & Configuration Tips

This app manages Codex account data. Do not commit `auth.json`, decrypted exports, local backups, tokens, or machine-specific secrets. Prefer encrypted full exports for backups, and document new configuration in `README.md`.

<!-- chinese-language-config:start -->
## Language
Use **Chinese** for:
- Task execution results and error messages
- Confirmations and clarifications with the user
- Solution descriptions and to-do items
- Commit info for git
<!-- chinese-language-config:end -->

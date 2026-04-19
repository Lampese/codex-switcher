# Build AppImage (Codex Switcher)

This document explains how to build a Linux AppImage for this repository.

## Prerequisites

- Node.js 18+
- pnpm
- Rust (cargo/rustup)
- Linux build environment

## Build Command

From the repository root:

```bash
cd ~/codex-switcher
pnpm install
pnpm tauri build --bundles appimage --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

Why the `--config` override is used:

- `src-tauri/tauri.conf.json` enables updater artifacts.
- If `TAURI_SIGNING_PRIVATE_KEY` is not set, Tauri can fail at signing even though the AppImage was already built.
- This override disables updater-artifact generation for local AppImage builds.

## Output Location

Generated AppImage path pattern:

```text
~/codex-switcher/src-tauri/target/release/bundle/appimage/Codex Switcher_<version>_amd64.AppImage
```

Current example file:

```text
~/codex-switcher/src-tauri/target/release/bundle/appimage/Codex Switcher_0.1.7_amd64.AppImage
```

## Run the AppImage

```bash
"$HOME/codex-switcher/src-tauri/target/release/bundle/appimage/Codex Switcher_0.1.7_amd64.AppImage"
```

# Codex Access Token Account Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add accounts from a pasted `CODEX_ACCESS_TOKEN`, switch to them using `codex login --with-access-token`, and fetch K12/Codex usage/metadata in the same shape Codex CLI uses.

**Architecture:** Store access-token accounts as a dedicated auth mode so existing OAuth and `auth.json` behavior stays unchanged. Parse JWT claims for display metadata, delegate actual login materialization to the Codex CLI via stdin during account switch, and use Codex's AgentIdentity request signing for access-token usage reads.

**Tech Stack:** Rust/Tauri backend, React/TypeScript frontend, existing account store and modal patterns.

---

### Task 1: Backend Auth Model

**Files:**
- Modify: `src-tauri/src/types.rs`
- Modify: `src-tauri/src/auth/switcher.rs`

- [x] Add a failing Rust test for `StoredAccount::new_codex_access_token`.
- [x] Add `AuthMode::CodexAccessToken` and `AuthData::CodexAccessToken`.
- [x] Parse JWT payload claims `email`, `plan_type`, and `account_id` for display metadata.
- [x] Keep existing API key and ChatGPT OAuth serialization unchanged.

### Task 2: Backend Commands and Switch Flow

**Files:**
- Modify: `src-tauri/src/commands/account.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/web.rs`

- [x] Add `add_account_from_access_token(name, access_token)`.
- [x] Validate non-empty token input.
- [x] Switch access-token accounts by spawning `codex login --with-access-token` and writing the token to stdin.
- [x] Register the command for desktop Tauri and LAN web invocation.

### Task 3: Frontend UI

**Files:**
- Modify: `src/hooks/useAccounts.ts`
- Modify: `src/components/AddAccountModal.tsx`
- Modify: `src/App.tsx`

- [x] Add hook method `addFromAccessToken`.
- [x] Add an `Access Token` tab with a password textarea.
- [x] Submit to backend, reload accounts, and refresh usage as existing add-account paths do.

### Task 4: K12/Codex Usage and Metadata

**Files:**
- Modify: `src-tauri/src/api/usage.rs`
- Modify: `src-tauri/src/commands/usage.rs`
- Modify: `src-tauri/src/types.rs`

- [x] Fetch access-token usage from `https://chatgpt.com/backend-api/wham/usage`.
- [x] Register/decrypt AgentIdentity task IDs when needed and send `AgentAssertion` auth headers.
- [x] Send `ChatGPT-Account-Id` from the access-token claims when available.
- [x] Accept Codex usage payload aliases such as `primary`, `secondary`, `window_duration_mins`, and `resets_at`.
- [x] Refresh stored email/plan metadata for access-token accounts from JWT claims.

### Task 5: Verification

- [x] Run targeted Rust tests for token parsing and Codex endpoint parsing.
- [x] Run `cargo test --manifest-path src-tauri/Cargo.toml`.
- [x] Run `pnpm build`.
- [x] Smoke-test `k12_at` through the local web endpoint: metadata returns `k12`, usage returns `error: null` with 5-hour and weekly windows.

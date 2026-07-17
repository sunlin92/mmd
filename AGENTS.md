# Repository Guidelines

## Project Structure & Module Organization
This repository is a Tauri 2 desktop Markdown editor built with React, TypeScript, and Vite.

- `src/` contains the React frontend. `App.tsx` coordinates the editor UI, `components/` holds reusable UI/Markdown rendering components, `lib/` contains Markdown preprocessing and utility logic, and `types.ts` stores shared frontend types.
- `src-tauri/` contains the Rust backend. Commands, file-system authorization, directory traversal, and tests live in `src-tauri/src/lib.rs`; `main.rs` boots the app.
- `public/styles/typora-theme/` stores the migrated Typora-style theme assets.
- `src-tauri/capabilities/`, `tauri.conf.json`, and `icons/` define Tauri permissions, app metadata, and packaging assets.
- `dist/`, `node_modules/`, and `src-tauri/target/` are generated outputs; do not edit them directly.

## Build, Test, and Development Commands
- `npm install` installs frontend and Tauri CLI dependencies.
- `npm run dev` starts the Vite dev server for frontend development.
- `npm run typecheck` runs TypeScript without emitting files.
- `npm test` runs frontend unit tests with Vitest.
- `npm run test:watch` runs Vitest in watch mode for TDD loops.
- `npm run build` compiles TypeScript and creates the production frontend bundle.
- `cargo test --manifest-path src-tauri/Cargo.toml` runs Rust unit tests.
- `cargo check --manifest-path src-tauri/Cargo.toml` verifies Rust compilation quickly.
- `npm run tauri -- build --debug` builds and bundles the desktop app in debug mode.

## Coding Style & Naming Conventions
Use strict TypeScript and React function components. Prefer 2-space indentation in TS/TSX/CSS/JSON and Rustfmt defaults for Rust. Name React components in `PascalCase`, hooks/utilities in `camelCase`, and Rust commands/functions in `snake_case`. Keep file-system and security-sensitive logic centralized in `src-tauri/src/lib.rs` unless there is a clear boundary reason to split it.

## UI Feedback Conventions
All user-facing prompts, notices, warnings, and errors must be shown as modal dialogs. Do not add top bars, toast banners, raw `window.confirm` prompts, or inline technical error dumps for app-level feedback. Use the shared feedback dialog model in `src/lib/appFeedback.ts` for ordinary notices/errors, and keep destructive or branching flows such as unsaved-exit decisions in explicit multi-action dialogs. Repeated popout clicks are not app feedback: focus the existing popout directly and communicate its open state through the popout button state/label. Always translate Tauri permission/runtime errors into user-friendly text before display.

## Testing Guidelines
Use TDD for feature work and bug fixes: write or update a failing unit test first, implement the smallest change to pass it, then refactor while keeping tests green. Unit tests are required for any new behavior, bug fix, or change to security-sensitive logic. Frontend unit tests use Vitest and live beside source files as `*.test.ts` or `*.test.tsx`; Rust tests live near implementation code in `#[cfg(test)]` modules. Name tests by behavior, for example `rejects_absolute_and_parent_image_paths`. Add or update tests for file authorization, path normalization, Markdown image handling, directory traversal, and error cases. Do not remove or weaken existing tests unless the behavior intentionally changes and the PR explains why. Always run `npm test`, `npm run typecheck`, `npm run build`, and `cargo test --manifest-path src-tauri/Cargo.toml` before submitting changes.

## Commit & Pull Request Guidelines
There is no established commit history yet. Use short imperative commit messages, optionally scoped, such as `frontend: improve markdown preview` or `tauri: harden path authorization`. Pull requests should include a concise summary, verification commands run, screenshots for UI changes, and notes for any security- or permission-related behavior changes.

## Security & Configuration Tips
Treat local file access as security-sensitive. Do not broaden Tauri capabilities or path authorization without tests and an explanation. Keep dependency upgrades pinned and verify both frontend and Rust builds after changes.

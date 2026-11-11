# Testing

Marlin has static checks and four test suites. CI runs the static checks, the unit tests, and the end-to-end tests. CI does not run the Rust tests yet (issue #462), so run them yourself when you change Rust.

**Covers:** `vitest.config.ts`, `src/test-setup.ts`, `playwright.config.ts`, `e2e/`, `.github/workflows/check.yml`, `.husky/pre-push`, `src-tauri/tests/`, `src-tauri/crates/marlin-smb/tests/`
**Checked against the code:** 2026-11-11

## The suites

| Suite           | Command                                  | Tool                                   | Runs in CI | Needs                          |
| --------------- | ---------------------------------------- | -------------------------------------- | ---------- | ------------------------------ |
| Static checks   | `npm run check`                          | ESLint, Prettier, `tsc`, `cargo check` | yes        | nothing                        |
| Unit            | `npm run test:run` (`npm test` to watch) | Vitest, Testing Library, jsdom         | yes        | nothing                        |
| End-to-end      | `npm run test:e2e`                       | Playwright, Chromium                   | yes        | nothing; it starts Vite itself |
| Rust            | `cd src-tauri && cargo test`             | `cargo test`                           | **no**     | a Rust toolchain               |
| SMB integration | see below                                | `cargo test --ignored`                 | no         | a real SMB server              |

`npm run check` treats Rust warnings as errors. The pre-push hook runs it, so a push fails on a lint, format, type, or Rust warning.

CI skips a job when the files it covers did not change (`check.yml` uses path filters).

## Unit tests

- They live in `src/__tests__/unit/` (store, hooks, utils), in `src/components/__tests__/`, and beside the code as `*.test.ts`.
- `src/test-setup.ts` mocks `@tauri-apps/api/core` and `@tauri-apps/plugin-dialog`. A test sets what `invoke` returns.
- Most of the weight is on `useAppStore`: navigation, per-folder preferences, the hidden-files toggle, and the filter.

## End-to-end tests

- They live in `e2e/`, one `*.spec.ts` per area.
- **They do not run the desktop app.** Playwright starts `npm run dev` on port 1420 and opens the frontend in Chromium. `e2e/tauri-mocks.ts` is injected before page load and fakes every Tauri command with an in-memory file tree.
- So these tests cover the React UI and the store. They cannot catch a bug in Rust or in a real OS integration.
- Locally, Playwright reuses a server already on port 1420. In CI it starts a fresh one, uses one worker, and retries a failed test twice.
- A new backend command that the UI calls on load needs a fake in `tauri-mocks.ts`, or the page fails in every spec.

## Rust tests

- **A plain `cargo test` does not pass today.** The five ZPL thumbnail tests abort the test process when they run in parallel (issue #469). Five Google Drive tests fail on a machine with a Google account connected, because no TLS crypto provider is installed in tests (issue #470). `cargo test --lib -- --skip zpl` avoids the crash: expect 50 passed and those 5 failed.
- Unit tests sit in `#[cfg(test)]` modules inside the source files. There are about 60, mostly in `locations/` and `thumbnails/`.
- `cargo test -p marlin-smb` runs the SMB helper's test. It starts the built helper, sends a `ping`, and checks the reply and a clean exit. It needs samba installed, and CI does not run it.
- `src-tauri/tests/smb_integration.rs` holds manual tests that need a real SMB server. They are marked `#[ignore]`. The file's header lists the environment variables and the command to run them.

## What to test

- **A bug fix:** write the failing test first, then fix it.
- **Store logic:** a unit test.
- **A user flow in the UI:** an end-to-end test.
- **Rust logic:** a Rust unit test next to the code.
- **Real desktop behavior** (native drag, the clipboard, an actual remote server): no automated suite covers it. Test it by hand in `npm run tauri dev` and say so in the PR.

# Testing

Marlin has static checks and nine test suites. CI runs the static checks, the unit tests, and the end-to-end tests. CI does not run the Rust tests yet (issue #462), so run them yourself when you change Rust.

**Covers:** `vitest.config.ts`, `src/test-setup.ts`, `playwright.config.ts`, `e2e/`, `.github/workflows/check.yml`, `.husky/pre-push`, `src-tauri/tests/`, `src-tauri/crates/marlin-smb/tests/`, `src-tauri/crates/marlin-vfs/tests/`, `src-tauri/crates/marlin-gdrive/tests/`, `src-tauri/crates/marlin-mount/tests/`, `src-tauri/crates/marlin-mount/test-sweep.sh`, `src-tauri/crates/marlin-fuse/tests/`
**Checked against the code:** 2026-11-13

## The suites

| Suite                | Command                                  | Tool                                                            | Runs in CI | Needs                          |
| -------------------- | ---------------------------------------- | --------------------------------------------------------------- | ---------- | ------------------------------ |
| Static checks        | `npm run check`                          | ESLint, Prettier, `tsc`, `cargo check`                          | yes        | nothing                        |
| Unit                 | `npm run test:run` (`npm test` to watch) | Vitest, Testing Library, jsdom                                  | yes        | nothing                        |
| End-to-end           | `npm run test:e2e`                       | Playwright, Chromium                                            | yes        | nothing; it starts Vite itself |
| Rust                 | `cd src-tauri && cargo test`             | `cargo test`                                                    | **no**     | a Rust toolchain               |
| Mount core           | see below                                | `cargo test`                                                    | no         | a Rust toolchain               |
| Drive mount provider | see below                                | `cargo test`                                                    | no         | a Rust toolchain               |
| Mount helper         | see below                                | `cargo test`                                                    | no         | a Rust toolchain               |
| Linux mounts         | see below                                | `cargo test` in Docker                                          | no         | Docker                         |
| SMB integration      | see below                                | `cargo test --features smb --test smb_integration -- --ignored` | no         | a real SMB server              |
| Google Drive         | see below                                | `cargo test --test gdrive_integration -- --ignored`             | no         | a connected Google account     |

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

- **A plain `cargo test` does not pass today.** The five ZPL thumbnail tests abort the test process when they run in parallel (issue #469). `cargo test --lib -- --skip zpl` avoids the crash, and every other test passes.
- Unit tests sit in `#[cfg(test)]` modules inside the source files, mostly in `locations/` and `thumbnails/`. The SFTP host key test in `locations/sftp/pool.rs` starts an SSH server inside the test process on a free local port.
- `cargo test -p marlin-vfs` runs the mount core's tests. A plain `cargo test` does not include them. They run on an in-memory provider and need nothing installed. See [Mount core](mount-core.md).
- `cargo test -p marlin-gdrive` runs the tests of the Google Drive provider for mounts. A plain `cargo test` does not include them. They run against a fake Drive server inside the test process and need no network and no account. See [Google Drive mount provider](mount-gdrive.md).
- `cargo test -p marlin-mount` runs the mount helper's tests. A plain `cargo test` does not include them. They talk to the helper over a real socket in a temporary folder, with a fake adapter, and need nothing installed. `tests/client.rs` tests the app's client the same way: it starts the built helper, and it stands in a fake helper where it needs another protocol version. See [Mount helper](mount-helper.md).
- `src-tauri/crates/marlin-mount/test-sweep.sh` checks the helper against a real dead mount. It needs a Mac with macFUSE, or Linux with `fuse3`. No `cargo test` covers the real unmount of a dead mount, so run it when you change that code. See [Mount helper](mount-helper.md).
- `src-tauri/crates/marlin-fuse/test-in-docker.sh` runs the Linux mount adapter's tests in a container: unit tests, tests that mount an in-memory provider through FUSE, and `test-sweep.sh`. On a Mac the crate compiles to nothing, so `cargo test -p marlin-fuse` runs no test there. See [Linux mount adapter](mount-linux.md).
- `scripts/check-bundled-mount-helper.mjs` is not a `cargo test`. The release dry run in CI runs it on each built bundle: it starts the helper from the macOS app or the `deb` and checks that it answers. See [Mount helper](mount-helper.md#building-and-bundling).
- `cargo test -p marlin-smb` runs the SMB helper's test. It starts the built helper, sends a `ping`, and checks the reply and a clean exit. It needs samba installed, and CI does not run it.
- A unit test that uses the keychain calls `test_keychain::install()` first. `locations/test_keychain.rs` is an in-memory keychain that takes the place of the real one. There is one for the whole test process, because the `keyring` crate holds one store per process. A unit test that reaches for the real Google accounts file gets an error, because reading that file can rewrite it.
- `src-tauri/tests/smb_integration.rs` holds manual tests that need a real SMB server. They are marked `#[ignore]`. The file's header lists the environment variables and the command to run them.
- `src-tauri/tests/gdrive_integration.rs` holds manual tests that call the live Drive API as the first account Marlin lists (the service account when `GOOGLE_SERVICE_ACCOUNT_KEY_FILE` is set). They are marked `#[ignore]`, and they read the real keychain. The file's header lists the command, the one environment variable, and what to do before the first run.

## What to test

- **A bug fix:** write the failing test first, then fix it.
- **Store logic:** a unit test.
- **A user flow in the UI:** an end-to-end test.
- **Rust logic:** a Rust unit test next to the code.
- **Real desktop behavior** (native drag, the clipboard, an actual remote server): no automated suite covers it. Test it by hand in `npm run tauri dev` and say so in the PR.

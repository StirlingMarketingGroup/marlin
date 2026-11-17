# 0004: Tests drive the real app through a WebDriver server built into a test build

**Status:** accepted
**Date:** 2026-11-17
**Issue:** #497

## Decision

A test drives the real Marlin app over WebDriver. The server is inside the app: the `tauri-plugin-wdio-webdriver` crate, compiled in only by the Cargo feature `app-test`. macOS and Linux use the same route.

- **The test build.** `npm run test:app` builds the app with `--features app-test` in debug, into its own folder, `src-tauri/target/app-test/`. The feature does two things: it serves WebDriver on a port of `127.0.0.1`, and it swaps the OS keychain for the in-memory one from `marlin-test-keychain`.
- **The client.** Tests are Vitest files in `app-tests/`. They talk to the server with the `webdriverio` package, version 9, used as a plain client. We do not use WebdriverIO's test runner or its Tauri service.
- **One app per test file.** `startApp()` in `app-tests/app.ts` starts the app with a temporary home folder and a free port. When the file is done, it stops the app and removes the folder.
- **The existing Playwright suite stays as it is,** in Chromium with Tauri faked. Running it in WebKit as well is worth doing, and #576 tracks it.

[Testing](../testing.md#real-app-tests) says how to run the suite and how to write a test.

## Why

**The gap.** No test ran the real app. The Playwright suite opens the frontend in Chromium with every Tauri command faked, so it cannot catch a bug in Rust, in the webview the app really uses, or in how the two work together. #194 and #480 were both fixed without anyone seeing the fix in a running app.

**What the route had to do:**

| Need                                             | How this route meets it                                                                                                                                                                                                                                              |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Work on macOS, where the work happens            | macOS has no outside WebDriver for WKWebView. This plugin drives WKWebView from inside the app.                                                                                                                                                                      |
| Work on Linux, where a Mac cannot see the bugs   | The same plugin drives WebKitGTK from inside the app. No `WebKitWebDriver` package is needed.                                                                                                                                                                        |
| One command for a person, and usable by an agent | `npm run test:app`. An agent writes a test file and runs it. A test can save a screenshot of the window.                                                                                                                                                             |
| No control port in a release                     | The plugin is an optional dependency behind `app-test`. `lib.rs` refuses to compile that feature without debug assertions.                                                                                                                                           |
| Never touch the developer's settings or sign-ins | The app reads its settings, caches, logs, and webview data through `HOME`, and its temp files through `TMPDIR`. The harness points both at a temporary folder and passes on only a short list of environment variables. The feature replaces the keychain in memory. |
| Run beside `npm run tauri dev`                   | Its own target folder, its own port, its own home folder. The app has no single-instance lock.                                                                                                                                                                       |

**It is the route Tauri's own docs name.** The [Tauri WebDriver page](https://v2.tauri.app/develop/tests/webdriver/) says `tauri-driver` supports only Windows and Linux on desktop, and points to WebdriverIO's embedded server for macOS. The crate is kept by the WebdriverIO organization (`webdriverio/desktop-mobile`, MIT). On 2026-11-17 crates.io showed version 1.5.1 from 2026-10-24, a release about every month since 1.0.0 in May 2026, and 453,000 downloads.

**The plugin asks nothing of the app.** It has no commands and an empty permission set, so `src-tauri/capabilities/` does not change. It reads its port from `TAURI_WEBDRIVER_PORT` and binds to `127.0.0.1` only.

**The client can be small.** The server speaks plain W3C WebDriver. Starting the app and waiting for `GET /status` is all the setup it needs, so the harness is one file and one new dev dependency.

**What was checked, on macOS 27.2 (Apple silicon), 2026-11-17:**

- The test build compiled on the first try with Marlin's Tauri 2.12.5. A cold build of the separate target folder took 2 minutes and 7.4 GB.
- The app answered `/status` 0.35 seconds after it started.
- With `HOME` and `CFFIXED_USER_HOME` set to a temporary folder, the app wrote its preferences, bookmarks, logs, thumbnail cache, and WebKit data under that folder, and opened that folder as its first location.
- The app's window did not take keyboard focus from the terminal that started it.
- The proof test, `app-tests/directory-watcher.test.ts`, is the #194 case: with a folder open, a file is written as `report.pdf.crdownload` and renamed to `report.pdf`, and the list shows the final name with no refresh.
- That test passed 7 of 8 runs once its waits were 120 seconds. The one failure was a wait of more than 120 seconds for a file event. The cause is the machine, not the route: see "macOS file events" below.
- A release build holds none of it. `cargo build --release --features app-test` stopped at the `compile_error!` in `lib.rs`. `cargo build --release` then built, and `strings` on that binary found no `wdio`, `webdriver`, or `marlin_test_keychain` (the test build has 41 lines with `wdio`). Without the feature, `cargo tree -i tauri-plugin-wdio-webdriver` finds no such package.

**What was checked on Linux,** in Docker (`rust:1-slim-bookworm`, arm64, WebKitGTK 2.50.6, Xvfb), 2026-11-17: `app-tests/test-in-docker.sh` built the same test build and ran the same test. It passed 5 of 5 runs once `startApp()` waited for the first folder, in under 2 seconds each. The window drew correctly in a screenshot. No `WebKitWebDriver` and no extra container rights were needed.

**macOS file events.** The first real-app test found something no other suite could: on the Mac it ran on, macOS handed the app its file events 5 seconds to more than 2 minutes late. Timestamps in `fs_watcher.rs` showed the time was lost before Marlin's code ran, and `fseventsd` was at 93% to 98% CPU throughout. So the proof test waits up to 120 seconds for each change on disk, and can still fail on a Mac that busy. #577 tracks what the app should do about late events.

**Startup.** A test must not type before the app has opened its first folder: the startup then overwrites what was typed. `startApp()` waits for the path bar to show a location. On Linux, where the app starts fast, the test failed every run until it did.

**Untested:**

- Windows. The plugin lists WebView2 support. Marlin has no Windows build to test (#135).
- A real Linux desktop. Only Docker with a virtual display was run.
- Marlin's other windows (preferences, progress, connect). The server lists each window by its Tauri label, and the proof uses only `main`.
- Native parts that are not the webview: the app menu, the native context menu, drag out of the window, and system dialogs. WebDriver sees the page, not these.

## Rejected

| Route                                                | Why it lost                                                                                                                                                                                                                              |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tauri's `tauri-driver`                               | No macOS. On Linux it also needs the `WebKitWebDriver` package. It would mean two routes, and the one we use most would be the one without it.                                                                                           |
| WebdriverIO's test runner with `@wdio/tauri-service` | It drives the same plugin. It adds a second test runner, a reporter, and a service whose extras (faking commands, a second plugin in the frontend) we do not want in a test of the real app. Vitest plus the plain client does the job.  |
| `webdriverio` version 10                             | It needs Node 22.23.4 or newer. Version 9 runs on Node 18.20 and up, and it is the version the plugin's own service is pinned to.                                                                                                        |
| `tauri-plugin-webdriver` (Choochmeque)               | The WebdriverIO plugin is a fork of it. Both are kept up. We took the one Tauri's docs name and an organization keeps.                                                                                                                   |
| `tauri-plugin-playwright`                            | Its README says real Playwright cannot attach on macOS or Linux, so it is a socket bridge with a Playwright-like API. One maintainer, last release 2026-06-20. It needs `withGlobalTauri` and a permission in the app's capability file. |
| `tauri-webdriver-automation`                         | macOS only. Last release 2026-02-14.                                                                                                                                                                                                     |
| MCP plugins (`tauri-plugin-mcp`, `mcp-server-tauri`) | They give an agent tools, not a test a person runs with one command. `tauri-plugin-mcp` is an alpha last released in November 2025. Not tried.                                                                                           |
| CrabNebula's `tauri-driver` fork                     | It needs a paid key on macOS.                                                                                                                                                                                                            |
| Playwright's WebKit build, in place of the real app  | It is the same engine family as the macOS webview, but every Tauri command is still faked. It tests the page, not the app.                                                                                                               |
| A test build in `src-tauri/target/debug`             | That folder belongs to `npm run tauri dev`. A test build there would replace the binary of the app the developer is using.                                                                                                               |

**The existing suite in WebKit.** Run once on 2026-11-17 with a `webkit` project (`devices['Desktop Safari']`), the Playwright suite had 80 tests pass and 18 fail. So WebKit sees things Chromium does not, and the suite cannot switch over as it is. #576 tracks sorting the 18 into app bugs and test assumptions, and then running both browsers in CI.

## What follows

- A change to Rust, or to how Rust and the frontend work together, can now be seen working in the running app before the PR opens. `AGENTS.md` and [Testing](../testing.md) say when to do that.
- The test build shares three outputs with every other `tauri build`: `dist/`, the copy of the mount helper in `src-tauri/binaries/`, and the copy of libzpl in `src-tauri/lib/`. It never touches `src-tauri/target/debug/`.
- The mount helper is a separate process and still uses the real keychain. A test that mounts an account needs that solved first.
- A test that needs a native menu, a native drag, or a system dialog needs another tool. This route does not cover them.
- Follow-up work: #578 runs the suite in CI. #579 adds tests for the main file flows. #580 lets a test use mounts. #576 runs the Playwright suite in WebKit. #577 is the late file events on macOS.

# Marlin: rules for agents

Marlin is a desktop file browser. The frontend is React + TypeScript + Tailwind in `src/`. The backend is Rust + Tauri 2 in `src-tauri/`.

This file holds the rules that always apply. Everything longer lives in the wiki under `docs/`. **Read [`docs/index.md`](docs/index.md) before you start a task**, then read the pages it routes you to. `CLAUDE.md` imports this file, so there is one rule set.

## Read before you touch

| Task                                                                | Read first                                                 |
| ------------------------------------------------------------------- | ---------------------------------------------------------- |
| Anything, to find your way around                                   | [`docs/architecture.md`](docs/architecture.md)             |
| Remote storage: Google Drive, SMB, SFTP, archives, paste, drag/drop | [`docs/location-providers.md`](docs/location-providers.md) |
| Mounting remotes as native filesystems (epic #368)                  | `docs/location-providers.md`, then `docs/decisions/`       |
| Thumbnails                                                          | [`docs/thumbnails.md`](docs/thumbnails.md)                 |
| Writing or running tests                                            | [`docs/testing.md`](docs/testing.md)                       |
| Releases, signing, notarization                                     | [`docs/release-signing.md`](docs/release-signing.md)       |
| A design choice that later work must not reopen                     | [`docs/decisions/README.md`](docs/decisions/README.md)     |

## Commands

- `npm run tauri dev` starts the desktop app with hot reload.
- `npm run dev` starts the frontend alone in Vite.
- `npm run check` runs ESLint on changed files, Prettier, both TypeScript projects, and `cargo check` with warnings as errors. The pre-push hook runs it.
- `npm run test:run` runs the unit tests once.
- `npm run test:e2e` runs the Playwright tests. They start their own Vite server and mock Tauri, so the desktop app does not need to run.
- `cd src-tauri && cargo test` runs the Rust tests.
- `npm run format:fix` and `npm run lint:fix` fix formatting and lint errors.
- `npm run build:smb-sidecar` builds the SMB helper binary. It needs samba installed (see `README.md`).

## Before you call work done

1. Run `npm run check` and `npm run test:run`. Both must pass with no warnings.
2. If you changed Rust, run `cargo test` in `src-tauri/`. CI does not run it yet (issue #462). [`docs/testing.md`](docs/testing.md) lists the tests that fail today and the issue for each. A failure listed there does not block your branch. Any other failure does. A plain `cargo test` skips the workspace crates. If you changed one under `src-tauri/crates/` (`marlin-smb`, `marlin-vfs`, `marlin-mount`), also run `cargo test -p <crate>` there. If you changed `marlin-fuse`, or the `Adapter` or `Session` trait or `main.rs` in `marlin-mount`, run `src-tauri/crates/marlin-fuse/test-in-docker.sh`: the Linux code is compiled out on a Mac.
3. If you changed `src/` or `e2e/`, run `npm run test:e2e`.
4. Update the wiki page that covers what you changed (see "Keeping the wiki true").

## Never kill the dev server

`npm run tauri dev` reloads TypeScript changes at once and rebuilds and restarts on Rust changes. Do not kill it or search for its PID. If you think it needs a restart, ask the developer.

## Branch size

One branch makes one semantic change. A reviewer must be able to hold the whole branch in their head. Drive-by fixes (see "Fix what you find") go in the same branch. They count toward the ceiling, not toward the layer count.

- **Ceiling:** about 10 files and 1,300 added lines that a person has to read. Lockfiles, generated files, renames, and deletions do not count.
- **Count the layers before you branch:** a Rust backend module, the Tauri command surface (`commands.rs`, `lib.rs`, `src/types/`), the Zustand store, components or windows, CI or packaging, and code for one more operating system. Assume the real work is twice what the issue says.
- **Two layers or fewer and under the ceiling:** go.
- **More than that:** stop before you branch. Split the work into spec issues, where each issue is one branch and names what blocks it. Start the first one.
- **A branch that grows past the ceiling midway:** stop. Move the drive-by fixes to one branch of their own first. If the branch is still over, park it as a reference, file the issues it should have been, and restart the first piece on a fresh branch.
- Branch from the latest `origin/master`, never from another feature branch.

## Fix what you find

Work on this code the way a mechanic works on an airplane. A mechanic who finds a defect does not close the panel and walk away. Expect to find defects as you work.

- **Every defect you find ends up fixed or tracked.**
- **A defect is something that is wrong:** the code, a test, or a page does not do or say what it should. Code you would have written another way, a missing feature, and a rough spot that a wiki page already lists are not defects.
- **The default is to fix it now, in the branch you are on.** This covers a bug, a wrong comment, dead code, a stale doc line, a broken link, and a failing or flaky test. Fix the cause: do not skip, delete, or loosen a test to make it pass. Test the fix like any other change (see [`docs/testing.md`](docs/testing.md)). A fix made this way is a "drive-by".
- **An issue is not a fix. File one only when the fix cannot go in this branch.** That is true in four cases:
  - It would push the branch past the size ceiling.
  - It needs a decision that is not yours to make. Either it changes behavior a user sees and you cannot tell which behavior is intended, or it reopens a record in `docs/decisions/`.
  - You cannot check it from here. For example, it needs another operating system, a real server or account, or a hand test in the desktop app that you cannot do.
  - Nobody asked you to change code, as in a review or an investigation. Report the defect to whoever asked, or file the issue.
- **When you file an issue,** give the evidence: file, line, and what is wrong.
- **A defect that an open issue already tracks** follows the same default. Fix it and close that issue from the PR. If one of the four cases holds, add what you learned to the issue. Do not file a second one.
- Never patch on top of a patch. If adding a feature needs contortions, redesign that piece first. Less code is better code.

## Code conventions

**TypeScript and React**

- Strict mode. Keep types precise.
- 2-space indent. Function components with hooks.
- Components are PascalCase (`FileGrid.tsx`). Hooks are `useX.ts`. Stores are `useXStore.ts`. Shared types live in `src/types/index.ts`.
- Import with the `@/` alias instead of long relative paths.
- Style with Tailwind utility classes.

**Rust**

- Expose a function to the frontend with `#[tauri::command]` and register it in `src-tauri/src/lib.rs`.
- Return `Result`. Use async for I/O.
- Put platform code behind `#[cfg(target_os = "...")]`.
- Warnings fail the check. Fix them, do not silence them.

**Security**

- Request only the Tauri capabilities you use (`src-tauri/capabilities/`).
- Never commit secrets. Never put credentials or tokens in paths, logs, or error messages.
- Do not hardcode user paths.

## Commits and pull requests

- Prefix the subject: `feat:`, `fix:`, `refactor:`, `docs:`, `chore:`. Keep it under 50 characters.
- A PR links its issue, says which platforms you tested, and shows a screenshot for any UI change.
- A PR lists each drive-by fix under a "Drive-by" heading.

## Keeping the wiki true

The wiki follows the LLM wiki pattern: the code is the source, the pages in `docs/` are the compiled knowledge, and this file is the schema that says how to maintain them.

- **Read:** start at `docs/index.md`. It lists every page with one line on what it answers.
- **Write:** when a change makes a page wrong, fix the page in the same branch. When you learn something a future reader would have to rediscover, add it to the page that owns the topic.
- **One topic per page.** A page states what the code does today, not how it got there. Git history is the log.
- **Every page names the files it covers** and the date someone last checked it against the code.
- **Every page is in the index.** A new page gets an index line in the same branch.
- **Link, do not copy.** If two pages need the same fact, one owns it and the other links to it.
- **Decisions** go in `docs/decisions/` as numbered records. **Old plans** in `docs/plans/` are frozen history. Never treat them as current, and never edit them.
- **Lint when you pass through:** if you read a page and find a claim the code no longer supports, a dead file path, or two pages that disagree, fix it in your branch.

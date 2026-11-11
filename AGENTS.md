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
2. If you changed Rust, run `cargo test` in `src-tauri/`. CI does not run it yet (issue #462).
3. If you changed `src/` or `e2e/`, run `npm run test:e2e`.
4. Update the wiki page that covers what you changed (see "Keeping the wiki true").

## Never kill the dev server

`npm run tauri dev` reloads TypeScript changes at once and rebuilds and restarts on Rust changes. Do not kill it or search for its PID. If you think it needs a restart, ask the developer.

## Branch size

One branch makes one semantic change. A reviewer must be able to hold the whole branch in their head.

- **Ceiling:** about 10 files and 1,300 added lines that a person has to read. Lockfiles, generated files, renames, and deletions do not count.
- **Count the layers before you branch:** a Rust backend module, the Tauri command surface (`commands.rs`, `lib.rs`, `src/types/`), the Zustand store, components or windows, CI or packaging, and code for one more operating system. Assume the real work is twice what the issue says.
- **Two layers or fewer and under the ceiling:** go.
- **More than that:** stop before you branch. Split the work into spec issues, where each issue is one branch and names what blocks it. Start the first one.
- **A branch that grows past the ceiling midway:** stop. Park it as a reference, file the issues it should have been, and restart the first piece on a fresh branch.
- Branch from the latest `origin/master`, never from another feature branch.

## Fix what you find

Much of this code was written early and fast. Expect things that make no sense.

- **Small and safe:** fix it in the branch you are on. Examples: a wrong comment, dead code, a stale doc line, a broken link.
- **Too big for a drive-by:** file an issue with the evidence (file, line, what is wrong). Do not leave it unmentioned.
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

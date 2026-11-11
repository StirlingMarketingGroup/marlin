# Marlin wiki

Start here. Each line says what a page answers. The rules for keeping these pages true are in [`AGENTS.md`](../AGENTS.md), under "Keeping the wiki true".

## How the app works

| Page                                        | Read it to learn                                                                                       |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| [Architecture](architecture.md)             | What runs where: the two processes, the Rust modules, the React windows and stores, and how they talk. |
| [Location providers](location-providers.md) | How local folders, archives, Google Drive, SMB, and SFTP work today, and where that model breaks.      |
| [Thumbnails](thumbnails.md)                 | How a thumbnail gets requested, generated, and cached, for local and remote files.                     |

## How to work on it

| Page                                  | Read it to learn                                                             |
| ------------------------------------- | ---------------------------------------------------------------------------- |
| [Testing](testing.md)                 | The four test suites, what each one covers, and which ones CI runs.          |
| [Release signing](release-signing.md) | How a release gets built, signed, and notarized, and which secrets it needs. |

## Decisions

[`decisions/`](decisions/README.md) holds numbered decision records. A record says what we chose, why, and what we rejected, so later work does not reopen it. There are none yet. The first one is the mount architecture from issue #369.

## Frozen history

[`plans/`](plans/) holds design documents written before the work was done. They show intent at the time. **They do not describe the current code**, and nobody updates them.

| File                                                     | What it was                                              |
| -------------------------------------------------------- | -------------------------------------------------------- |
| `plans/2025-09-09-roadmap.md`                            | The original product roadmap. GitHub issues replaced it. |
| `plans/2025-12-09-google-drive-integration-design.md`    | Design for the Google Drive provider.                    |
| `plans/2026-01-12-large-directory-performance-design.md` | Design for fast listing of very large folders.           |
| `plans/2026-01-14-smb-support-design.md`                 | Design for the SMB provider.                             |

## Not here

- Setup, install, and the feature list are in [`README.md`](../README.md).
- Theme file format is in `README.md` and [`src/themes/README.md`](../src/themes/README.md).
- Planned work is in [GitHub issues](https://github.com/StirlingMarketingGroup/marlin/issues).

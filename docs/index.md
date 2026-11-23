# Marlin wiki

Start here. Each line says what a page answers. The rules for keeping these pages true are in [`AGENTS.md`](../AGENTS.md), under "Keeping the wiki true".

## How the app works

| Page                                           | Read it to learn                                                                                                                                                                                                                                          |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [Architecture](architecture.md)                | What runs where: the three processes, the Rust modules, the React windows and stores, and how they talk.                                                                                                                                                  |
| [Location providers](location-providers.md)    | How local folders, archives, Google Drive, SMB, and SFTP work today, and where that model breaks.                                                                                                                                                         |
| [Thumbnails](thumbnails.md)                    | How a thumbnail gets requested, generated, and cached, for local and remote files.                                                                                                                                                                        |
| [Mount core](mount-core.md)                    | What the `marlin-vfs` crate gives a mount adapter and asks of a provider: nodes, names, limits, the write spool, and uploads.                                                                                                                             |
| [Google Drive mount provider](mount-gdrive.md) | What the `marlin-gdrive` crate shows of a Google account in a mount, and how it changes one: folders, uploads, link files, item IDs, errors, retries, Google's limits. Also the saved Google sign-in, and where the OAuth client ID and secret come from. |
| [SFTP mount provider](mount-sftp.md)           | What the `marlin-sftp` crate shows of an SFTP server in a mount: the top folder, links, reads, free space, the connection, refused sign-ins, and errors.                                                                                                  |
| [Mount helper](mount-helper.md)                | How the `marlin-mount` process is built, bundled, and controlled: its socket, its protocol, the rules of its mounts, the providers it links, and the app's commands and events for it.                                                                    |
| [macOS mount adapter](mount-macos.md)          | How `marlin-fuse` serves a mount through macFUSE: what the machine needs, the life of a session, and the tests.                                                                                                                                           |
| [Linux mount adapter](mount-linux.md)          | How `marlin-fuse` serves a mount through FUSE: what the machine needs, how writes are served, the error table, and the tests.                                                                                                                             |

## How to work on it

| Page                                  | Read it to learn                                                                                                       |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| [Testing](testing.md)                 | The test suites, what each one covers, and which ones CI runs.                                                         |
| [Release signing](release-signing.md) | How a release gets built, signed, and notarized, which secrets it needs, and where the Linux packages put `libzpl.so`. |
| [Licenses](licenses.md)               | Which licenses a dependency may have, how the check works, and which license texts each installer carries.             |

## Decisions

[`decisions/`](decisions/README.md) holds numbered decision records. A record says what we chose, why, and what we rejected, so later work does not reopen it.

| Record                                                                                 | Read it to learn                                                                                                                                   |
| -------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| [0001: Mount architecture](decisions/0001-mount-architecture.md)                       | How remotes become native mounts (epic #368): the portable core, the adapter per OS, the helper process, and the risks.                            |
| [0002: Node IDs follow lookup counts](decisions/0002-node-ids-follow-lookup-counts.md) | How long a mount's node ID lasts, and why it is not the life of the mount.                                                                         |
| [0003: Google Drive link files](decisions/0003-gdrive-link-files.md)                   | How a Google Drive mount shows Google Docs, Sheets, Slides, and shortcuts, which have no bytes to read.                                            |
| [0004: Real-app tests](decisions/0004-real-app-tests.md)                               | How a test drives the real app on macOS and Linux, and which other routes lost.                                                                    |
| [0005: Mount writes](decisions/0005-mount-writes.md)                                   | How a mount takes writes: what goes to the remote at once, what waits in the local spool, and what `close`, `fsync`, a conflict, and a crash mean. |
| [0006: SMB mounts](decisions/0006-smb-mounts.md)                                       | What an SMB mount is and what it is named, who starts `marlin-smb` for it, how many processes it runs, and where it gets the password.             |
| [0007: Permanent delete](decisions/0007-permanent-delete.md)                           | How a permanent delete runs on four threads without following a link or emptying another drive inside it, and why no native tool is used.          |
| [0008: Read-only files](decisions/0008-read-only-files.md)                             | How a mount shows a file the account may not change, what the core refuses, and why an upload the remote refuses stops and waits for the user.     |

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

# Licenses

What license Marlin has, which licenses a dependency may have, and how the check works. Read this before you add a dependency or upgrade one across a major version.

**Covers:** `LICENSE`, `THIRD-PARTY-NOTICES.md`, `src-tauri/about.toml`, `scripts/check-licenses.mjs`, the "Check dependency licenses" step in `.github/workflows/check.yml`
**Checked against the code:** 2026-11-18

## Marlin's license

- Marlin's own code is MIT. [`LICENSE`](../LICENSE) holds the text, and every `Cargo.toml` in the workspace says `license = "MIT"`.
- [`THIRD-PARTY-NOTICES.md`](../THIRD-PARTY-NOTICES.md) names the work of others that Marlin ships or uses, where its license asks for more than a copy of its text. That page owns those facts. This page owns the rules.

## Rules for a dependency

A dependency is anything that ends up in a binary or bundle Marlin ships: a Rust crate, an npm package the frontend imports, a C library a crate compiles, an icon set, a font.

1. **Every Rust crate and npm package that ships is under a license on the accepted list**, the `accepted` array in [`src-tauri/about.toml`](../src-tauri/about.toml). The list holds permissive licenses and the Mozilla Public License 2.0. When a package offers a choice, one accepted choice is enough. The check enforces this rule. An npm package whose JavaScript, CSS, fonts, or images the frontend imports goes under `dependencies` in `package.json`, even when only a stylesheet imports it, because the check skips `devDependencies`.
2. **No GPL, AGPL, or LGPL code is compiled into the app or the mount helper.** GPL and AGPL code would put the whole binary under those terms. LGPL code would oblige Marlin to give users what they need to relink the binary. Loading an LGPL library at run time is fine: GTK, WebKitGTK, and macFUSE's libfuse work this way. Two things are compiled in today and are allowed: libgit2, which is GPL with a linking exception that covers Marlin, and the LibXDiff code inside libgit2, which is LGPL. Anything else like these is the owner's decision.
3. **A Windows build holds only open-source code.** Windows mounts will load WinFsp ([decision 0001](decisions/0001-mount-architecture.md)), and WinFsp's license lets an open-source app do that only when the app is "not linked or distributed with proprietary (non-FLOSS) software". The check enforces this rule for the crates in `NOT_OPEN_SOURCE` in the script. UnRAR is the one entry, and only macOS and Linux builds link it.
4. **Read what the dependency builds, not only its label.** The check reads the license each package states. A crate that compiles or downloads other code, or a package of icons or fonts, can state MIT and carry something else: `unrar_sys`, `libgit2-sys`, `libdbus-sys`, `zpl-rs`, and `@iconify-json/vscode-icons` all do. Look in the package for a `vendor/` folder, a build script that compiles or downloads code, embedded fonts and images, and the upstream project's own license statement. Rules 2 and 3 apply to what you find, whatever the label says.
5. **A program Marlin runs but does not link may have any license**, as long as Marlin does not ship it. FFmpeg is downloaded on first use. The `marlin-smb` helper is a separate program because it links Samba, which is GPL. Putting such a program or library inside a bundle is a new decision: write a record in [`decisions/`](decisions/README.md). The AppImage already carries Samba's libraries: see [System libraries in the AppImage](../THIRD-PARTY-NOTICES.md#system-libraries-in-the-appimage).
6. **When a license asks for a notice, a credit, or anything else, add it to `THIRD-PARTY-NOTICES.md`** in the same branch.

## The check

`npm run check:licenses` runs `scripts/check-licenses.mjs`. It fails when:

- an npm package that can ship in the frontend (anything but a dev dependency) states a license that is not on the list, or states none. It reads `package-lock.json`. When a package states none, find its license and add the package to `UNSTATED` in the script, with a comment that says where the license is. A license that is not an SPDX expression (`SEE LICENSE IN ...`) fails too: the script has no way past that yet.
- a Rust crate that ships in the app or a helper states a license that is not on the list. This part is [cargo-about](https://github.com/EmbarkStudios/cargo-about), which reads `about.toml` and looks at every target in its `targets` list. Build and dev dependencies do not count.
- a Windows build would link a crate in `NOT_OPEN_SOURCE` (rule 3).

CI runs it in `check.yml` whenever the static checks run, so a Dependabot update gets checked too. It is not part of `npm run check`, because it needs cargo-about installed. Install it with the `cargo install` line in the "Check dependency licenses" step of `check.yml`. The check takes about a minute.

`scripts/check-licenses.test.mjs` tests the npm rule. `npm run test:run` runs it.

## Adding a license to the list

- A permissive license that is not on the list yet (the kind that asks only for its notice to be kept): add it to `about.toml` in the branch that needs it, and say so in the PR.
- Any copyleft license, or one that limits how the software may be used: this is the owner's decision. Ask first, then write a record in `decisions/`.

## Known gaps

- **The installers carry no license texts for the code built into Marlin** (#589). Most licenses on the list ask that their text go with every copy. The AppImage does carry the copyright files of the system libraries it copies in.
- **`libzpl` carries three commercial fonts** ([go-zpl #37](https://github.com/StirlingMarketingGroup/go-zpl/issues/37)). With them inside, `libzpl` does not pass rule 3. `zpl-rs` is not in `NOT_OPEN_SOURCE`, because every build links it today and the check would fail on every change.
- **The check trusts labels.** Rule 4 is a step a person does.
- **The check skips build tools.** Vite is a dev dependency, and it adds a few lines of its own to the frontend bundle (the module preload helper).

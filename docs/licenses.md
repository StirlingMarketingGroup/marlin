# Licenses

What license Marlin has, which licenses a dependency may have, how the check works, and which license texts each installer carries. Read this before you add a dependency or upgrade one across a major version.

**Covers:** `LICENSE`, `THIRD-PARTY-NOTICES.md`, `src-tauri/about.toml`, `src-tauri/licenses/`, `bundle.resources` in `src-tauri/tauri.conf.json`, `scripts/check-licenses.mjs`, `scripts/generate-license-texts.mjs`, `scripts/check-bundled-licenses.mjs`, the "Check dependency licenses" step in `.github/workflows/check.yml`
**Checked against the code:** 2026-11-18

## Marlin's license

- Marlin's own code is MIT. [`LICENSE`](../LICENSE) holds the text, and every `Cargo.toml` in the workspace says `license = "MIT"`.
- [`THIRD-PARTY-NOTICES.md`](../THIRD-PARTY-NOTICES.md) names the work of others that Marlin ships or uses, where its license asks for more than a copy of its text. That page owns those facts. This page owns the rules.

## Rules for a dependency

A dependency is anything that ends up in a binary or bundle Marlin ships: a Rust crate, an npm package the frontend imports, a C library a crate compiles, an icon set, a font.

1. **Every Rust crate and npm package that ships is under a license on the accepted list**, the `accepted` array in [`src-tauri/about.toml`](../src-tauri/about.toml). The list holds permissive licenses and the Mozilla Public License 2.0. When a package offers a choice, one accepted choice is enough. The check enforces this rule. An npm package whose JavaScript, CSS, fonts, or images the frontend imports goes under `dependencies` in `package.json`, even when only a stylesheet imports it, because the check skips `devDependencies`.
2. **No GPL, AGPL, or LGPL code is compiled into the app or the mount helper.** GPL and AGPL code would put the whole binary under those terms. LGPL code would oblige Marlin to give users what they need to relink the binary. Loading an LGPL library at run time is fine: GTK, WebKitGTK, and macFUSE's libfuse work this way. Two things are compiled in today and are allowed: libgit2, which is GPL with a linking exception that covers Marlin, and the LibXDiff code inside libgit2, which is LGPL. Anything else like these is the owner's decision.
3. **A Windows build holds only open-source code.** Windows mounts will load WinFsp ([decision 0001](decisions/0001-mount-architecture.md)), and WinFsp's license lets an open-source app do that only when the app is "not linked or distributed with proprietary (non-FLOSS) software". The check enforces this rule for the crates in `NOT_OPEN_SOURCE` in the script. UnRAR is the one entry, and only macOS and Linux builds link it.
4. **Read what the dependency builds, not only its label.** The check reads the license each package states. A crate that compiles or downloads other code, or a package of icons or fonts, can state MIT and carry something else: `unrar_sys`, `libgit2-sys`, `libdbus-sys`, `hayro-font`, `zpl-rs`, and `@iconify-json/vscode-icons` all do. Look in the package for a `vendor/` folder, a build script that compiles or downloads code, embedded fonts and images, and the upstream project's own license statement. Rules 2 and 3 apply to what you find, whatever the label says. When a crate builds in code, fonts, or data under a license its label does not name, add the crate to `CRATE_FILES` in `scripts/generate-license-texts.mjs`, so the installers carry that license too.
5. **A program Marlin runs but does not link may have any license**, as long as Marlin does not ship it. FFmpeg is downloaded on first use. The `marlin-smb` helper is a separate program because it links Samba, which is GPL. Putting such a program or library inside a bundle is a new decision: write a record in [`decisions/`](decisions/README.md). The AppImage already carries Samba's libraries: see [System libraries in the AppImage](../THIRD-PARTY-NOTICES.md#system-libraries-in-the-appimage).
6. **When a license asks for a notice, a credit, or anything else, add it to `THIRD-PARTY-NOTICES.md`** in the same branch.

## The check

`npm run check:licenses` runs `scripts/check-licenses.mjs`. It fails when:

- an npm package that can ship in the frontend (anything but a dev dependency) states a license that is not on the list, or states none. It reads `package-lock.json`. When a package states none, find its license and add the package to `UNSTATED` in the script, with a comment that says where the license is. A license that is not an SPDX expression (`SEE LICENSE IN ...`) fails too: the script has no way past that yet.
- a Rust crate that ships in the app or a helper states a license that is not on the list. This part is [cargo-about](https://github.com/EmbarkStudios/cargo-about), which reads `about.toml` and looks at every target in its `targets` list. Build and dev dependencies do not count.
- a Windows build would link a crate in `NOT_OPEN_SOURCE` (rule 3).
- `src-tauri/licenses/libzpl.txt` was written for another libzpl than the one the build downloads. The first line of the file names a libzpl release, and it must match the version of `zpl-rs` in `Cargo.lock`. See [libzpl's texts](#libzpls-texts).

CI runs it in `check.yml` whenever the static checks run, so a Dependabot update gets checked too. It is not part of `npm run check`, because it needs cargo-about installed. Install it with `npm run install:cargo-about`, which holds the one version every workflow uses. The check takes about a minute.

`scripts/check-licenses.test.mjs` tests the npm rule. `npm run test:run` runs it.

## Adding a license to the list

- A permissive license that is not on the list yet (the kind that asks only for its notice to be kept): add it to `about.toml` in the branch that needs it, and say so in the PR.
- Any copyleft license, or one that limits how the software may be used: this is the owner's decision. Ask first, then write a record in `decisions/`.

## License texts in the installers

Most licenses on the list ask that their text go with every copy. So every installer carries a `licenses` folder. It is a Tauri resource (`bundle.resources`).

| Installer                                          | Where the folder is                       |
| -------------------------------------------------- | ----------------------------------------- |
| macOS app, in the DMG and in the updater's archive | `Marlin.app/Contents/Resources/licenses/` |
| `.deb` and `.rpm`                                  | `/usr/lib/Marlin/licenses/`               |
| AppImage                                           | `usr/lib/Marlin/licenses/` in the image   |

The folder holds six files:

| File                         | Holds                                                                                                             | Comes from                                    |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| `LICENSE`                    | The license of Marlin's own code.                                                                                 | The repo root.                                |
| `THIRD-PARTY-NOTICES.md`     | The parts whose licenses ask for more than their text.                                                            | The repo root.                                |
| `THIRD-PARTY-LICENSES.txt`   | The license text of every Rust crate, of the code, fonts, and data that crates build in, and of the npm packages. | Made at build time.                           |
| `rust-standard-library.html` | The license texts of the Rust standard library, which is linked into each program.                                | Copied at build time from the Rust toolchain. |
| `libzpl.txt`                 | The license texts of libzpl and what it is built from.                                                            | `src-tauri/licenses/`, kept by hand.          |
| `GPL-3.0.txt`                | The license of the built `marlin-smb` helper.                                                                     | `src-tauri/licenses/`.                        |

### The two files made at build time

`scripts/run-tauri.mjs` calls `scripts/generate-license-texts.mjs` before every command that makes a bundle (`npm run tauri build` and `npm run tauri bundle`), for the target of that build. `tauri dev` and a `--no-bundle` build skip it. Git ignores both files. The generator needs cargo-about, and it fails the build when any text is missing. `THIRD-PARTY-LICENSES.txt` has three sections:

- **Rust crates.** cargo-about reads the license files of every crate in the workspace for the target, under the rules in `about.toml`.
- **More license texts from Rust crates.** The `CRATE_FILES` table in the script names license files that cargo-about leaves out. One kind is a part a crate builds in (code, fonts, data) under a license its label does not name (rule 4). The other is a crate's own license file that cargo-about cannot match, because one file holds several licenses or has an unusual name: cargo-about then prints a text with no copyright line. A file goes in when the target's dependency graph has its crate.
- **npm packages in the frontend.** Every package the check counts as shipping, and the `BUILD_TOOLS` in the script: Vite and Rolldown are dev dependencies, but each adds a few lines of its own to the bundle. The text is the package's own license file. A package that carries none needs an entry in `TEXT_ELSEWHERE` in the script, which says where its text is.

cargo-about gives no warning when it leaves a file out. To find such files after a large dependency change, generate the texts, then search them for the copyright lines of each `LICENSE*`, `COPYING*`, and `NOTICE*` file in the crates that ship. A crate whose own license file has no line in the output, or a nested license file of code the crate builds, needs an entry.

The generator leans one way: a text too many does no harm, and a missing one is the defect. It does not work out which helper a release job built. So the Intel Mac app, which has no SMB helper, still carries the texts of that helper's crates and `GPL-3.0.txt`.

`scripts/generate-license-texts.test.mjs` tests the three sections. `npm run test:run` runs it.

### libzpl's texts

A go-zpl release holds the library and no license text ([go-zpl #38](https://github.com/StirlingMarketingGroup/go-zpl/issues/38)). So `src-tauri/licenses/libzpl.txt` is kept by hand, and the check ties it to the `zpl-rs` version. When you upgrade `zpl-rs`:

1. Download the new release archive and run `go version -m` on the library. It lists the Go version and every Go module built in.
2. Read `render/font.go` in go-zpl at that tag for the fonts it embeds, and look for `LICENSE` files under `internal/`.
3. Change the parts of `libzpl.txt` that differ, and its first line.

### The check on a built bundle

`scripts/check-bundled-licenses.mjs` opens each installer a build made and fails when one of the six files is missing or empty, or when `THIRD-PARTY-LICENSES.txt` lacks a section. `release.yml` runs it in the dry run and in the release: see [Release signing](release-signing.md#the-license-texts-in-the-installers).

## Known gaps

- **The AppImage does not say where the source of its GPL and LGPL system libraries is** (#606). It does carry the copyright files of the system libraries it copies in.
- **`libzpl.txt` is kept by hand** ([go-zpl #38](https://github.com/StirlingMarketingGroup/go-zpl/issues/38)). The check catches a new `zpl-rs` version. It does not catch a build that sets `LIBZPL_VERSION` or `LIBZPL_PATH` to use another library.
- **The app has no screen that shows the licenses.** A user has to open the folder.
- **`libzpl` carries three commercial fonts** ([go-zpl #37](https://github.com/StirlingMarketingGroup/go-zpl/issues/37)). With them inside, `libzpl` does not pass rule 3. `zpl-rs` is not in `NOT_OPEN_SOURCE`, because every build links it today and the check would fail on every change.
- **The check trusts labels.** Rule 4 is a step a person does.
- **The check skips build tools.** Vite and Rolldown are dev dependencies, and each adds a few lines of its own to the frontend bundle (the module preload helper, the module helpers). The installers do carry the license text of both.

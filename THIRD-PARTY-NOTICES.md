# Third-party notices

Marlin's own code is under the MIT license. See [`LICENSE`](LICENSE).

Marlin also contains and uses work that other people made. This page names the parts whose licenses ask for more than a copy of their text, and the programs Marlin uses but does not ship. The rules for adding a dependency are in [`docs/licenses.md`](docs/licenses.md).

The source of each Marlin release is [this repository](https://github.com/StirlingMarketingGroup/marlin) at the tag of that release.

## What Marlin ships

### UnRAR (macOS and Linux only)

Marlin reads RAR archives with the UnRAR source code by Alexander Roshal, through the `unrar` crate. UnRAR is free to use, but its license is not an open-source license. Windows builds of Marlin do not contain it.

From the UnRAR license:

> UnRAR source code may be used in any software to handle RAR archives without limitations free of charge, but cannot be used to develop RAR (WinRAR) compatible archiver and to re-create RAR compression algorithm, which is proprietary. Distribution of modified UnRAR source code in separate form or as a part of other software is permitted, provided that full text of this paragraph, starting from "UnRAR source code" words, is included in license, or in documentation if license is not available, and in source code comments of resulting package.

The full license is `vendor/unrar/license.txt` in the [`unrar_sys` crate](https://crates.io/crates/unrar_sys).

### libgit2

Marlin reads Git status with [libgit2](https://libgit2.org), through the `git2` crate. libgit2 is under the GNU General Public License, version 2, with this linking exception:

> In addition to the permissions in the GNU General Public License, the authors give you unlimited permission to link the compiled version of this library into combinations with other programs, and to distribute those combinations without any restriction coming from the use of this file. (The General Public License restrictions do apply in other respects; for example, they cover modification of the file, and distribution when not linked into a combined executable.)

Marlin links libgit2 unmodified. The full license, and the licenses of most of the code libgit2 bundles, are in its [`COPYING`](https://github.com/libgit2/libgit2/blob/v1.9.4/COPYING) file. One of them asks for this line:

> This product includes software developed by Rich Salz.

libgit2 also bundles LibXDiff by Davide Libenzi, under the GNU Lesser General Public License, version 2.1 or later. `COPYING` holds the text of that license but does not name LibXDiff. Most files in libgit2's [`deps/xdiff/`](https://github.com/libgit2/libgit2/tree/v1.9.4/deps/xdiff) folder state it at the top. One file there, `xhistogram.c`, states the Eclipse Distribution License 1.0.

### OpenSSL (macOS and Linux)

The `git2` crate builds [OpenSSL](https://www.openssl.org) into the app. OpenSSL is under the Apache License 2.0.

### D-Bus (Linux only)

The `keyring` crate builds [libdbus](https://www.freedesktop.org/wiki/Software/dbus/) into the app and the mount helper. libdbus offers a choice of the Academic Free License 2.1 or the GPL, version 2 or later. Marlin uses it under the Academic Free License 2.1.

### libzpl

Marlin draws label previews with `libzpl`, a library built from [go-zpl](https://github.com/StirlingMarketingGroup/go-zpl) (MIT, Stirling Marketing Group). The `zpl-rs` crate downloads it when Marlin is built. It contains:

- The Go runtime (BSD 3-Clause, The Go Authors).
- Go packages: `boombuler/barcode`, `fogleman/gg`, and `skip2/go-qrcode` (MIT), `golang.org/x/image` and `golang.org/x/text` (BSD 3-Clause), and three barcode packages in go-zpl's `internal/` folder (MIT).
- Freetype-Go, used under the FreeType License, which asks for this credit:

  > Portions of this software are copyright © 2017 The FreeType Project (www.freetype.org). All rights reserved.

- Five fonts. Each file names its own owner:

  | Font                  | Owner and terms                                                                                            |
  | --------------------- | ---------------------------------------------------------------------------------------------------------- |
  | Noto Sans CJK SC Bold | © 2014-2021 Adobe. SIL Open Font License 1.1.                                                              |
  | DejaVu Sans Mono      | © 2003 Bitstream, Inc. The Bitstream Vera license.                                                         |
  | OCRB                  | © 2002-2008 Barcodesoft. The Barcodesoft Font License Agreement.                                           |
  | Monospace 821 Bold BT | © 1990-1993 Bitstream Inc. All rights reserved.                                                            |
  | Triumvirate Cond Bold | © 1990 Adobe Systems Incorporated. All rights reserved. CG Triumvirate is a trademark of Agfa Corporation. |

  The last three are commercial fonts, and go-zpl states no license that lets it pass them on. [go-zpl #37](https://github.com/StirlingMarketingGroup/go-zpl/issues/37) tracks this.

### File-type icons

The icons Marlin shows for file types are from [vscode-icons](https://github.com/vscode-icons/vscode-icons) by Roberto Huertas and contributors, through the `@iconify-json/vscode-icons` package. Marlin uses them unchanged. vscode-icons licenses its icons under the [Creative Commons Attribution-ShareAlike 4.0](https://creativecommons.org/licenses/by-sa/4.0/) license, except the icons that show a brand. Those keep the license of the brand's owner.

### Rust crates and npm packages

`src-tauri/Cargo.lock` and `package-lock.json` name Marlin's dependencies from [crates.io](https://crates.io) and [npm](https://www.npmjs.com), and the version of each. The Rust toolchain also links its standard library (MIT or Apache-2.0) into each binary.

- Each crate and package states a license on the accepted list in [`src-tauri/about.toml`](src-tauri/about.toml), except one npm package: `@crabnebula/tauri-plugin-drag` states no license in its package, and its [repository](https://github.com/crabnebula-dev/drag-rs) states Apache-2.0 or MIT. All of the licenses on the list are permissive, except the Mozilla Public License 2.0.
- These crates are under the Mozilla Public License 2.0: `cssparser`, `cssparser-macros`, `dtoa-short`, `freedesktop_entry_parser`, `linicon`, `linicon-theme`, `option-ext`, and `selectors`. Marlin uses them unmodified. Their source is on crates.io.
- To see every crate and the full text of its license, run `cargo about generate --workspace --format json` in `src-tauri/`.

### System libraries in the AppImage

The Linux AppImage carries copies of the system libraries it needs, taken from the Ubuntu release it was built on. The `.deb` and `.rpm` packages use the system's copies.

- GTK, WebKitGTK, and others are under the GNU Lesser General Public License.
- The [Samba](https://www.samba.org) libraries have more than one license. `libsmbclient` is under the GNU General Public License, version 3 or later, and `libwbclient` is under the GNU Lesser General Public License, version 3 or later. Only the `marlin-smb` helper loads them (see [Samba](#samba-macos-deb-and-rpm) below).
- The AppImage holds the copyright file of each Ubuntu package a library came from, under `usr/share/doc/`. The Ubuntu page of each package names its source package. For example, `libsmbclient0` comes from the source package `samba`.

## What Marlin uses but does not ship

### Samba (macOS, `.deb`, and `.rpm`)

Marlin reaches SMB shares through a helper program, `marlin-smb`. Outside the AppImage, the helper loads `libsmbclient` from the Samba that you install.

- The helper's source code is MIT, like the rest of Marlin. Because the built helper links a GPL library, you receive the built helper under the GPL, version 3 or later.
- The helper is a separate program. The Marlin app starts it and talks to it over a pipe. The app itself does not link Samba.

### FFmpeg

Marlin draws video thumbnails with [FFmpeg](https://ffmpeg.org). Marlin does not ship FFmpeg. The first time it needs a video thumbnail, it downloads an FFmpeg program to your cache folder and runs it as a separate program. FFmpeg is under the LGPL or the GPL, depending on how it was built. Marlin downloads these builds:

| System  | Build                                                                                                            |
| ------- | ---------------------------------------------------------------------------------------------------------------- |
| macOS   | [evermeet.cx](https://evermeet.cx/ffmpeg/) (Intel), [osxexperts.net](https://www.osxexperts.net) (Apple Silicon) |
| Linux   | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), the GPL build                                       |
| Windows | [gyan.dev](https://www.gyan.dev/ffmpeg/builds/)                                                                  |

### FUSE

Mounts need FUSE on Linux and [macFUSE](https://macfuse.github.io) on macOS. You install them. Marlin does not ship either one. See the license table in [`docs/decisions/0001-mount-architecture.md`](docs/decisions/0001-mount-architecture.md#licenses).

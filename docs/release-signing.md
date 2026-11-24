# Release signing

A release is built, signed, and notarized by `.github/workflows/release.yml`. Pushing a `v*` tag, or running the workflow by hand with a tag, starts it. Signing needs six Apple secrets and two Tauri updater secrets in the repository settings. The build also needs Marlin's Google OAuth client.

**Covers:** `.github/workflows/release.yml`, `src-tauri/Entitlements.plist`, `src-tauri/tauri.conf.json`, `scripts/check-linux-package-libraries.mjs`, `scripts/check-bundled-licenses.mjs`, `src-tauri/build.rs`
**Checked against the code:** 2026-11-24

## What the workflow does

| Job               | Runs when                                                       | Does                                                                                                                                                      |
| ----------------- | --------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sync-version`    | a `v*` tag, or a manual run with a tag                          | Writes the version into `package.json` and `tauri.conf.json`, commits it, and makes sure the tag exists.                                                  |
| `release`         | after `sync-version`, never on a PR                             | Builds the helpers, builds and publishes installers, and on macOS signs, makes a DMG, and notarizes it.                                                   |
| `dry-run-changes` | every PR and every push to `master`                             | Checks which files changed and decides whether the dry run needs to build.                                                                                |
| `release-dry-run` | a PR or a push to `master` that changes something in the bundle | Builds the bundle without publishing, and checks its mount helper, the Linux package libraries, and the license texts. It also keeps the Rust cache warm. |

The first `release` job to publish creates the GitHub release. Its title is the tag. Its notes are the download line from the workflow, then GitHub's generated notes: the PRs merged since the previous release (`generateReleaseNotes` in "Build and publish installers").

Build targets:

| Target                      | Runner                    | SMB helper | Mount helper |
| --------------------------- | ------------------------- | ---------- | ------------ |
| `aarch64-apple-darwin`      | GitHub `macos-15`         | yes        | yes          |
| `x86_64-apple-darwin`       | GitHub `macos-15-intel`   | no         | yes          |
| `x86_64-unknown-linux-gnu`  | self-hosted               | yes        | yes          |
| `aarch64-unknown-linux-gnu` | GitHub `ubuntu-24.04-arm` | yes        | yes          |

There is no Windows build (issue #135).

## The two helpers

Both helpers are Tauri `externalBin` files in `src-tauri/binaries/`. The bundler puts each one beside the app binary.

- **`marlin-smb`** is built by the "Build SMB sidecar" step, on the targets where samba is installed. On the other target the build script leaves an empty placeholder, and that app has no SMB support.
- **`marlin-mount`** has no step. "Build and publish installers" and "Build Tauri bundle" both run `npm run tauri build`, and that script builds the helper first. See [Mount helper](mount-helper.md#building-and-bundling).
- **Signing.** On macOS the bundler signs both helpers inside the app, with the hardened runtime and the app's entitlements. In `release`, when a signing identity is set, `marlin-smb` is also signed by its own step before the bundler runs. `marlin-mount` needs `com.apple.security.cs.disable-library-validation` from those entitlements, or it cannot load macFUSE: see [Mount helper](mount-helper.md#building-and-bundling).
- **The check.** "Check the bundled mount helper" starts `marlin-mount` from the built bundle and verifies the macOS signatures. The dry run signs ad hoc so that the same check runs on a PR. In `release` that step, the Linux package library check, and the license text check run last, after every installer is uploaded: the dry run is the gate, and those steps make a bad release show as a failed job.

`release` and `release-dry-run` repeat about 200 lines of the same setup steps. Change both when you change one.

Both jobs set `MACOSX_DEPLOYMENT_TARGET` to `10.13`. `tauri build` sets that variable from `bundle.macOS.minimumSystemVersion` (default `10.13`), and the plain `cargo build` for a helper does not. When the two disagree, every crate that compiles C is rebuilt by each step. If you set `minimumSystemVersion` in `tauri.conf.json`, change the workflow value to match.

A newer push cancels a running PR build, but never a `master` build. Only `master` builds save the Rust cache, and a cancelled one would save a partial cache under a key that is never written again. If a cache does get poisoned, delete it with `gh cache delete <id>` and the next `master` build saves a full one.

The Linux step that installs system packages stops after 10 minutes. It normally takes about a minute. A runner that cannot reach the Ubuntu mirror fails there, and rerunning the job gets a new runner. `check.yml` has the same limit.

The dry run is skipped when a change touches nothing that goes into the bundle. The list of paths is in the `dry-run-changes` job. Add a path there when a new file starts to affect the build.

## The libzpl library in the Linux packages

| Package  | Where `libzpl.so` sits      | How the binary finds it                              |
| -------- | --------------------------- | ---------------------------------------------------- |
| deb      | `/usr/lib/marlin/libzpl.so` | by the rpath that `build.rs` sets                    |
| rpm      | `/usr/lib/marlin/libzpl.so` | by the rpath that `build.rs` sets                    |
| AppImage | `usr/lib/libzpl.so`         | by the rpath that linuxdeploy sets, `$ORIGIN/../lib` |

The two package maps (`deb.files`, `rpm.files`) each name the folder. `build.rs` takes the rpath from the deb map. "Check the libraries in the Linux packages" extracts the deb and the rpm and fails when a program in `usr/bin` needs a library that neither the build machine nor the package's rpath folder holds. The arm64 build makes only the deb, and that job checks only the deb. Both jobs delete the bundle folder before the build, so the check reads the packages that build made.

The check does not look at the AppImage. It does not install a package on a clean machine. A library the build machine has, and that the package's `depends` does not name, still passes.

## The license texts in the installers

Both jobs install cargo-about, because `npm run tauri build` writes the license texts with it before it makes a bundle. "Check the license texts in the installers" then reads what the job made: the built app on macOS, the deb on arm64 Linux, and the deb, the rpm, and the AppImage on x86_64 Linux. [Licenses](licenses.md#license-texts-in-the-installers) says what the files are and where they sit.

## Secrets

| Secret                       | Value                                                                                 | Used by                        |
| ---------------------------- | ------------------------------------------------------------------------------------- | ------------------------------ |
| `APPLE_CERTIFICATE`          | The Developer ID Application certificate as a base64-encoded `.p12`                   | "Import Apple Certificate"     |
| `APPLE_CERTIFICATE_PASSWORD` | The password set when exporting the `.p12`                                            | "Import Apple Certificate"     |
| `APPLE_SIGNING_IDENTITY`     | The certificate's name, such as `Developer ID Application: Example Corp (A1B2C3D4E5)` | codesign steps                 |
| `APPLE_ID`                   | The Apple ID email                                                                    | "Notarize DMG"                 |
| `APPLE_PASSWORD`             | An app-specific password for that Apple ID                                            | "Notarize DMG"                 |
| `APPLE_TEAM_ID`              | The 10-character team ID                                                              | "Notarize DMG"                 |
| `TAURI_PRIVATE_KEY`          | The Tauri updater signing key                                                         | "Build and publish installers" |
| `TAURI_KEY_PASSWORD`         | The password for that key                                                             | "Build and publish installers" |
| `GOOGLE_CLIENT_ID`           | The client ID of Marlin's Google OAuth client                                         | "Build and publish installers" |
| `GOOGLE_CLIENT_SECRET`       | The client secret of that OAuth client                                                | "Build and publish installers" |
| `APP_ID`, `APP_PRIVATE_KEY`  | The GitHub App that commits the version bump                                          | `sync-version`                 |

Without `APPLE_CERTIFICATE` the import step is skipped. Without `APPLE_SIGNING_IDENTITY` the build signs ad hoc (`-`), which Gatekeeper does not accept. Without both Google secrets the `release` job stops at its first step, "Check the Google sign-in client": the app would build but could not sign in to Google. The dry run does not get them, so its bundle has no OAuth client.

## Setting up Apple signing from nothing

You need a paid Apple Developer Program membership.

1. **Make the certificate.** In Xcode: Settings → Accounts → your Apple ID → Manage Certificates → **+** → Developer ID Application. Or make it in the [developer portal](https://developer.apple.com/account/resources/certificates/list) from a certificate signing request.
2. **Export it.** In Keychain Access, open My Certificates, right-click "Developer ID Application: …", choose Export, and save a `.p12` with a strong password.
3. **Encode it.** `base64 -i path/to/certificate.p12 | pbcopy` puts the value for `APPLE_CERTIFICATE` on the clipboard.
4. **Make an app-specific password.** At [appleid.apple.com](https://appleid.apple.com/), under App-Specific Passwords. This is `APPLE_PASSWORD`.
5. **Find the team ID.** At [developer.apple.com/account](https://developer.apple.com/account), under Membership Details.
6. **Add the secrets** in the repository: Settings → Secrets and variables → Actions.
7. **Check it.** Run a release and read the log. "Import Apple Certificate" succeeds, the codesign steps pass, and `notarytool` ends with "Accepted".

A local `npm run tauri build` signs with the Developer ID certificate in your keychain when one is installed.

## When it fails

| Message                                                 | Fix                                                                                             |
| ------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| "The specified item could not be found in the keychain" | The import failed. Check that `APPLE_CERTIFICATE_PASSWORD` matches the `.p12` password exactly. |
| "Developer ID Application certificate not found"        | `APPLE_SIGNING_IDENTITY` must match the certificate name exactly, team ID included.             |
| Notarization: "Invalid credentials"                     | Make a new app-specific password. Check `APPLE_ID`, and that the membership is active.          |
| "Code signature invalid"                                | Check `src-tauri/Entitlements.plist` exists and lists only entitlements the app can have.       |

## References

- [Tauri: macOS code signing](https://v2.tauri.app/distribute/sign/macos/)
- [Apple: notarizing macOS software](https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution)

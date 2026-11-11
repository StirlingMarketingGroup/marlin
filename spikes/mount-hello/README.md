# mount-hello

Evidence for [decision 0001](../../docs/decisions/0001-mount-architecture.md). It is a read-only filesystem with two files. A read of `slow.txt` is answered three seconds later, so the runs can show that other calls finish during that wait.

This is not the start of the real adapters. Issues #373 and #374 replace it.

| File                        | What it is                                                         |
| --------------------------- | ------------------------------------------------------------------ |
| `src/main.rs`, `Cargo.toml` | The Linux program, on the `fuser` crate.                           |
| `run-linux.sh`              | Builds and runs the Linux program in Docker.                       |
| `macos-hello.c`             | The macOS program, on the low-level API of macFUSE's libfuse 3.    |
| `run-macos.sh`              | Builds and runs the macOS program through macFUSE's FSKit backend. |
| `windows/`                  | The Windows program, on WinFsp through bindings written here.      |

## Run on Linux

```bash
spikes/mount-hello/run-linux.sh
```

It needs Docker. The container is given `/dev/fuse`, `CAP_SYS_ADMIN`, and no AppArmor profile. A non-root user mounts through the setuid `fusermount3`. The script does not cover a bare host without a container.

## Run on macOS

```bash
spikes/mount-hello/run-macos.sh
```

It needs macOS 15.4 or later, the Xcode command line tools, `pkg-config`, and macFUSE 5. Turn on both "macFUSE" and "macFUSE (local)" under System Settings, General, Login Items & Extensions, File System Extensions. The script mounts at a new `~/marlin-spike-mnt.XXXXXX` folder and removes it when it ends.

The Rust program does not work on macOS: `fuser` cannot use the FSKit backend. The decision record says why.

## Run on Windows

```bat
mount-hello-windows.exe self-test "%USERPROFILE%\marlin-spike-mnt"
```

It needs WinFsp 2.1. WinFsp creates the mount folder, so the path must not exist yet and its parent must. `windows/src/winfsp.rs` holds the bindings. They follow WinFsp's headers, because the `winfsp` crate is GPL-3.0.

No script builds the program. To build it for Windows on ARM64 from a Mac:

```bash
rustup target add aarch64-pc-windows-gnullvm
cd spikes/mount-hello/windows
CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER="$LLVM_MINGW/bin/aarch64-w64-mingw32-clang" \
  CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --release --target aarch64-pc-windows-gnullvm
```

`$LLVM_MINGW` is an unpacked [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) release. Without `crt-static` the program needs `libunwind.dll` beside it. A build on Windows itself is untested.

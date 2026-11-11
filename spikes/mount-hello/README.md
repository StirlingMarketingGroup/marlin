# mount-hello

Evidence for [decision 0001](../../docs/decisions/0001-mount-architecture.md). It is a read-only filesystem with two files. A read of `slow.txt` is answered three seconds later, so the runs can show that other calls finish during that wait.

This is not the start of the real adapters. Issues #373 and #374 replace it.

| File                        | What it is                                                         |
| --------------------------- | ------------------------------------------------------------------ |
| `src/main.rs`, `Cargo.toml` | The Linux program, on the `fuser` crate.                           |
| `run-linux.sh`              | Builds and runs the Linux program in Docker.                       |
| `macos-hello.c`             | The macOS program, on the low-level API of macFUSE's libfuse 3.    |
| `run-macos.sh`              | Builds and runs the macOS program through macFUSE's FSKit backend. |

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

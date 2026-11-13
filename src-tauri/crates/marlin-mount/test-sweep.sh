#!/usr/bin/env bash
# Makes a dead mount and a live one, then checks that the helper clears only the dead one when it starts.
# Runs on Linux or macOS, as a user who is not root. The helper cannot mount anything real yet,
# so the mounts come from the spike programs in spikes/mount-hello.
# Linux needs fuse3. macOS needs macFUSE 5 with both of its file system extensions turned on.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
work=$(mktemp -d)
# A fresh folder per run, so the script never touches a mount it did not make.
root=$(mktemp -d "$HOME/marlin-sweep-test.XXXXXX")
pids=()

case "$(uname -s)" in
  Linux)
    hello="${CARGO_TARGET_DIR:-$repo/spikes/mount-hello/target}/debug/mount-hello"
    build_hello() { cargo build --locked --manifest-path "$repo/spikes/mount-hello/Cargo.toml"; }
    serve() { exec "$hello" serve "$1"; }
    is_mounted() {
      awk -v path="$1" '$5 == path { found = 1 } END { exit !found }' /proc/self/mountinfo
    }
    force_unmount() { fusermount3 -u -z -- "$1"; }
    ;;
  Darwin)
    build_hello() {
      # The Darwin extensions swap struct stat for macFUSE's own types. Off, the API matches Linux.
      # shellcheck disable=SC2046
      cc -Wall -Werror -DFUSE_DARWIN_ENABLE_EXTENSIONS=0 "$repo/spikes/mount-hello/macos-hello.c" -o "$work/hello" \
        $(pkg-config fuse3 --cflags --libs)
    }
    serve() { exec "$work/hello" -o ro,fsname=marlin-hello,volname=MarlinHello,backend=fskit "$1"; }
    # No `grep -q`: it stops reading at the first match, and `pipefail` would then report the failed `mount`.
    is_mounted() { mount | grep -F " on $1 (" >/dev/null; }
    force_unmount() { umount -f "$1"; }
    ;;
  *)
    echo "FAIL: this script runs on Linux or macOS"
    exit 1
    ;;
esac

cleanup() {
  # Written this way because the bash that ships with macOS fails on an empty array under `set -u`.
  for pid in ${pids[@]+"${pids[@]}"}; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  for mnt in "$root/dead" "$root/live"; do
    if is_mounted "$mnt"; then
      force_unmount "$mnt" || echo "CLEANUP FAILED: unmount '$mnt' by hand"
    fi
  done
  # Only the file and the folders this script made, and never a recursive delete next to a mount.
  rm -f "$root/files/keep.txt"
  rmdir "$root/dead" "$root/live" "$root/empty" "$root/files" "$root" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

# Starts a mount at $1 in the background and waits until the mount table has it. Sets $mount_pid.
# On macOS the volume shows up one to three seconds after the program starts.
start_mount() {
  serve "$1" >"$work/$(basename "$1").log" 2>&1 &
  mount_pid=$!
  pids+=("$mount_pid")
  for _ in $(seq 1 150); do
    if is_mounted "$1"; then
      return
    fi
    sleep 0.1
  done
  echo "FAIL: did not mount $1"
  cat "$work/$(basename "$1").log"
  exit 1
}

build_hello
(cd "$repo/src-tauri" && cargo build --locked -p marlin-mount)
helper="${CARGO_TARGET_DIR:-$repo/src-tauri/target}/debug/marlin-mount"

mkdir "$root/dead" "$root/live" "$root/empty" "$root/files"
echo "keep me" >"$root/files/keep.txt"

echo "=== set up: a dead mount with a process inside it, and a live mount ==="
start_mount "$root/dead"
# A working folder inside the mount makes a plain unmount fail, so the helper has to force it.
# The process starts inside the folder, so it is there before the server is killed.
pushd "$root/dead" >/dev/null
sleep 600 &
pids+=("$!")
popd >/dev/null
kill -9 "$mount_pid"
wait "$mount_pid" 2>/dev/null || true
for _ in $(seq 1 50); do
  if ! ls "$root/dead" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
if ls "$root/dead" >/dev/null 2>&1; then
  echo "FAIL: a listing of the dead mount still works"
  exit 1
fi
if ! is_mounted "$root/dead"; then
  echo "FAIL: the dead mount left the mount table before the helper ran"
  exit 1
fi
echo "PASS set up: the mount table still has the dead mount, and a listing of it fails"
start_mount "$root/live"

echo "=== start the helper ==="
"$helper" --socket "$work/mount.sock" --root "$root" 2>"$work/helper.log" &
helper_pid=$!
pids+=("$helper_pid")
# Both checks: on Linux a dead mount can fail the "is it there" test while it is still mounted.
for _ in $(seq 1 150); do
  if ! is_mounted "$root/dead" && [[ ! -e "$root/dead" ]]; then
    break
  fi
  sleep 0.1
done
# The helper finishes its sweep before it acts on a signal, so once it has exited the sweep is complete.
kill -TERM "$helper_pid" 2>/dev/null || true
if ! wait "$helper_pid"; then
  echo "FAIL: the helper did not exit 0 after SIGTERM"
  cat "$work/helper.log"
  exit 1
fi

if is_mounted "$root/dead" || [[ -e "$root/dead" ]]; then
  echo "FAIL: the dead mount or its folder is still there"
  cat "$work/helper.log"
  exit 1
fi
echo "PASS: the dead mount is unmounted and its folder is gone"
if [[ -e "$root/empty" ]]; then
  echo "FAIL: the empty folder is still there"
  exit 1
fi
echo "PASS: the empty folder is gone"
if [[ "$(cat "$root/files/keep.txt")" != "keep me" ]]; then
  echo "FAIL: files/keep.txt changed"
  exit 1
fi
echo "PASS: the folder that holds a file is as it was"
if ! is_mounted "$root/live" || [[ "$(cat "$root/live/hello.txt")" != "Hello from Marlin" ]]; then
  echo "FAIL: the live mount is gone or does not serve hello.txt"
  exit 1
fi
echo "PASS: the live mount still serves its file"
if grep -q 'helper.dead_mount' "$work/helper.log"; then
  echo "FAIL: the helper logged a dead mount it could not clear"
  cat "$work/helper.log"
  exit 1
fi
echo "PASS: the helper logged no dead mount it could not clear"
echo "=== all steps passed ==="

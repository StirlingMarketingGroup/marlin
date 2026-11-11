#!/usr/bin/env bash
# Mounts macos-hello.c through macFUSE's FSKit backend, which needs no kernel extension.
# Needs macFUSE 5 with both of its file system extensions turned on in System Settings.
set -euo pipefail

src=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
build=$(mktemp -d)
# A fresh folder per run, so the script never touches a mount it did not make.
mnt=$(mktemp -d "$HOME/marlin-spike-mnt.XXXXXX")
pid=

cleanup() {
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid"
    wait "$pid" 2>/dev/null || true
  fi
  if mount | grep -q " $mnt "; then
    umount "$mnt" || echo "CLEANUP FAILED: run: umount '$mnt'"
  fi
  rmdir "$mnt" 2>/dev/null || true
  rm -rf "$build"
}
trap cleanup EXIT

echo "=== environment ==="
sw_vers -productVersion
uname -m
defaults read /Library/Filesystems/macfuse.fs/Contents/Info.plist CFBundleShortVersionString
pkg-config --modversion fuse3

echo "=== build ==="
# The Darwin extensions swap struct stat for macFUSE's own types. Off, the API matches Linux.
# shellcheck disable=SC2046
cc -Wall -Werror -DFUSE_DARWIN_ENABLE_EXTENSIONS=0 "$src/macos-hello.c" -o "$build/hello" \
  $(pkg-config fuse3 --cflags --libs)

# Starts the filesystem in the background and waits for the mount to show up. Sets $pid.
# The program's "mounted" line comes too early: with the FSKit backend the volume
# appears one to three seconds after fuse_session_mount returns.
start_serve() {
  "$build/hello" -o ro,fsname=marlin-hello,volname=MarlinHello,backend=fskit "$mnt" \
    > "$build/serve.log" 2>&1 &
  pid=$!
  for _ in $(seq 1 150); do
    if mount | grep -q " $mnt "; then
      return
    fi
    sleep 0.1
  done
  echo "FAIL: did not mount $mnt"
  cat "$build/serve.log"
  exit 1
}

echo "=== step A: mount, list, read, overlap, read-only, unmount ==="
start_serve
mount | grep " $mnt "
[[ "$(ls "$mnt" | tr '\n' ' ')" == "hello.txt slow.txt " ]]
echo "PASS list"
[[ "$(cat "$mnt/hello.txt")" == "Hello from Marlin" ]]
echo "PASS read"
cat "$mnt/slow.txt" > "$build/slow.out" &
slow=$!
# Wait until the slow read has reached the filesystem.
for _ in $(seq 1 50); do
  if grep -q '^slow read pending' "$build/serve.log"; then
    break
  fi
  sleep 0.1
done
grep -q '^slow read pending' "$build/serve.log"
ls "$mnt" > /dev/null
cat "$mnt/hello.txt" > /dev/null
# A name nothing has asked for yet, so the OS cannot answer it from a cache.
if ls "$mnt/never-seen.txt" 2> /dev/null; then
  echo "FAIL: a missing file was listed"
  exit 1
fi
# The 3-second read must still be waiting after the fast calls return,
# and at least one of them must have reached the filesystem.
kill -0 "$slow"
grep '^callback during slow read' "$build/serve.log"
wait "$slow"
[[ "$(cat "$build/slow.out")" == "slow" ]]
echo "PASS concurrent: a listing, a read, and a lookup finished while the slow read was waiting"
if touch "$mnt/new.txt" 2> "$build/touch.err"; then
  echo "FAIL: created a file on a read-only mount"
  exit 1
fi
echo "PASS read-only: $(cat "$build/touch.err")"
kill -TERM "$pid"
wait "$pid"
if mount | grep -q " $mnt "; then
  echo "FAIL: still mounted after SIGTERM"
  exit 1
fi
echo "PASS unmount: SIGTERM, exit 0"

echo "=== step B: kill -9 ==="
start_serve
kill -9 "$pid"
wait "$pid" 2>/dev/null || true
sleep 3
echo "--- read after kill ---"
if cat "$mnt/hello.txt"; then
  echo "FAIL: a read still worked after the server was killed"
  exit 1
fi
echo "--- mount table ---"
# grep fails the script here if the dead mount is not in the table.
mount | grep " $mnt "
echo "--- umount ---"
umount "$mnt"
if mount | grep -q " $mnt "; then
  echo "FAIL: still mounted after umount"
  exit 1
fi
echo "PASS: a plain umount cleared the dead mount"
echo "=== all steps passed ==="

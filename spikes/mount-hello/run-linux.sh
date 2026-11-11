#!/usr/bin/env bash
# Prove the unprivileged FUSE path: a non-root user mounts through fusermount3.
set -euo pipefail

src=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

docker run --rm \
  --device /dev/fuse \
  --cap-add SYS_ADMIN \
  --security-opt apparmor=unconfined \
  -v "${src}:/src:ro" \
  rust:1-slim-bookworm \
  bash -c "$(cat <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq fuse3
useradd -m spike
mkdir /tmp/build
# Skip target/ so a host build cannot race the copy or land in the image.
tar -C /src --exclude target -cf - . | tar -C /tmp/build -xf -
chown -R spike:spike /tmp/build
cat > /tmp/as-spike.sh << 'SPIKE'
set -euo pipefail
cd /tmp/build
echo "=== environment ==="
id
uname -r
fusermount3 --version
ls -l "$(command -v fusermount3)"
echo "The container was given /dev/fuse, CAP_SYS_ADMIN, and no AppArmor profile. The non-root user mounts through the setuid fusermount3."

echo "=== build ==="
cargo build --locked
bin=/tmp/build/target/debug/mount-hello

echo "=== step A: self-test ==="
"$bin" self-test /home/spike/mnt

# Starts `serve` in the background and waits for its "mounted" line. Sets $pid.
start_serve() {
  mkdir -p "$1"
  "$bin" serve "$1" > /tmp/serve.log 2>&1 &
  pid=$!
  for _ in $(seq 1 100); do
    if grep -q '^mounted ' /tmp/serve.log; then
      return
    fi
    sleep 0.1
  done
  echo "FAIL: serve did not mount $1"
  cat /tmp/serve.log
  exit 1
}

echo "=== step B: kill -9 ==="
start_serve /home/spike/mnt2
echo "--- ls before kill ---"
ls /home/spike/mnt2
echo "--- kill -9 ${pid} ---"
kill -9 "$pid" || true
set +e
wait "$pid"
set -e
sleep 0.2
echo "--- ls after kill ---"
if ls /home/spike/mnt2; then
  echo "FAIL: a listing still worked after the server was killed"
  exit 1
fi
echo "--- /proc/mounts ---"
# grep fails the script here if the dead mount is not in the table.
grep ' /home/spike/mnt2 ' /proc/mounts
echo "--- fusermount3 -u ---"
fusermount3 -u /home/spike/mnt2
if grep -q ' /home/spike/mnt2 ' /proc/mounts; then
  echo "FAIL: /home/spike/mnt2 is still in /proc/mounts"
  exit 1
fi
echo "PASS: /home/spike/mnt2 is gone from /proc/mounts"

echo "=== step C: SIGTERM ==="
start_serve /home/spike/mnt3
kill -TERM "$pid"
set +e
wait "$pid"
code=$?
set -e
echo "serve exit code: ${code}"
if [[ "$code" -ne 0 ]]; then
  echo "FAIL: expected exit 0 after SIGTERM"
  cat /tmp/serve.log
  exit 1
fi
if grep -q ' /home/spike/mnt3 ' /proc/mounts; then
  echo "FAIL: /home/spike/mnt3 is still in /proc/mounts"
  exit 1
fi
echo "PASS: /home/spike/mnt3 is gone from /proc/mounts"
echo "=== all steps passed ==="
SPIKE
chmod 755 /tmp/as-spike.sh
su -s /bin/bash spike -c 'bash /tmp/as-spike.sh'
EOF
)"

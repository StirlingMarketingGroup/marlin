#!/usr/bin/env bash
# The container is given /dev/fuse, CAP_SYS_ADMIN, and no AppArmor profile.
# A non-root user mounts through the setuid fusermount3.
set -euo pipefail

src=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)

docker run --rm \
  --device /dev/fuse \
  --cap-add SYS_ADMIN \
  --security-opt apparmor=unconfined \
  -v "${src}:/src:ro" \
  -v marlin-fuse-test-registry:/usr/local/cargo/registry \
  -v marlin-fuse-test-target:/target \
  rust:1-slim-bookworm \
  bash -c "$(cat <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq fuse3
useradd -m tester
chown tester /usr/local/cargo/registry /target
cat > /tmp/as-tester.sh << 'TEST'
set -euo pipefail
cd /src
export CARGO_TARGET_DIR=/target
echo "=== environment ==="
id
uname -r
fusermount3 --version
# marlin-mount is here because its Linux half, the wiring to this adapter, is compiled out on a Mac.
# One test at a time: see the note at the top of tests/mount.rs.
cargo test --locked -p marlin-fuse -p marlin-mount -- --include-ignored --test-threads=1
TEST
chmod 755 /tmp/as-tester.sh
su -s /bin/bash tester -c 'bash /tmp/as-tester.sh'
EOF
)"

#!/usr/bin/env bash
# Runs the SMB helper tests against Samba's smbd.
# The container gets no device and no extra capability. smbd runs as the user tester.
set -euo pipefail

src=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)

docker run --rm \
  -v "${src}:/src:ro" \
  -v marlin-smb-test-registry:/usr/local/cargo/registry \
  -v marlin-smb-test-target:/target \
  rust:1-slim-bookworm \
  bash -c "$(cat <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# procps: a test server stops its samba-dcerpcd with pkill.
apt-get install -y -qq samba smbclient libsmbclient-dev pkg-config procps
useradd -m tester
chown tester /usr/local/cargo/registry /target
cat > /tmp/as-tester.sh << 'TEST'
set -euo pipefail
cd /src/src-tauri
export CARGO_TARGET_DIR=/target
export RUSTFLAGS="-D warnings"
cargo test --locked -p marlin-smb
TEST
chmod 755 /tmp/as-tester.sh
su -s /bin/bash tester -c 'bash /tmp/as-tester.sh'
EOF
)"

#!/usr/bin/env bash
# Runs the app's own Rust tests that mount a real FUSE filesystem, on Linux, from a Mac.
# The container is given /dev/fuse, CAP_SYS_ADMIN, and no AppArmor profile.
# A non-root user mounts through the setuid fusermount3.
set -euo pipefail

# The repo root. The app's build writes into src-tauri/, so the copy below is what cargo tests.
src=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

docker run --rm \
  --device /dev/fuse \
  --cap-add SYS_ADMIN \
  --security-opt apparmor=unconfined \
  -v "${src}:/src:ro" \
  -v marlin-app-rust-test-registry:/usr/local/cargo/registry \
  -v marlin-app-rust-test-target:/target \
  rust:1-slim-bookworm \
  bash -c "$(cat <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# The build packages are the ones check.yml installs, plus fuse3 for the mounts.
apt-get install -y -qq build-essential clang libclang-dev libssl-dev pkg-config \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  fuse3
useradd -m tester
# The source is mounted read-only. Work on a copy without the host dependencies and build output, and without the agent worktrees in .claude.
mkdir -p /work
tar -C /src --exclude=./node_modules --exclude=./src-tauri/target --exclude=./dist --exclude=./.git --exclude=./.claude \
  --exclude=./test-results -cf - . | tar -C /work -xf -
chown -R tester /work /usr/local/cargo/registry /target
cat > /tmp/as-tester.sh << 'TEST'
set -euo pipefail
cd /work/src-tauri
export CARGO_TARGET_DIR=/target
echo "=== environment ==="
id
uname -r
fusermount3 --version
# The tests marked ignore in the library of the app are the ones that mount. No apostrophe in this block: bash 3.2 misreads one here.
# One test at a time: a mount test forks, and until the child calls exec it holds every file this process has open.
cargo test --locked --lib -- --ignored --test-threads=1
TEST
chmod 755 /tmp/as-tester.sh
su -s /bin/bash tester -c 'bash /tmp/as-tester.sh'
EOF
)"

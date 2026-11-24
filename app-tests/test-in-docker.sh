#!/usr/bin/env bash
# Runs `npm run test:app` on Linux, in a container with a virtual display.
# A failed test's screenshot lands in test-results/app/ of this checkout.
set -euo pipefail

src=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "${src}/test-results/app"

docker run --rm \
  -v "${src}:/src:ro" \
  -v "${src}/test-results/app:/screenshots" \
  -v marlin-app-test-registry:/usr/local/cargo/registry \
  -v marlin-app-test-target:/work/src-tauri/target \
  rust:1-slim-bookworm \
  bash -c "$(cat <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates
# The Node that Debian ships is too old for Vite.
curl -fsSL https://deb.nodesource.com/setup_24.x | bash - > /dev/null
# The build packages are the ones check.yml installs. xvfb is the virtual display.
# The SFTP test runs its own sshd. Copy and paste run xclip.
apt-get install -y -qq nodejs build-essential clang libclang-dev libssl-dev pkg-config \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  procps xvfb xauth xclip openssh-server
useradd -m tester
# The source is mounted read-only, and its node_modules and target folders were built on a Mac. Work on a copy without them.
tar -C /src --exclude=./node_modules --exclude=./src-tauri/target --exclude=./dist --exclude=./.git \
  --exclude=./test-results -cf - . | tar -C /work -xf -
chown -R tester /work /usr/local/cargo/registry
cat > /tmp/as-tester.sh << 'TEST'
set -euo pipefail
cd /work
echo "=== environment ==="
uname -srm
node --version
pkg-config --modversion webkit2gtk-4.1
npm ci
xvfb-run -a npm run test:app
TEST
chmod 755 /tmp/as-tester.sh
status=0
su -s /bin/bash tester -c 'bash /tmp/as-tester.sh' || status=$?
# The screenshots of failed tests go back to the checkout, owned like the folder they land in.
if [ -d /work/test-results/app ]; then
  cp -r /work/test-results/app/. /screenshots/
  chown -R "$(stat -c %u:%g /screenshots)" /screenshots
fi
exit "$status"
EOF
)"

#!/usr/bin/env bash
# Runs the mount tests on this Mac.
# Needs macFUSE 5 with both of its file system extensions turned on.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

cargo test --locked -p marlin-fuse -- --include-ignored --test-threads=1
cargo test --locked -p marlin-mount -- --include-ignored --test-threads=1
bash crates/marlin-mount/test-sweep.sh

#!/usr/bin/env bash
set -euo pipefail

REPO="StirlingMarketingGroup/marlin"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

fatal() {
  echo "Error: $*" >&2
  exit 1
}

require() {
  command -v "$1" >/dev/null 2>&1 || fatal "Required command '$1' is not available."
}

require curl
require python3

OS=$(uname -s)
ARCH=$(uname -m)

TMPDIR=$(mktemp -d 2>/dev/null || mktemp -d -t marlin)
MOUNT_POINT=""
cleanup() {
  if [[ -n "$MOUNT_POINT" ]]; then
    hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMPDIR"
}
trap cleanup EXIT

RELEASE_JSON=$(curl -fsSL "$API_URL") || fatal "Failed to query GitHub API."
JSON_FILE="$TMPDIR/release.json"
printf '%s' "$RELEASE_JSON" > "$JSON_FILE"

TAG=$(python3 - "$JSON_FILE" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as f:
    data = json.load(f)
print(data.get('tag_name', ''))
PY
)
TAG=${TAG//$'\n'/}
[[ -n "$TAG" ]] || fatal "Unable to determine latest release tag from GitHub."
VERSION="${TAG#v}"

select_asset() {
  local pattern="$1"
  python3 - "$JSON_FILE" "$pattern" <<'PY'
import json, re, sys
with open(sys.argv[1], 'r', encoding='utf-8') as f:
    data = json.load(f)
pattern = sys.argv[2]
for asset in data.get('assets', []):
    name = asset.get('name', '')
    if re.search(pattern, name):
        print(name)
        sys.exit(0)
sys.exit(1)
PY
}

case "$OS" in
  Darwin)
    require hdiutil
    require sudo
    ASSET=$(select_asset '_universal\.dmg$') || fatal "Unable to locate macOS DMG asset."
    URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
    DEST="/Applications/Marlin.app"
    DMG="$TMPDIR/$ASSET"
    echo "Downloading ${ASSET}..."
    curl -fL "$URL" -o "$DMG" || fatal "Failed to download ${ASSET}."
    MOUNT_POINT="$TMPDIR/mount"
    mkdir "$MOUNT_POINT"
    echo "Mounting DMG..."
    hdiutil attach "$DMG" -nobrowse -quiet -mountpoint "$MOUNT_POINT" || fatal "Failed to mount DMG."
    echo "Installing to $DEST (sudo may prompt)..."
    sudo rm -rf "$DEST"
    sudo cp -R "$MOUNT_POINT/Marlin.app" "$DEST"
    hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true
    MOUNT_POINT=""
    echo "Marlin installed to $DEST."
    ;;
  Linux)
    case "$ARCH" in
      x86_64|amd64)
        ASSET=$(select_asset '_amd64\.AppImage$')
        ;;
      aarch64|arm64)
        ASSET=$(select_asset '_aarch64\.AppImage$') || ASSET=$(select_asset '_arm64\.AppImage$')
        ;;
      armv7l|armhf)
        fatal "32-bit ARM is not supported yet."
        ;;
      *)
        fatal "Unsupported CPU architecture '$ARCH'."
        ;;
    esac
    [[ -n "$ASSET" ]] || fatal "Unable to locate Linux AppImage asset for $ARCH."
    URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
    DEST="/usr/local/bin/marlin"
    echo "Downloading ${ASSET}..."
    curl -fL "$URL" -o "$TMPDIR/$ASSET" || fatal "Failed to download ${ASSET}."
    chmod +x "$TMPDIR/$ASSET"
    DEST_DIR="$(dirname "$DEST")"
    if [[ ! -w "$DEST_DIR" ]]; then
      if command -v sudo >/dev/null 2>&1; then
        USE_SUDO=1
        echo "Installing to $DEST (sudo may prompt)..."
        sudo mkdir -p "$DEST_DIR"
        sudo rm -f "$DEST"
        sudo mv "$TMPDIR/$ASSET" "$DEST"
        sudo chmod +x "$DEST"
      else
        DEST="$HOME/.local/bin/marlin"
        DEST_DIR="$(dirname "$DEST")"
        echo "Installing to $DEST (no sudo available)..."
        mkdir -p "$DEST_DIR"
        rm -f "$DEST"
        mv "$TMPDIR/$ASSET" "$DEST"
        chmod +x "$DEST"
        echo "Ensure $DEST_DIR is in your PATH to launch Marlin."
      fi
    else
      echo "Installing to $DEST..."
      mkdir -p "$DEST_DIR"
      rm -f "$DEST"
      mv "$TMPDIR/$ASSET" "$DEST"
      chmod +x "$DEST"
    fi
    echo "Marlin AppImage installed. Launch with 'marlin'."
    ;;
  *)
    fatal "Unsupported operating system '$OS'."
    ;;
esac

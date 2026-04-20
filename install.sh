#!/usr/bin/env bash
set -euo pipefail

REPO="BigtoC/scorpio-analyst"
INSTALL_DIR="$HOME/.local/bin"
TMP=$(mktemp -d)

cleanup() {
  rm -rf "$TMP"
}
trap cleanup EXIT

CURL_OPTS=(
  --fail
  --silent
  --show-error
  --location
  --connect-timeout 10
  --max-time 60
  --retry 3
  --retry-delay 2
  --retry-all-errors
)

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required tool: $1" >&2
    exit 1
  fi
}

unsupported_platform() {
  echo "Unsupported OS or architecture: $(uname -s)/$(uname -m)" >&2
  exit 1
}

latest_asset_missing() {
  echo "Latest release does not include ${TARGET} yet." >&2
  exit 1
}

for tool in curl tar sed; do
  require_tool "$tool"
done

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *) unsupported_platform ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64) TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *) unsupported_platform ;;
    esac
    ;;
  *)
    unsupported_platform
    ;;
esac

VERSION=$(curl "${CURL_OPTS[@]}" "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
if [ -z "$VERSION" ]; then
  echo "Failed to resolve latest release tag." >&2
  exit 1
fi

ARCHIVE="scorpio-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
ARCHIVE_URL="$BASE_URL/$ARCHIVE"

PROBE_OPTS=()
for opt in "${CURL_OPTS[@]}"; do
  if [ "$opt" != "--fail" ]; then
    PROBE_OPTS+=("$opt")
  fi
done

probe_status=$(curl "${PROBE_OPTS[@]}" --head --write-out '%{http_code}' --output /dev/null "$ARCHIVE_URL") || probe_status="curl_error"

if [ "$probe_status" = "404" ]; then
  latest_asset_missing
fi
if [ "$probe_status" != "200" ]; then
  echo "Failed to access release archive: $ARCHIVE_URL" >&2
  exit 1
fi

echo "Installing scorpio ${VERSION} for ${TARGET}..."
echo "Downloading $ARCHIVE_URL..."

curl "${CURL_OPTS[@]}" "$ARCHIVE_URL" -o "$TMP/$ARCHIVE"

tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
if [ ! -f "$TMP/scorpio" ]; then
  echo "Expected scorpio binary missing from archive." >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
mv "$TMP/scorpio" "$INSTALL_DIR/scorpio"
chmod +x "$INSTALL_DIR/scorpio"

if [ ! -x "$INSTALL_DIR/scorpio" ]; then
  echo "Installed binary is not executable: $INSTALL_DIR/scorpio" >&2
  exit 1
fi

if ! "$INSTALL_DIR/scorpio" -h >/dev/null 2>&1; then
  echo "Installed binary failed to run: $INSTALL_DIR/scorpio" >&2
  exit 1
fi

echo "Installed: $HOME/.local/bin/scorpio"
echo "Run 'scorpio -h' to get started."
echo "Run 'which scorpio' to confirm it is on your PATH."

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo
    echo "NOTE: $INSTALL_DIR is not in your PATH."
    echo "Add the following line to your shell profile:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac

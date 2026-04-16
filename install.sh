#!/usr/bin/env bash
set -euo pipefail

REPO="BigtoC/scorpio-analyst"
BINARY_NAME="scorpio"
INSTALL_DIR="$HOME/.local/bin"

# --- Detect platform ---
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)
        echo "Unsupported Linux architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64)  TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *)
        echo "Unsupported macOS architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS" >&2
    echo "For Windows, run:" >&2
    echo "  curl.exe -fsSL https://raw.githubusercontent.com/$REPO/main/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -" >&2
    exit 1
    ;;
esac

# --- Fetch latest release tag ---
API_RESPONSE=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")
if echo "$API_RESPONSE" | grep -q '"message"'; then
  MSG=$(echo "$API_RESPONSE" | grep '"message"' | sed 's/.*"message": *"\([^"]*\)".*/\1/')
  echo "GitHub API error: $MSG" >&2
  exit 1
fi
VERSION=$(echo "$API_RESPONSE" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$VERSION" ]; then
  echo "Failed to determine latest release version." >&2
  exit 1
fi

echo "Installing $BINARY_NAME $VERSION for $TARGET..."

# --- Download ---
ARCHIVE="scorpio-analyst-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $URL..."
curl -fsSL "$URL" -o "$TMP/$ARCHIVE"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"

# --- Install ---
if [ ! -f "$TMP/scorpio-analyst" ]; then
  echo "Extraction failed: scorpio-analyst not found in archive." >&2
  exit 1
fi
mkdir -p "$INSTALL_DIR"
mv "$TMP/scorpio-analyst" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

echo ""
echo "Installed: $INSTALL_DIR/$BINARY_NAME"
echo "Version:   $VERSION"

# --- PATH hint ---
case ":${PATH}:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo ""
    echo "NOTE: $INSTALL_DIR is not in your PATH."
    echo "Add the following line to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac

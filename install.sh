#!/bin/sh
set -e

REPO="hadefication/bunker"
INSTALL_DIR="/usr/local/bin"
BINARY="bunker"

main() {
    # Check macOS
    if [ "$(uname -s)" != "Darwin" ]; then
        echo "Error: bunker only supports macOS." >&2
        exit 1
    fi

    # Detect architecture
    ARCH="$(uname -m)"
    case "$ARCH" in
        arm64)  TARGET="aarch64-apple-darwin" ;;
        x86_64) TARGET="x86_64-apple-darwin" ;;
        *)
            echo "Error: unsupported architecture: $ARCH" >&2
            exit 1
            ;;
    esac

    # Get latest version
    echo "==> Fetching latest release..."
    RELEASE_JSON=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")
    VERSION=$(echo "$RELEASE_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')

    if [ -z "$VERSION" ]; then
        echo "Error: could not determine latest version." >&2
        exit 1
    fi

    ASSET="bunker-${TARGET}"
    URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"

    echo "==> Downloading bunker ${VERSION} (${TARGET})..."
    TMP_DIR=$(mktemp -d)
    TMP_BIN="${TMP_DIR}/${BINARY}"

    HTTP_CODE=$(curl -fSL -o "$TMP_BIN" -w "%{http_code}" "$URL" 2>/dev/null) || true
    if [ ! -f "$TMP_BIN" ] || [ -z "$HTTP_CODE" ] || [ "$HTTP_CODE" -lt 200 ] || [ "$HTTP_CODE" -ge 300 ]; then
        rm -rf "$TMP_DIR"
        echo "Error: download failed (HTTP ${HTTP_CODE:-unknown}) from ${URL}" >&2
        echo "Check https://github.com/${REPO}/releases for available assets." >&2
        exit 1
    fi

    chmod +x "$TMP_BIN"

    # Verify it runs
    if ! "$TMP_BIN" --version >/dev/null 2>&1; then
        rm -rf "$TMP_DIR"
        echo "Error: downloaded binary failed verification." >&2
        exit 1
    fi

    # Install
    echo "==> Installing to ${INSTALL_DIR}/${BINARY}..."
    if [ -w "$INSTALL_DIR" ]; then
        mv "$TMP_BIN" "${INSTALL_DIR}/${BINARY}"
    else
        sudo mv "$TMP_BIN" "${INSTALL_DIR}/${BINARY}"
    fi

    rm -rf "$TMP_DIR"

    echo "==> bunker ${VERSION} installed to ${INSTALL_DIR}/${BINARY}"
    echo ""
    echo "Run 'bunker help' to get started."
}

main

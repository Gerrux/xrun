#!/usr/bin/env sh
# xrun installer — https://github.com/gerrux/xrun
#
# Usage:
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh -s -- --version v0.4.0
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh -s -- --prefix ~/.local

set -eu

REPO="gerrux/xrun"
BINARY="xrun"
DEFAULT_PREFIX="${HOME}/.local"

# ── parse flags ──────────────────────────────────────────────────────────────
VERSION=""
PREFIX="$DEFAULT_PREFIX"

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --prefix)  PREFIX="$2";  shift 2 ;;
        --help|-h)
            echo "Usage: install.sh [--version v0.4.0] [--prefix ~/.local]"
            exit 0 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

# ── detect platform ───────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)
        case "$ARCH" in
            x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
            aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
            *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
        esac
        ;;
    Darwin)
        case "$ARCH" in
            x86_64)  TARGET="x86_64-apple-darwin" ;;
            arm64)   TARGET="aarch64-apple-darwin" ;;
            *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
        esac
        ;;
    *)
        echo "Unsupported OS: $OS"
        echo "For Windows, use: irm https://raw.githubusercontent.com/gerrux/xrun/main/install.ps1 | iex"
        exit 1 ;;
esac

# ── resolve version ───────────────────────────────────────────────────────────
if [ -z "$VERSION" ]; then
    if command -v curl > /dev/null 2>&1; then
        VERSION="$(curl -sSf "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    elif command -v wget > /dev/null 2>&1; then
        VERSION="$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    else
        echo "curl or wget is required"
        exit 1
    fi
fi

if [ -z "$VERSION" ]; then
    echo "Could not determine latest version. Pass --version v0.4.0 explicitly."
    exit 1
fi

# ── download & install ────────────────────────────────────────────────────────
ARCHIVE="${BINARY}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"
INSTALL_DIR="${PREFIX}/bin"

echo "Installing xrun ${VERSION} (${TARGET}) → ${INSTALL_DIR}/${BINARY}"

mkdir -p "$INSTALL_DIR"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if command -v curl > /dev/null 2>&1; then
    curl -sSfL "$URL" -o "${TMP}/${ARCHIVE}"
elif command -v wget > /dev/null 2>&1; then
    wget -qO "${TMP}/${ARCHIVE}" "$URL"
fi

tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP"
install -m 755 "${TMP}/${BINARY}" "${INSTALL_DIR}/${BINARY}"

echo "xrun ${VERSION} installed to ${INSTALL_DIR}/${BINARY}"

# ── PATH hint ─────────────────────────────────────────────────────────────────
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Add ${INSTALL_DIR} to your PATH:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

echo ""
echo "Run 'xrun doctor' to verify your setup."

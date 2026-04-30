#!/usr/bin/env sh
# xrun installer — https://github.com/gerrux/xrun
#
# Usage:
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh -s -- --version v0.4.0
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh -s -- --with-skill
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/main/install.sh | sh -s -- --skill-only

set -eu

REPO="gerrux/xrun"
BINARY="xrun"
DEFAULT_PREFIX="${HOME}/.local"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/main"

# ── parse flags ──────────────────────────────────────────────────────────────
VERSION=""
PREFIX="$DEFAULT_PREFIX"
WITH_SKILL=0
SKILL_ONLY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --version)    VERSION="$2"; shift 2 ;;
        --prefix)     PREFIX="$2";  shift 2 ;;
        --with-skill) WITH_SKILL=1; shift ;;
        --skill-only) SKILL_ONLY=1; shift ;;
        --help|-h)
            echo "Usage: install.sh [--version v0.4.0] [--prefix ~/.local] [--with-skill] [--skill-only]"
            exit 0 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

# ── helpers ───────────────────────────────────────────────────────────────────
download() {
    if command -v curl > /dev/null 2>&1; then
        curl -sSfL "$1" -o "$2"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        echo "curl or wget is required"; exit 1
    fi
}

fetch_text() {
    if command -v curl > /dev/null 2>&1; then
        curl -sSf "$1"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO- "$1"
    else
        echo "curl or wget is required"; exit 1
    fi
}

# ── install Claude Code skill ─────────────────────────────────────────────────
install_skill() {
    SKILL_DIR="${HOME}/.claude/skills/xrun"
    mkdir -p "$SKILL_DIR"
    download "${RAW_BASE}/claude/skill.md" "${SKILL_DIR}/SKILL.md"
    echo "Claude Code skill installed → ${SKILL_DIR}/SKILL.md"
}

if [ "$SKILL_ONLY" = "1" ]; then
    install_skill
    exit 0
fi

# ── detect platform ───────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)
        case "$ARCH" in
            x86_64)        TARGET="x86_64-unknown-linux-musl" ;;
            aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
            *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
        esac
        ;;
    Darwin)
        case "$ARCH" in
            x86_64) TARGET="x86_64-apple-darwin" ;;
            arm64)  TARGET="aarch64-apple-darwin" ;;
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
    VERSION="$(fetch_text "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
fi

if [ -z "$VERSION" ]; then
    echo "Could not determine latest version. Pass --version v0.4.0 explicitly."
    exit 1
fi

# ── download & install binary ─────────────────────────────────────────────────
ARCHIVE="${BINARY}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"
INSTALL_DIR="${PREFIX}/bin"

echo "Installing xrun ${VERSION} (${TARGET}) → ${INSTALL_DIR}/${BINARY}"

mkdir -p "$INSTALL_DIR"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download "$URL" "${TMP}/${ARCHIVE}"
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

# ── optional: Claude Code skill ───────────────────────────────────────────────
if [ "$WITH_SKILL" = "1" ]; then
    echo ""
    install_skill
fi

echo ""
echo "Run 'xrun doctor' to verify your setup."

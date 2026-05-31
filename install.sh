#!/usr/bin/env sh
# xrun installer - https://github.com/gerrux/xrun
#
# Usage:
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --version v0.7.1
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --no-tui
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --install-pip
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --with-skill
#   curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --skill-only

set -eu

REPO="gerrux/xrun"
BINARY="xrun"
DEFAULT_PREFIX="${HOME}/.local"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/master"

VERSION=""
PREFIX="$DEFAULT_PREFIX"
WITH_SKILL=0
SKILL_ONLY=0
WITH_TUI=1
TUI_ONLY=0
INSTALL_PIP=0

while [ $# -gt 0 ]; do
    case "$1" in
        --version)     VERSION="$2"; shift 2 ;;
        --prefix)      PREFIX="$2"; shift 2 ;;
        --with-skill)  WITH_SKILL=1; shift ;;
        --skill-only)  SKILL_ONLY=1; shift ;;
        --with-tui)    WITH_TUI=1; shift ;;
        --no-tui)      WITH_TUI=0; shift ;;
        --tui-only)    TUI_ONLY=1; WITH_TUI=1; shift ;;
        --install-pip) INSTALL_PIP=1; shift ;;
        --help|-h)
            echo "Usage: install.sh [--version v0.7.1] [--prefix ~/.local] [--with-tui|--no-tui] [--install-pip] [--with-skill] [--skill-only|--tui-only]"
            exit 0 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

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

install_skill() {
    SKILL_DIR="${HOME}/.claude/skills/xrun"
    mkdir -p "$SKILL_DIR"
    download "${RAW_BASE}/claude/skill.md" "${SKILL_DIR}/SKILL.md"
    echo "Claude Code skill installed -> ${SKILL_DIR}/SKILL.md"
}

find_python() {
    for candidate in python3.11 python3 python; do
        if command -v "$candidate" > /dev/null 2>&1; then
            if "$candidate" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)' > /dev/null 2>&1; then
                echo "$candidate"
                return 0
            fi
        fi
    done
    return 1
}

ensure_pip() {
    PY="$1"
    if "$PY" -m pip --version > /dev/null 2>&1; then
        return 0
    fi

    if [ "$INSTALL_PIP" = "1" ]; then
        echo "pip not found for $PY; trying ensurepip..."
        "$PY" -m ensurepip --upgrade
        "$PY" -m pip --version > /dev/null 2>&1 || {
            echo "ensurepip finished, but pip is still not available for $PY."
            echo "Install pip with your OS package manager, then re-run this installer."
            return 1
        }
        return 0
    fi

    echo "pip not found for $PY."
    echo "Re-run with --install-pip to try: $PY -m ensurepip --upgrade"
    echo "Or install pip with your OS package manager, then re-run this installer."
    return 1
}

install_tui() {
    PY="$(find_python)" || {
        echo "Python >= 3.11 is required for xrun-tui."
        echo "Install Python 3.11+ and re-run, or pass --no-tui for CLI-only install."
        return 1
    }
    ensure_pip "$PY"

    TUI_REF="${VERSION:-master}"
    TUI_URL="git+https://github.com/${REPO}.git@${TUI_REF}#subdirectory=python/xrun_tui"
    echo "Installing xrun-tui from ${TUI_REF}..."
    "$PY" -m pip install --user "$TUI_URL"
    echo "xrun-tui installed"
}

resolve_version() {
    if [ -z "$VERSION" ]; then
        VERSION="$(fetch_text "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    fi

    if [ -z "$VERSION" ]; then
        echo "Could not determine latest version. Pass --version v0.7.1 explicitly."
        exit 1
    fi
}

if [ "$SKILL_ONLY" = "1" ]; then
    install_skill
    exit 0
fi

resolve_version

if [ "$TUI_ONLY" = "1" ]; then
    install_tui
    echo ""
    echo "Run 'xrun-tui' to start the TUI."
    exit 0
fi

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
        echo "For Windows, use: irm https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1 | iex"
        exit 1 ;;
esac

ARCHIVE="${BINARY}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"
INSTALL_DIR="${PREFIX}/bin"

echo "Installing xrun ${VERSION} (${TARGET}) -> ${INSTALL_DIR}/${BINARY}"

mkdir -p "$INSTALL_DIR"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

download "$URL" "${TMP}/${ARCHIVE}"
tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP"
install -m 755 "${TMP}/${BINARY}" "${INSTALL_DIR}/${BINARY}"

echo "xrun ${VERSION} installed to ${INSTALL_DIR}/${BINARY}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Add ${INSTALL_DIR} to your PATH:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

if [ "$WITH_SKILL" = "1" ]; then
    echo ""
    install_skill
fi

if [ "$WITH_TUI" = "1" ]; then
    echo ""
    install_tui
fi

echo ""
echo "Run 'xrun doctor' to verify your setup."

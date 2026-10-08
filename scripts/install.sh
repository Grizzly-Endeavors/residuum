#!/bin/sh
# Residuum installer
# Usage: curl -fsSL https://github.com/grizzly-endeavors/residuum/releases/latest/download/install.sh | sh
set -eu

REPO="grizzly-endeavors/residuum"
BINARY_NAME="residuum"
INSTALL_DIR="${RESIDUUM_INSTALL_DIR:-/usr/local/bin}"

# --- helpers ----------------------------------------------------------------

say() {
  printf '  %s\n' "$*"
}

err() {
  printf '\033[31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" > /dev/null 2>&1 || err "required command not found: $1"
}

# --- detect platform --------------------------------------------------------

detect_platform() {
  OS="$(uname -s)"
  ARCH="$(uname -m)"

  case "$OS" in
    Linux)  OS="linux" ;;
    Darwin) OS="macos" ;;
    *)      err "unsupported OS: $OS" ;;
  esac

  case "$ARCH" in
    x86_64 | amd64)  ARCH="x86_64" ;;
    aarch64 | arm64)  ARCH="aarch64" ;;
    *)                err "unsupported architecture: $ARCH" ;;
  esac

  # macOS Intel builds are not provided — Apple Silicon only
  if [ "$OS" = "macos" ] && [ "$ARCH" = "x86_64" ]; then
    err "macOS x86_64 (Intel) is not supported — Apple Silicon only"
  fi

  PLATFORM="${OS}-${ARCH}"
}

# --- resolve latest version -------------------------------------------------

get_latest_version() {
  need curl

  VERSION="$(
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' \
      | head -1 \
      | sed 's/.*"tag_name": *"//;s/".*//'
  )" || err "failed to fetch latest release from GitHub"

  [ -n "$VERSION" ] || err "could not determine latest version"
}

# --- checksum ---------------------------------------------------------------

sha256() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum > /dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    err "required command not found: sha256sum or shasum"
  fi
}

verify_checksum() {
  SUMS_URL="https://github.com/${REPO}/releases/download/${VERSION}/SHA256SUMS"
  curl -fsSL -o "${TMP}/SHA256SUMS" "$SUMS_URL" \
    || err "failed to download SHA256SUMS for ${VERSION}"

  EXPECTED="$(awk -v asset="$ASSET" '$2 == asset { print $1 }' "${TMP}/SHA256SUMS")"
  [ -n "$EXPECTED" ] || err "no checksum listed for ${ASSET} in SHA256SUMS"

  ACTUAL="$(sha256 "${TMP}/${BINARY_NAME}")"
  [ "$ACTUAL" = "$EXPECTED" ] \
    || err "checksum mismatch for ${ASSET} (expected ${EXPECTED}, got ${ACTUAL})"

  say "checksum verified"
}

# --- download and install ---------------------------------------------------

install() {
  ASSET="${BINARY_NAME}-${PLATFORM}"
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT

  say "downloading ${BINARY_NAME} ${VERSION} for ${PLATFORM}..."
  curl -fsSL -o "${TMP}/${BINARY_NAME}" "$URL" \
    || err "download failed — asset may not exist for your platform yet"

  verify_checksum

  chmod +x "${TMP}/${BINARY_NAME}"

  # install to target directory; fresh macOS has no /usr/local/bin
  if [ ! -d "$INSTALL_DIR" ]; then
    mkdir -p "$INSTALL_DIR" 2> /dev/null || sudo mkdir -p "$INSTALL_DIR"
  fi

  if [ -w "$INSTALL_DIR" ]; then
    mv "${TMP}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
  else
    say "installing to ${INSTALL_DIR} (requires sudo)..."
    sudo mv "${TMP}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
  fi
}

# --- verify -----------------------------------------------------------------

verify() {
  if command -v "$BINARY_NAME" > /dev/null 2>&1; then
    INSTALLED_PATH="$(command -v "$BINARY_NAME")"
    say ""
    say "installed: ${INSTALLED_PATH}"
    say "run 'residuum serve' to start, or 'residuum init' to configure."
  else
    say ""
    say "installed to ${INSTALL_DIR}/${BINARY_NAME}"
    say "make sure ${INSTALL_DIR} is in your PATH."
  fi
}

# --- main -------------------------------------------------------------------

main() {
  printf '\n\033[1m  Residuum Installer\033[0m\n'
  say "────────────────────"
  say ""

  detect_platform
  get_latest_version
  install
  verify

  say ""
}

main

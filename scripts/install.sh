#!/bin/sh
# Residuum installer
# Usage: curl -fsSL https://github.com/grizzly-endeavors/residuum/releases/latest/download/install.sh | sh
set -eu

REPO="grizzly-endeavors/residuum"
BINARY_NAME="residuum"
# Residuum updates itself in place, so it installs where this account can
# write without sudo: RESIDUUM_INSTALL_DIR if set, else the directory of an
# existing writable install, else /usr/local/bin when writable, else
# ~/.local/bin.
USER_BIN_DIR="${HOME}/.local/bin"
PATH_MARKER="# added by the Residuum installer"

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

# --- choose install directory -----------------------------------------------

choose_install_dir() {
  EXISTING="$(command -v "$BINARY_NAME" 2> /dev/null || true)"

  if [ -n "${RESIDUUM_INSTALL_DIR:-}" ]; then
    INSTALL_DIR="$RESIDUUM_INSTALL_DIR"
    mkdir -p "$INSTALL_DIR" 2> /dev/null \
      || err "can't create ${INSTALL_DIR} — choose a directory this account can write to"
    [ -w "$INSTALL_DIR" ] \
      || err "${INSTALL_DIR} isn't writable by this account, so residuum couldn't update itself there — choose another RESIDUUM_INSTALL_DIR"
    return
  fi

  if [ -n "$EXISTING" ] && [ -w "$(dirname "$EXISTING")" ]; then
    INSTALL_DIR="$(dirname "$EXISTING")"
  elif [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
    INSTALL_DIR="/usr/local/bin"
  else
    INSTALL_DIR="$USER_BIN_DIR"
    mkdir -p "$INSTALL_DIR" || err "can't create ${INSTALL_DIR}"
  fi
}

# --- PATH -------------------------------------------------------------------

on_path() {
  case ":${PATH}:" in
    *":$1:"*) return 0 ;;
    *)        return 1 ;;
  esac
}

# Prepend INSTALL_DIR to PATH in the login shell's startup file, once.
add_to_path() {
  on_path "$INSTALL_DIR" && return 0

  SHELL_NAME="$(basename "${SHELL:-sh}")"
  case "$SHELL_NAME" in
    zsh)
      RC_FILE="${ZDOTDIR:-$HOME}/.zshrc"
      LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
    bash)
      if [ "$OS" = "macos" ]; then RC_FILE="${HOME}/.bash_profile"; else RC_FILE="${HOME}/.bashrc"; fi
      LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
    fish)
      RC_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/residuum.fish"
      LINE="fish_add_path -g ${INSTALL_DIR}"
      mkdir -p "$(dirname "$RC_FILE")"
      ;;
    *)
      RC_FILE="${HOME}/.profile"
      LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
  esac

  if [ -f "$RC_FILE" ] && grep -qF "$PATH_MARKER" "$RC_FILE"; then
    PATH_NOTE="open a new terminal so ${INSTALL_DIR} is on your PATH."
    return 0
  fi

  printf '\n%s\n%s\n' "$PATH_MARKER" "$LINE" >> "$RC_FILE"
  PATH_NOTE="added ${INSTALL_DIR} to your PATH in ${RC_FILE} — open a new terminal to use it."
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

  mv "${TMP}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
}

# --- verify -----------------------------------------------------------------

verify() {
  INSTALLED="${INSTALL_DIR}/${BINARY_NAME}"
  say ""
  say "installed: ${INSTALLED}"

  if [ -n "${PATH_NOTE:-}" ]; then
    say "$PATH_NOTE"
  fi

  # An older copy somewhere this account can't write can shadow the new
  # one, and can never update itself.
  if [ -n "$EXISTING" ] && [ "$EXISTING" != "$INSTALLED" ]; then
    say ""
    say "an older residuum is also installed at ${EXISTING}."
    say "remove it so only the new one is used: sudo rm ${EXISTING}"
  fi

  say ""
  if [ -n "$EXISTING" ]; then
    say "if a gateway is running, stop it and start it again with 'residuum serve'."
  else
    say "run 'residuum serve' to start, or 'residuum init' to configure."
  fi
}

# --- main -------------------------------------------------------------------

main() {
  printf '\n\033[1m  Residuum Installer\033[0m\n'
  say "────────────────────"
  say ""

  detect_platform
  get_latest_version
  choose_install_dir
  install
  add_to_path
  verify

  say ""
}

main

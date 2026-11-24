#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$HOME/Applications/Residuum.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"

echo "Building release binary..."
cargo build --release --manifest-path "$PROJECT_ROOT/Cargo.toml"

echo "Creating app bundle at $APP_DIR..."
mkdir -p "$MACOS_DIR"
cp "$PROJECT_ROOT/bundle/Info.plist" "$CONTENTS/Info.plist"
cp "$CARGO_TARGET_DIR/release/residuum" "$MACOS_DIR/residuum"

echo "Signing app bundle..."
codesign -s - -f --deep "$APP_DIR"

/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP_DIR"

cp "$CARGO_TARGET_DIR/release/residuum" "$HOME/.cargo/bin/residuum"

echo "Done. Residuum.app installed at $APP_DIR"
echo ""
echo "Start with:  open $APP_DIR --args serve"
echo ""
echo "Grant notification permissions at:"
echo "  System Settings > Notifications > Residuum > Allow Notifications"

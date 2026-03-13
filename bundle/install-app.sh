#!/bin/bash
# Build and install Residuum.app bundle for macOS notifications.
#
# UNUserNotificationCenter requires a process with a CFBundleIdentifier.
# This script creates a minimal .app wrapper around the release binary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$HOME/Applications/Residuum.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"

echo "Building release binary..."
cargo build --release --manifest-path "$PROJECT_ROOT/Cargo.toml"

echo "Creating app bundle at $APP_DIR..."
mkdir -p "$MACOS_DIR"
cp "$PROJECT_ROOT/bundle/Info.plist" "$CONTENTS/Info.plist"
cp "$PROJECT_ROOT/target/release/residuum" "$MACOS_DIR/residuum"

# Ad-hoc sign so macOS recognizes the bundle for notifications
echo "Signing app bundle..."
codesign -s - -f --deep "$APP_DIR"

# Register with LaunchServices so it appears in Notification Settings
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP_DIR"

# Also update the cargo bin copy for non-notification use
cp "$PROJECT_ROOT/target/release/residuum" "$HOME/.cargo/bin/residuum"

echo "Done. Residuum.app installed at $APP_DIR"
echo ""
echo "Start with:  open $APP_DIR --args serve"
echo ""
echo "Grant notification permissions at:"
echo "  System Settings > Notifications > Residuum > Allow Notifications"

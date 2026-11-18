#!/usr/bin/env bash
# Build rebro (release) and install it on macOS (or Linux), at a stable path so git's credential-helper
# entry never moves.
#   ./install.sh            CLI -> ~/.local/bin/rebro, app -> ~/Applications/rebro.app (macOS)
#   ./install.sh --no-gui
# Needs: rustup (https://rustup.rs) and, on macOS, the Xcode command line tools (xcode-select --install).
set -euo pipefail
cd "$(dirname "$0")"
NO_GUI=${1:-}
command -v cargo >/dev/null || { [ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH"; }
command -v cargo >/dev/null || { echo "cargo missing: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2; exit 1; }

cargo build --release -p rebro
BIN="$HOME/.local/bin"
mkdir -p "$BIN"
install -m 0755 target/release/rebro "$BIN/rebro"
echo "cli: $BIN/rebro"
case ":$PATH:" in *":$BIN:"*) ;; *) echo "add to your shell profile: export PATH=\"$BIN:\$PATH\"";; esac

if [ "$NO_GUI" != --no-gui ]; then
  # GPUI on macOS draws with Metal; gpui-kit's runtime shaders need no full Xcode
  cargo build --release -p rebro-gui
  if [ "$(uname)" = Darwin ]; then
    APP="$HOME/Applications/rebro.app"
    pkill -x rebro-gui 2>/dev/null || true
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    install -m 0755 target/release/rebro-gui "$APP/Contents/MacOS/rebro-gui"
    install -m 0755 target/release/rebro "$APP/Contents/MacOS/rebro"   # the GUI wires git to its sibling CLI
    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>rebro</string>
  <key>CFBundleDisplayName</key><string>rebro</string>
  <key>CFBundleIdentifier</key><string>dev.rebro.app</string>
  <key>CFBundleExecutable</key><string>rebro-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
    echo "app: $APP (Spotlight: rebro)"
  else
    install -m 0755 target/release/rebro-gui "$BIN/rebro-gui"
    echo "gui: $BIN/rebro-gui"
  fi
fi

# point git/gh at the installed CLI (no-op without accounts)
"$BIN/rebro" sync
echo "installed"

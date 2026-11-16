#!/bin/sh
# Package a release build as Eshvi.app (unsigned).
#   scripts/bundle-macos.sh [path/to/eshvi] [out dir]
# svtrace and slang (scripts/build-slang.sh) go beside eshvi when both are found:
# SVTRACE and SLANG in the environment, else svtrace beside the eshvi binary and slang
# in ~/.cache/slang-v12.1/bin. Without them the bundle is as it was, with a note.
set -eu
bin=${1:-target/release/eshvi}
out=${2:-target/release}
root=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
app="$out/Eshvi.app"

bindir=$(dirname "$bin")
svtrace=${SVTRACE:-$bindir/svtrace}
slang=${SLANG:-${XDG_CACHE_HOME:-$HOME/.cache}/slang-v12.1/bin/slang}
with_trace=0
if [ -x "$svtrace" ] && [ -x "$slang" ]; then
  [ -f "$(dirname "$slang")/slang-LICENSES.txt" ] ||
    { echo "bundle: $(dirname "$slang")/slang-LICENSES.txt is missing; run scripts/build-slang.sh" >&2; exit 1; }
  with_trace=1
else
  echo "bundle: svtrace ($svtrace) or slang ($slang) not found; the signal trace is left out" >&2
fi

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/eshvi"
if [ "$with_trace" = 1 ]; then
  cp "$svtrace" "$app/Contents/MacOS/svtrace"
  cp "$slang" "$app/Contents/MacOS/slang"
fi
cp "$root/assets/icon/eshvi.icns" "$app/Contents/Resources/eshvi.icns"
# The icons', the grammar's and the Rust crates' licenses ask for their credit and text
# to go with every copy.
{
  cat "$root/assets/THIRD-PARTY-NOTICES.txt"
  printf '\n\n===== Rust crates =====\n\n'
  cat "$root/assets/rust-crates.LICENSES.txt"
  printf '\n\n===== Material Icon Theme =====\n\n'
  cat "$root/assets/material-icons.LICENSE"
  printf '\n\n===== Codicons =====\n\n'
  cat "$root/assets/codicons/LICENSE"
  printf '\n\n===== SystemVerilog grammar =====\n\n'
  cat "$root/assets/syntaxes/SystemVerilog.LICENSE"
  if [ "$with_trace" = 1 ]; then
    printf '\n\n===== slang, and what it links statically =====\n\n'
    cat "$(dirname "$slang")/slang-LICENSES.txt"
  fi
} > "$app/Contents/Resources/THIRD-PARTY-NOTICES.txt"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Eshvi</string>
  <key>CFBundleDisplayName</key><string>Eshvi</string>
  <key>CFBundleIdentifier</key><string>io.github.aviyn.eshvi</string>
  <key>CFBundleExecutable</key><string>eshvi</string>
  <key>CFBundleIconFile</key><string>eshvi</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Seal the bundle with an ad-hoc signature. Unsealed, macOS calls a
# downloaded copy "damaged"; sealed, it asks once to allow an unknown
# developer (or clear quarantine: xattr -dr com.apple.quarantine Eshvi.app).
codesign --force --deep --sign - "$app"
echo "$app"

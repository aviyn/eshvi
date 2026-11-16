#!/bin/sh
# Make assets/rust-crates.LICENSES.txt: the Rust crates built into eshvi and svtrace,
# each with its license, which scripts/bundle-macos.sh puts in the app's
# THIRD-PARTY-NOTICES.txt. Run it after Cargo.lock changes; a test
# (crates/app/tests/notices.rs) fails until you do.
# Needs cargo-about: cargo install cargo-about --locked --features cli
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
out=assets/rust-crates.LICENSES.txt
# FNV-1a (64-bit) of Cargo.lock, the same as the test computes.
lock=$(python3 -c '
import sys
h = 0xcbf29ce484222325
for b in open(sys.argv[1], "rb").read():
    h = ((h ^ b) * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
print("%016x" % h)' Cargo.lock)
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
cargo about generate --locked -c scripts/notices/about.toml scripts/notices/about.hbs -o "$tmp"
{
  echo "Made from Cargo.lock $lock by scripts/rust-notices.sh."
  echo
  cat "$tmp"
  # nvim-rs is under the LGPL 3.0, which is a set of permissions added to the GPL 3.0:
  # its text goes with it.
  printf '\n\n===== GNU General Public License v3.0 =====\n\n'
  echo "The GNU Lesser General Public License v3.0 above (nvim-rs) adds its permissions to this license."
  echo
  cat scripts/notices/GPL-3.0.txt
} > "$out"
echo "wrote $out"

#!/bin/sh
# Build the slang driver (v12.1) that svtrace runs, from source, with one patch of ours:
# --ast-json-compact (scripts/slang-ast-json-compact.patch), so the AST dump comes without
# indentation, a quarter of the size.
#   scripts/build-slang.sh [OUT]
# OUT defaults to ~/.cache/slang-v12.1/bin/slang. Next to it goes slang-LICENSES.txt,
# the licenses of slang and of what it links statically (bundle-macos.sh ships them).
# Environment: SLANG_SRC (default ~/.cache/slang-v12.1-src, cloned from tag v12.1 if
# missing), SLANG_BUILD (default ~/.cache/slang-v12.1, the CMake build folder).
set -eu
# The commit that tag v12.1 names. A tag can be moved; what is built and shipped is this commit.
commit=42710c788ce76cd08ce2080773e0b8286c428a05
cache=${XDG_CACHE_HOME:-$HOME/.cache}
out=${1:-$cache/slang-v12.1/bin/slang}
src=${SLANG_SRC:-$cache/slang-v12.1-src}
build=${SLANG_BUILD:-$cache/slang-v12.1}

if [ ! -f "$src/CMakeLists.txt" ]; then
  git clone --depth 1 --branch v12.1 https://github.com/MikePopoloski/slang.git "$src"
fi
# (A source folder that is not a git checkout, such as an unpacked archive, can't be checked here: the
# version the binary prints, which has the commit in it, is checked below.)
if [ -e "$src/.git" ]; then
  head=$(git -C "$src" rev-parse HEAD)
  [ "$head" = "$commit" ] || { echo "build-slang: $src is at $head, not v12.1's $commit" >&2; exit 1; }
fi
# The patch, unless the source already has it.
patch="$(cd "$(dirname "$0")" && pwd)/slang-ast-json-compact.patch"
if ! (cd "$src" && patch -p1 -R --dry-run --force --quiet < "$patch") >/dev/null 2>&1; then
  (cd "$src" && patch -p1 --forward --quiet < "$patch")
fi
if command -v ninja >/dev/null 2>&1; then gen=Ninja; else gen="Unix Makefiles"; fi
jobs=$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

# Same generator as an earlier configure in this folder, or CMake refuses to reuse it.
if [ -f "$build/CMakeCache.txt" ]; then gen_args=""; else gen_args=$gen; fi
if [ -n "$gen_args" ]; then
  cmake -S "$src" -B "$build" -G "$gen_args" -DCMAKE_BUILD_TYPE=Release \
    -DSLANG_INCLUDE_TESTS=OFF -DSLANG_INCLUDE_PYLIB=OFF -DSLANG_INCLUDE_TOOLS=ON >&2
else
  cmake -S "$src" -B "$build" -DCMAKE_BUILD_TYPE=Release \
    -DSLANG_INCLUDE_TESTS=OFF -DSLANG_INCLUDE_PYLIB=OFF -DSLANG_INCLUDE_TOOLS=ON >&2
fi
cmake --build "$build" --target slang_driver -j "$jobs" >&2
# The driver tool's target is named `slang_driver`, its binary `slang`.
built=$(find "$build" -type f -name slang -perm -u+x -path '*bin*' | head -1)
[ -n "$built" ] || { echo "build-slang: no slang binary in $build" >&2; exit 1; }

mkdir -p "$(dirname "$out")"
if [ "$(cd "$(dirname "$built")" && pwd)/slang" != "$(cd "$(dirname "$out")" && pwd)/$(basename "$out")" ]; then
  cp "$built" "$out"
fi
"$out" --version | grep -q 'slang version 12\.1' || { echo "build-slang: $out is not slang 12.1" >&2; exit 1; }
# (The version ends in the commit as git abbreviates it, which is 7 characters in a shallow clone and
# can be longer in a full one: any prefix of 7 or more must be the commit's.)
built_from=$("$out" --version | sed -n 's/.*+\([0-9a-f]\{7,\}\).*/\1/p')
case "$commit" in
  "$built_from"?*|"$built_from") [ -n "$built_from" ] ;;
  *) false ;;
esac || { echo "build-slang: $out was not built from $commit (it says ${built_from:-no commit})" >&2; exit 1; }
"$out" --help | grep -q -- '--ast-json-compact' || { echo "build-slang: $out lacks --ast-json-compact" >&2; exit 1; }

# Only system libraries may be linked, or it won't run on another Mac. (The list is taken first, so
# that an otool that is missing or fails stops the script instead of passing as "nothing found".)
if [ "$(uname)" = Darwin ]; then
  linked=$(otool -L "$out") || { echo "build-slang: otool could not read $out" >&2; exit 1; }
  if printf '%s\n' "$linked" | tail -n +2 | awk '{print $1}' | grep -vE '^(/usr/lib/|/System/)' ; then
    echo "build-slang: $out links something outside the system" >&2
    exit 1
  fi
fi

# Licenses of what the binary contains: slang (MIT); {fmt}, mimalloc, toml++ (MIT);
# boost::regex/unordered/concurrent and expected-lite (Boost); BS::thread_pool (MIT).
notices=$(dirname "$out")/slang-LICENSES.txt
dep() { # name, folder in the build's _deps
  printf '\n\n===== %s =====\n\n' "$1"
  cat "$build/_deps/$2/LICENSE"
}
{
  printf '===== slang (MIT) =====\n\n'
  cat "$src/LICENSE"
  dep "{fmt} (MIT)" fmt-src
  dep "mimalloc (MIT)" mimalloc-src
  dep "toml++ (MIT)" tomlplusplus-src
  printf '\n\n===== BS::thread_pool (MIT) =====\n\n'
  printf 'Copyright (c) 2021-2026 Barak Shoshany. Licensed under the MIT license;\nthe text is the MIT license of slang above.\n'
  printf '\n\n===== Boost: regex, unordered, concurrent containers (Boost Software License 1.0)'
  printf '\nand expected-lite by Martin Moene (Boost Software License 1.0) =====\n\n'
  cat "$src/LICENSES/BSL-1.0.txt"
} > "$notices"
echo "$out"

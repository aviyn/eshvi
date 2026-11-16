#!/bin/sh
# Regenerates the checked-in AST dumps (crates/svtrace/tests/data/*.json) that svtrace's tests read,
# one per fixture .sv, with the slang that svtrace is pinned to (v12.1). Run it after editing a
# fixture or moving to another slang release. It records a hash of each source in sources.fnv; the
# tests fail when a .sv no longer matches it.
#   scripts/svtrace-fixtures.sh              all fixtures
#   scripts/svtrace-fixtures.sh cone_test    the named ones
set -eu
slang=${SLANG:-$HOME/tools/slang/slang}
data=$(cd "$(dirname "$0")/.." && pwd)/crates/svtrace/tests/data

[ -x "$slang" ] || { echo "no slang at $slang (set SLANG)" >&2; exit 1; }

if [ $# -eq 0 ]; then
    set -- $(cd "$data" && ls *.sv | sed 's/\.sv$//')
fi

# Run from the fixture folder so the dump's file names are relative, as svtrace sees them. The
# flags are the ones svtrace passes (see elaborate.rs).
cd "$data"
# (A dump indents a level per nesting level, so the deeply nested ones are written without indentation,
# with the patched slang's flag: they would be tens of megabytes otherwise.)
compact=" deep "
for name in "$@"; do
    flag=
    case "$compact" in *" $name "*) flag=--ast-json-compact ;; esac
    "$slang" "$name.sv" --ast-json "$name.json" --ast-json-source-info --ast-json-detailed-types $flag \
        --disable-instance-caching -q || [ $? -eq 1 ]
    echo "$name.json"
done

# FNV-1a (64 bits) of each source made, in sources.fnv, as tests/fixtures.rs computes it.
python3 - "$@" <<'PY'
import sys
def fnv(data):
    h = 0xcbf29ce484222325
    for b in data:
        h = ((h ^ b) * 0x100000001b3) & 0xffffffffffffffff
    return '%016x' % h
known = {}
try:
    for line in open('sources.fnv'):
        name, _, value = line.strip().partition(' ')
        if name:
            known[name] = value
except FileNotFoundError:
    pass
for stem in sys.argv[1:]:
    known[stem + '.sv'] = fnv(open(stem + '.sv', 'rb').read())
with open('sources.fnv', 'w') as out:
    for name in sorted(known):
        out.write('%s %s\n' % (name, known[name]))
PY

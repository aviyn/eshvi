//! The checked-in dumps stand in for slang in the other tests, so they must come from the sources
//! next to them: `scripts/svtrace-fixtures.sh` makes them again, and records in `sources.fnv` a hash of
//! each source it made a dump from. (A hash, not file times: a checkout gives every file the same time.)

use std::collections::HashMap;
use std::fs;

/// FNV-1a, 64 bits: what the script records.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn every_dump_was_made_from_its_source_as_it_is() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let manifest = fs::read_to_string(dir.join("sources.fnv")).expect("tests/data/sources.fnv: run scripts/svtrace-fixtures.sh");
    let recorded: HashMap<&str, &str> = manifest.lines().filter_map(|line| line.split_once(' ')).collect();
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let source = entry.unwrap().path();
        if source.extension().is_none_or(|extension| extension != "sv") {
            continue;
        }
        let name = source.file_name().unwrap().to_str().unwrap();
        let stem = name.trim_end_matches(".sv");
        assert!(source.with_extension("json").exists(), "{name} has no dump: run scripts/svtrace-fixtures.sh {stem}");
        let hash = format!("{:016x}", fnv1a(&fs::read(&source).unwrap()));
        assert_eq!(recorded.get(name).copied(), Some(hash.as_str()), "{name} changed since its dump: run scripts/svtrace-fixtures.sh {stem}");
        checked += 1;
    }
    assert!(checked >= 4, "found only {checked} fixtures");
}

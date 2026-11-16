//! `assets/rust-crates.LICENSES.txt` lists the Rust crates built into the app with their
//! licenses, for the app's THIRD-PARTY-NOTICES.txt. `scripts/rust-notices.sh` makes it from
//! Cargo.lock and writes down which Cargo.lock: when that changes, the list has to be made again.

#[test]
fn the_rust_crates_notices_are_made_from_this_cargo_lock() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let lock = std::fs::read(format!("{root}/Cargo.lock")).expect("Cargo.lock");
    // FNV-1a, 64 bits, as the script computes it.
    let hash = lock.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let notices = std::fs::read_to_string(format!("{root}/assets/rust-crates.LICENSES.txt")).expect("the notices");
    assert!(
        notices.starts_with(&format!("Made from Cargo.lock {hash:016x} ")),
        "Cargo.lock changed since the Rust crates' notices were made: run scripts/rust-notices.sh \
         (it needs cargo-about: `cargo install cargo-about --locked --features cli`)"
    );
}

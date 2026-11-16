//! Nesting past serde_json's 128 levels (deep.sv): an expression of 300 chained operators and an
//! `else if` chain of 300. The dump reads, and the trace answers through them.

mod common;

use common::{fixture, DRIVERS, LOADS};

fn sites_of(f: &common::Fixture, command: &str, path: &str, endpoint: &str) -> Vec<String> {
    let answer = f.answer(command, path);
    let entry = answer.as_array().unwrap().iter().find(|entry| entry["path"] == endpoint).unwrap_or_else(|| panic!("{endpoint} in {path}"));
    entry["sites"].as_array().unwrap().iter().map(|site| site["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_long_chain_of_operators_reads_and_traces() {
    let f = fixture("deep");
    // The 300 reads are of one signal in one statement: one site, in both directions.
    assert_eq!(sites_of(&f, DRIVERS, "top.y", "top.a"), vec!["data"]);
    assert_eq!(sites_of(&f, LOADS, "top.a", "top.y"), vec!["data"]);
}

#[test]
fn a_long_else_if_chain_reads_and_traces() {
    let f = fixture("deep");
    let controls = sites_of(&f, DRIVERS, "top.z", "top.k");
    assert_eq!(controls, vec!["control"; 300]);
    assert_eq!(sites_of(&f, DRIVERS, "top.z", "top.a"), vec!["data"; 300]);
    assert_eq!(sites_of(&f, LOADS, "top.k", "top.z"), vec!["control"; 300]);
}

#[test]
fn the_walks_and_bit_paths_go_through_it_too() {
    let f = fixture("deep");
    let entries = f.step("drivers", "top.y", serde_json::json!({}));
    assert_eq!(common::paths(&entries), ["top.a"]);
    let entries = f.step("drivers", "top.z", serde_json::json!({}));
    assert_eq!(common::paths(&entries), ["top.a", "top.k"]);
    let entries = f.step("loads", "top.a[7]", serde_json::json!({}));
    assert!(common::paths(&entries).contains(&"top.y") && common::paths(&entries).contains(&"top.z"), "{entries:?}");
    assert!(!f.endpoints("drivers", "top.y").is_empty());
}

#[test]
fn a_build_and_its_questions_need_no_stack_of_their_own() {
    use std::os::unix::fs::PermissionsExt;
    // slang is a script that prints the dump; the thread has a stack far too small for 300 levels of
    // recursion in a debug build, so everything must run on the one svtrace makes.
    let dir = std::env::temp_dir().join(format!("svtrace-deep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".slang")).unwrap();
    std::fs::copy(common::data_dir().join("deep.sv"), dir.join("deep.sv")).unwrap();
    std::fs::write(dir.join("deep.f"), "deep.sv\n").unwrap();
    std::fs::write(dir.join(".slang/server.json"), r#"{ "build": "deep.f" }"#).unwrap();
    let slang = dir.join("fake-slang");
    std::fs::write(&slang, format!("#!/bin/sh\ncat '{}'\n", common::data_dir().join("deep.json").display())).unwrap();
    std::fs::set_permissions(&slang, std::fs::Permissions::from_mode(0o755)).unwrap();
    let workspace = std::fs::canonicalize(&dir).unwrap();

    let answer = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let built = svtrace::build(&slang, &workspace).expect("the build");
            let argument = serde_json::json!({ "path": "top.z", "direction": "drivers" });
            let answer = svtrace::trace::run(&built.graph, "svtrace.step", &[argument]).expect("an answer");
            let scopes = svtrace::hier::run(&built.hier, "slang.getScopes", &[serde_json::json!("")]).expect("scopes");
            (answer, scopes)
        })
        .unwrap()
        .join()
        .expect("no overflow");
    assert_eq!(common::paths(answer.0.as_array().unwrap()), ["top.a", "top.k"]);
    assert!(!answer.1.as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

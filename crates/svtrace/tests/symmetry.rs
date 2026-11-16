//! Drivers and loads are the same relation read from its two ends: if `A` is a driver of `B`, then `B` is
//! a load of `A`, with the same sites (each site is the use of `A`, and both answers put it there, with
//! the kind both give it). Checked for every signal of every fixture, so a rule that one tracer has and the
//! other lacks (a read outside an assignment, a loop condition, a left-side index, the items of a `case`)
//! shows up here whatever the fixture.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{data_dir, fixture, Fixture, DRIVERS, LOADS};

/// (line, column, kind) of each site, by endpoint path.
type Cone = BTreeMap<String, BTreeSet<(u64, u64, String)>>;

fn cone(f: &Fixture, command: &str, path: &str) -> Cone {
    // (A signal nothing reads has no loads: the command says so with an error.)
    let Ok(answer) = svtrace::trace::execute(&f.graph, command, path) else { return Cone::new() };
    let mut cone = Cone::new();
    for entry in answer.as_array().expect("a list") {
        let sites = cone.entry(entry["path"].as_str().unwrap().to_string()).or_default();
        for site in entry["sites"].as_array().unwrap() {
            let start = &site["location"]["range"]["start"];
            sites.insert((start["line"].as_u64().unwrap(), start["character"].as_u64().unwrap(), site["kind"].as_str().unwrap().to_string()));
        }
    }
    cone
}

/// Pairs that are meant to differ: (fixture, signal whose answer is looked at, endpoint in it), and why.
/// None so far: the exceptions that exist are not in the leaves (a constant driver is not an endpoint,
/// and a signal nothing reads has no loads), and are handled above.
const EXCEPTIONS: &[(&str, &str, &str, &str)] = &[];

fn excepted(fixture: &str, path: &str, other: &str) -> bool {
    EXCEPTIONS.iter().any(|(f, p, o, _)| *f == fixture && *p == path && *o == other)
}

fn fixtures() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data_dir())
        .unwrap()
        .filter_map(|entry| entry.unwrap().path().file_name()?.to_str()?.strip_suffix(".sv").map(str::to_string))
        .collect();
    names.sort();
    names
}

#[test]
fn drivers_and_loads_are_each_others_mirror() {
    let mut pairs = 0;
    let mut problems = Vec::new();
    for name in fixtures() {
        let f = fixture(&name);
        let mut paths: Vec<String> = f
            .graph
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, svtrace::ast::ValueKind::Variable | svtrace::ast::ValueKind::Net))
            .map(|symbol| symbol.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        for path in &paths {
            for (command, other_command) in [(DRIVERS, LOADS), (LOADS, DRIVERS)] {
                for (other, sites) in cone(&f, command, path) {
                    if excepted(&name, path, &other) {
                        continue;
                    }
                    pairs += 1;
                    // `path` has `other` as a driver (or a load): `other` must have `path` as a load (a driver), at these sites.
                    let back = cone(&f, other_command, &other);
                    match back.get(path) {
                        None => problems.push(format!("{name}: {other} is in the {command} of {path}, but {path} is not in its {other_command}")),
                        Some(back_sites) if *back_sites != sites => problems.push(format!(
                            "{name}: {command} {path} -> {other} has sites {sites:?}, {other_command} {other} -> {path} has {back_sites:?}"
                        )),
                        Some(_) => {}
                    }
                }
            }
        }
    }
    assert!(pairs > 300, "only {pairs} pairs were checked");
    assert!(problems.is_empty(), "{} of {pairs} pairs differ:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn the_fixture_with_every_rule_has_the_pairs_it_is_meant_to() {
    // A guard for the test above: it must see the interesting ones (this fixture holds them all).
    let f = fixture("writes");
    let drivers = cone(&f, DRIVERS, "top.mem");
    assert!(drivers.contains_key("top.waddr"), "{drivers:?}");
}

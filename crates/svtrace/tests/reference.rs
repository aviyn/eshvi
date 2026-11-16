//! `svtrace.endpoints` against a port of `collect_endpoints` in `slang/trace.lua`, which walks with the
//! phase 1 commands one question at a time. Every signal of every fixture, both directions; and
//! CV32E40P's when `SVTRACE_DESIGN` names its folder (`cargo test --release -- --ignored`).

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use common::{fixture, CONSTANTS, ENDPOINTS, LOADS};
use serde_json::{json, Value};
use svtrace::graph::Graph;
use svtrace::trace;

/// `is_constant` in the Lua: a package enum's member, `pkg::enum_e.MEMBER`.
fn lua_is_constant(path: &str) -> bool {
    let word = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    let Some((package, rest)) = path.split_once("::") else { return false };
    let Some((name, member)) = rest.split_once('.') else { return false };
    word(package) && word(name) && word(member)
}

/// `at_top` in the Lua: `top.x`, one level under the top module.
fn lua_at_top(path: &str) -> bool {
    match path.split_once('.') {
        Some((top, name)) => ![top, name].iter().any(|part| part.is_empty() || part.contains(['.', ':'])),
        None => false,
    }
}

/// `collect_endpoints` of `slang/trace.lua`, with its question limit gone.
fn collect_endpoints(graph: &Graph, path: &str, direction: &str) -> Result<Vec<Value>, String> {
    let mut seen: HashSet<String> = HashSet::from([path.to_string()]);
    let mut lines: HashSet<String> = HashSet::new();
    let mut found: Vec<Value> = Vec::new();
    let mut add = |location: &Value| {
        let key = format!("{}:{}", location["uri"], location["range"]["start"]["line"]);
        if lines.insert(key) {
            found.push(location.clone());
        }
    };
    // (path, the entry it was reached as)
    let mut frontier: Vec<(String, Option<Value>)> = vec![(path.to_string(), None)];
    while !frontier.is_empty() {
        let mut following = Vec::new();
        for (item, entry) in &frontier {
            let (answer, constants) = if direction == "loads" {
                (trace::execute(graph, LOADS, item), Vec::new())
            } else {
                match trace::execute(graph, CONSTANTS, item) {
                    Ok(both) => (Ok(both["drivers"].clone()), both["constants"].as_array().unwrap().clone()),
                    Err(error) => (Err(error), Vec::new()),
                }
            };
            let answer = match answer {
                Ok(answer) => answer,
                // The signal asked for: said. Further on: something that can't be followed.
                Err(error) if entry.is_none() => return Err(error),
                Err(_) => continue,
            };
            // normalize: once each, sorted by path
            let mut entries: Vec<Value> = Vec::new();
            for e in answer.as_array().unwrap() {
                if !entries.iter().any(|seen| seen["path"] == e["path"]) {
                    entries.push(e.clone());
                }
            }
            entries.sort_by(|a, b| a["path"].as_str().unwrap().cmp(b["path"].as_str().unwrap()));
            for e in &entries {
                let entry_path = e["path"].as_str().unwrap();
                let sites = if lua_is_constant(entry_path) { Vec::new() } else { e["sites"].as_array().unwrap().clone() };
                for site in sites {
                    match site["kind"].as_str().unwrap() {
                        "port" | "wire" => {
                            if seen.insert(entry_path.to_string()) {
                                following.push((entry_path.to_string(), Some(e.clone())));
                            }
                        }
                        "data" => add(&site["location"]),
                        "control" | "clock" if direction == "loads" => add(&site["location"]),
                        _ => {}
                    }
                }
            }
            for site in &constants {
                add(&site["location"]);
            }
            if entries.is_empty() && constants.is_empty() && lua_at_top(item) {
                if let Some(entry) = entry {
                    add(&entry["location"]);
                }
            }
        }
        frontier = following;
    }
    Ok(found)
}

/// The file and line of each location, sorted: the Lua lists one location per line, and which one it
/// keeps, and when, is down to what it met first.
fn lines(locations: &Result<Vec<Value>, String>) -> Result<Vec<(String, u64)>, String> {
    locations.clone().map(|list| {
        let mut lines: Vec<_> =
            list.iter().map(|l| (l["uri"].as_str().unwrap().to_string(), l["range"]["start"]["line"].as_u64().unwrap())).collect();
        lines.sort();
        lines
    })
}

/// Compares the two for every signal (`exact`: the same locations in the same order, else the same lines); returns how many
/// (signal, direction) pairs were compared and the differences.
///
/// A module-local enum member is a signal to the Lua's path pattern and a constant to the graph, so
/// its data site is a driver endpoint to one and the constant's place to the other: the same line,
/// not always the same column, and found at another time.
fn compare(graph: &Graph, exact: bool) -> (usize, Vec<String>) {
    let mut compared = 0;
    let mut differences = Vec::new();
    let paths: Vec<String> = graph.signals().map(str::to_string).collect();
    for path in &paths {
        for direction in ["drivers", "loads"] {
            let reference = collect_endpoints(graph, path, direction);
            let got = trace::run(graph, ENDPOINTS, &[json!({ "path": path, "direction": direction })]).map(|a| a.as_array().unwrap().clone());
            compared += 1;
            let same = if exact { reference == got } else { lines(&reference) == lines(&got) };
            if !same {
                differences.push(format!("{direction} {path}: reference {reference:?}, svtrace {got:?}"));
            }
        }
    }
    (compared, differences)
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(common::data_dir())
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension()? == "sv").then(|| path.file_stem().unwrap().to_string_lossy().to_string())
        })
        .collect();
    names.sort();
    names
}

#[test]
fn endpoints_match_the_lua_walk_for_every_signal_of_every_fixture() {
    let mut total = 0;
    for name in fixture_names() {
        let f = fixture(&name);
        let (compared, differences) = compare(&f.graph, true);
        assert!(differences.is_empty(), "{name}: {} of {compared} differ:\n{}", differences.len(), differences.join("\n"));
        total += compared;
    }
    assert!(total > 400, "only {total} signals compared");
}

#[test]
fn the_reference_walk_is_the_lua_s() {
    // A package enum's member is a constant to the Lua; a module-local one is not.
    assert!(lua_is_constant("kinds::state_e.BUSY"));
    assert!(!lua_is_constant("steps_top.local_e.L_B"));
    assert!(!lua_is_constant("top.x"));
    assert!(lua_at_top("top.x"));
    assert!(!lua_at_top("top.u.x"));
    assert!(!lua_at_top("pkg::x"));
}

/// CV32E40P, or another design: `SVTRACE_DESIGN=~/projects/cv32e40p cargo test --release -p svtrace
/// --test reference -- --ignored`. Needs the slang at `~/tools/slang/slang`.
#[test]
#[ignore = "needs SVTRACE_DESIGN and slang"]
fn endpoints_match_the_lua_walk_by_line_for_every_signal_of_a_real_design() {
    let Some(design) = std::env::var_os("SVTRACE_DESIGN") else {
        eprintln!("skipped: SVTRACE_DESIGN is not set");
        return;
    };
    let slang = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/slang/slang");
    let design = Path::new(&design).canonicalize().expect("SVTRACE_DESIGN is a folder");
    let built = svtrace::build(&slang, &design).expect("slang runs on the design");
    let (compared, differences) = compare(&built.graph, false);
    eprintln!("{compared} (signal, direction) pairs compared, {} differ", differences.len());
    assert!(differences.is_empty(), "{} of {compared} differ:\n{}", differences.len(), differences.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
}

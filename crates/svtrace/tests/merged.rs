//! `svtrace.step` and `svtrace.endpoints` with `paths`: the instances of a module, merged into
//! module-relative rows (fixture merged.sv).

mod common;

use common::{entry, fixture, kinds, paths, Fixture, ENDPOINTS, STEP};
use serde_json::{json, Value};
use svtrace::trace;

fn merged(f: &Fixture, direction: &str, starts: &[&str], extra: Value) -> Vec<Value> {
    let mut argument = json!({ "paths": starts, "direction": direction });
    for (key, value) in extra.as_object().into_iter().flatten() {
        argument[key] = value.clone();
    }
    trace::run(&f.graph, STEP, &[argument]).unwrap_or_else(|error| panic!("step {starts:?}: {error}")).as_array().unwrap().clone()
}

fn strings(value: &Value) -> Vec<&str> {
    value.as_array().unwrap().iter().map(|item| item.as_str().unwrap()).collect()
}

fn endpoint_lines(f: &Fixture, argument: Value) -> Vec<u64> {
    let answer = trace::run(&f.graph, ENDPOINTS, &[argument]).unwrap();
    answer.as_array().unwrap().iter().map(|location| location["range"]["start"]["line"].as_u64().unwrap()).collect()
}

#[test]
fn instances_connected_alike_are_one_row() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.u1.a", "merged_top.u2.a"], json!({}));
    assert_eq!(paths(&rows), ["merged_top.x"]);
    assert_eq!(kinds(&rows[0]), ["port", "port"]);
    assert_eq!(strings(&rows[0]["from"]), ["merged_top.u1.a", "merged_top.u2.a"]);
    assert_eq!(strings(&rows[0]["paths"]), ["merged_top.x"]);
}

#[test]
fn instances_connected_differently_split_into_rows() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.u1.a", "merged_top.u2.a", "merged_top.u3.a"], json!({}));
    assert_eq!(paths(&rows), ["merged_top.x", "merged_top.y"]);
    assert_eq!(strings(&entry(&rows, "merged_top.x")["from"]), ["merged_top.u1.a", "merged_top.u2.a"]);
    assert_eq!(strings(&entry(&rows, "merged_top.y")["from"]), ["merged_top.u3.a"]);
}

#[test]
fn an_internal_chain_is_one_row_per_step_and_ends_where_the_instances_differ() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.u1.q", "merged_top.u3.q"], json!({}));
    // The clock and the data: both modules' lines, so a row each, with both instances' paths.
    assert_eq!(paths(&rows), ["merged_top.u1.clk", "merged_top.u1.m2"]);
    let m2 = entry(&rows, "merged_top.u1.m2");
    assert_eq!(strings(&m2["paths"]), ["merged_top.u1.m2", "merged_top.u3.m2"]);
    assert_eq!(strings(&m2["from"]), ["merged_top.u1.q", "merged_top.u3.q"]);
    assert_eq!((m2["count"].as_u64(), m2["total"].as_u64()), (Some(1), Some(1)));
    // m2 <- m1 <- a, all still shared, and a is where the two parents connect differently.
    let a = &m2["next"][0]["next"][0];
    assert_eq!(a["path"], "merged_top.u1.a");
    assert_eq!(strings(&a["paths"]), ["merged_top.u1.a", "merged_top.u3.a"]);
    assert_eq!((a["count"].as_u64(), a["total"].as_u64()), (Some(2), Some(2)));
    // Two signals in its next step, so it is not followed; its own step splits.
    assert!(a.get("next").is_none());
    let split = merged(&f, "drivers", &["merged_top.u1.a", "merged_top.u3.a"], json!({}));
    assert_eq!(paths(&split), ["merged_top.x", "merged_top.y"]);
    // The clock is a row of its own, counted and followed like any other.
    let clk = entry(&rows, "merged_top.u1.clk");
    assert_eq!(kinds(clk), ["clock"]);
    assert_eq!(clk["next"][0]["path"], "merged_top.clk");
    assert_eq!(strings(&clk["next"][0]["from"]), ["merged_top.u1.clk", "merged_top.u3.clk"]);
}

#[test]
fn a_generated_array_on_one_signal_is_one_row_from_every_instance() {
    let f = fixture("merged");
    let starts = ["merged_top.g[0].c.a", "merged_top.g[1].c.a", "merged_top.g[2].c.a"];
    let rows = merged(&f, "drivers", &starts, json!({}));
    assert_eq!(paths(&rows), ["merged_top.vec"]);
    // (One source line connects them all: its site is there once.)
    assert_eq!(kinds(&rows[0]), ["port"]);
    assert_eq!(strings(&rows[0]["from"]), starts);
}

#[test]
fn a_branch_elaborated_in_one_instance_only_is_a_row_from_that_instance() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.gw.o", "merged_top.gn.o"], json!({}));
    assert_eq!(paths(&rows), ["merged_top.gn.d", "merged_top.gw.g_wide.w"]);
    assert_eq!(strings(&entry(&rows, "merged_top.gn.d")["from"]), ["merged_top.gn.o"]);
    assert_eq!(strings(&entry(&rows, "merged_top.gw.g_wide.w")["from"]), ["merged_top.gw.o"]);
    // Each goes on by itself, to x (in two rows: they are different lines of different instances' own chains).
    assert_eq!(entry(&rows, "merged_top.gn.d")["next"][0]["path"], "merged_top.x");
    assert_eq!(entry(&rows, "merged_top.gw.g_wide.w")["next"][0]["path"], "merged_top.gw.d");
}

#[test]
fn a_parent_instantiated_twice_is_rows_with_both_instances_paths() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.p1.c.m1", "merged_top.p2.c.m1"], json!({}));
    assert_eq!(paths(&rows), ["merged_top.p1.c.a"]);
    let x_int = &rows[0]["next"][0];
    assert_eq!(strings(&x_int["paths"]), ["merged_top.p1.x_int", "merged_top.p2.x_int"]);
    let x = &x_int["next"][0]["next"][0];
    assert_eq!(x["path"], "merged_top.x");
    assert_eq!(strings(&x["from"]), ["merged_top.p1.x", "merged_top.p2.x"]);
    // `prefer` puts the preferred instance's paths first, and the row's path is the first.
    let rows = merged(&f, "drivers", &["merged_top.p1.c.m1", "merged_top.p2.c.m1"], json!({ "prefer": "merged_top.p2" }));
    assert_eq!(strings(&rows[0]["paths"]), ["merged_top.p2.c.a", "merged_top.p1.c.a"]);
    assert_eq!(rows[0]["path"], "merged_top.p2.c.a");
    assert_eq!(rows[0]["signal"], "merged_top.p2.c.a");
}

#[test]
fn counts_and_totals_are_of_the_rows_own_merged_step() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.p1.z", "merged_top.p2.z"], json!({ "follow": false }));
    let q = entry(&rows, "merged_top.p1.c.q");
    // The clock and the data of the child: two rows, one of them a signal.
    assert_eq!((q["count"].as_u64(), q["total"].as_u64()), (Some(1), Some(2)));
    let rows = merged(&f, "drivers", &["merged_top.u1.m1", "merged_top.u2.m1", "merged_top.u3.m1"], json!({ "follow": false }));
    assert_eq!((rows[0]["count"].as_u64(), rows[0]["total"].as_u64()), (Some(2), Some(2)));
    assert!(rows[0].get("next").is_none());
}

#[test]
fn loads_merge_too() {
    let f = fixture("merged");
    let rows = merged(&f, "loads", &["merged_top.u1.q", "merged_top.u2.q", "merged_top.u3.q"], json!({ "follow": false }));
    assert_eq!(paths(&rows), ["merged_top.q1", "merged_top.q2", "merged_top.q3"]);
    let rows = merged(&f, "loads", &["merged_top.u1.a", "merged_top.u2.a"], json!({ "follow": false }));
    assert_eq!(paths(&rows), ["merged_top.u1.m1"]);
    assert_eq!(strings(&rows[0]["paths"]), ["merged_top.u1.m1", "merged_top.u2.m1"]);
}

#[test]
fn a_row_with_one_of_its_paths_above_is_a_loop() {
    let f = fixture("merged");
    let rows = merged(&f, "drivers", &["merged_top.u1.q", "merged_top.u2.q"], json!({ "follow": false, "ancestors": ["merged_top.u2.m2"] }));
    assert_eq!(entry(&rows, "merged_top.u1.m2")["loop"], true);
    assert_eq!(entry(&rows, "merged_top.u1.clk")["loop"], false);
}

#[test]
fn duplicate_paths_are_dropped_and_bad_ones_are_the_usual_errors() {
    let f = fixture("merged");
    let once = merged(&f, "drivers", &["merged_top.u1.q"], json!({}));
    let twice = merged(&f, "drivers", &["merged_top.u1.q", "merged_top.u1.q"], json!({}));
    assert_eq!(once, twice);
    let argument = json!({ "paths": ["merged_top.u1.q", "merged_top.nothing"], "direction": "drivers" });
    assert_eq!(f.step_error(argument), "Could not find path in compiled design: merged_top.nothing");
    assert!(f.step_error(json!({ "paths": [], "direction": "drivers" })).contains("no path"));
    assert!(f.step_error(json!({ "paths": "merged_top.x", "direction": "drivers" })).contains("paths"));
}

#[test]
fn endpoints_are_the_union_of_each_starts_walk() {
    let f = fixture("merged");
    for (direction, starts) in [
        ("drivers", vec!["merged_top.gw.o", "merged_top.gn.o"]),
        ("drivers", vec!["merged_top.u1.q", "merged_top.u3.q"]),
        ("loads", vec!["merged_top.u1.a", "merged_top.u2.a", "merged_top.u3.a"]),
        ("loads", vec!["merged_top.x", "merged_top.y"]),
    ] {
        let mut union: Vec<u64> = Vec::new();
        let mut lines = Vec::new();
        for start in &starts {
            for location in trace::run(&f.graph, ENDPOINTS, &[json!({ "path": start, "direction": direction })]).unwrap().as_array().unwrap() {
                let key = format!("{}:{}", location["uri"], location["range"]["start"]["line"]);
                if !lines.contains(&key) {
                    lines.push(key);
                    union.push(location["range"]["start"]["line"].as_u64().unwrap());
                }
            }
        }
        assert!(!union.is_empty(), "{direction} {starts:?}");
        assert_eq!(endpoint_lines(&f, json!({ "paths": starts, "direction": direction })), union, "{direction} {starts:?}");
    }
}

/// `paths` of one is `path`, row for row, for every signal of every fixture, both directions, followed or not;
/// the rows say `paths` of one and `from` the asked path, and endpoints too.
#[test]
fn one_path_in_paths_answers_exactly_as_path() {
    let mut checked = 0;
    for name in fixture_names() {
        let f = fixture(&name);
        let mut all: Vec<String> = f.graph.symbols.iter().map(|symbol| symbol.path.clone()).collect();
        all.sort();
        all.dedup();
        for path in &all {
            for direction in ["drivers", "loads"] {
                for follow in [true, false] {
                    let one = trace::run(&f.graph, STEP, &[json!({ "path": path, "direction": direction, "follow": follow })]);
                    let many = trace::run(&f.graph, STEP, &[json!({ "paths": [path], "direction": direction, "follow": follow })]);
                    assert_eq!(one, many, "{name}: step {direction} {path}");
                    if let Ok(rows) = one {
                        check_singletons(&rows, path, &name);
                        checked += 1;
                    }
                }
                let one = trace::run(&f.graph, ENDPOINTS, &[json!({ "path": path, "direction": direction })]);
                let many = trace::run(&f.graph, ENDPOINTS, &[json!({ "paths": [path], "direction": direction })]);
                assert_eq!(one, many, "{name}: endpoints {direction} {path}");
            }
        }
    }
    assert!(checked > 300, "only {checked} answers were checked");
}

/// In a one-path answer every row is of one path, and its `from` is the path asked (the row's parent's path).
fn check_singletons(rows: &Value, asked: &str, name: &str) {
    for row in rows.as_array().unwrap() {
        assert_eq!(row["paths"], json!([row["path"]]), "{name}: {asked}");
        assert_eq!(row["from"], json!([asked]), "{name}: {asked}");
        if let Some(next) = row.get("next") {
            check_singletons(next, row["path"].as_str().unwrap(), name);
        }
    }
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(common::data_dir())
        .unwrap()
        .filter_map(|entry| entry.unwrap().path().file_name()?.to_str()?.strip_suffix(".sv").map(str::to_string))
        .collect();
    names.sort();
    names
}

//! The edges of the rule set (edges.sv): `inout` and `ref` ports, a task's output argument, a
//! sensitivity list without edges, a type parameter's port, an assignment inside an expression, a
//! specparam, escaped names, paths into unpacked types. (tests/symmetry.rs checks that drivers and
//! loads agree on all of it.)

mod common;

use std::collections::BTreeMap;

use common::{fixture, paths, Fixture, CONSTANTS, DRIVERS, LOADS};
use serde_json::json;

/// The endpoints of `command` for `path`, each with the kinds of its sites in order.
fn cone(f: &Fixture, command: &str, path: &str) -> BTreeMap<String, Vec<String>> {
    f.answer(command, path)
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let kinds = entry["sites"].as_array().unwrap().iter().map(|site| site["kind"].as_str().unwrap().to_string()).collect();
            (entry["path"].as_str().unwrap().to_string(), kinds)
        })
        .collect()
}

fn expect(f: &Fixture, command: &str, path: &str, expected: &[(&str, &[&str])]) {
    let want: BTreeMap<String, Vec<String>> =
        expected.iter().map(|(name, kinds)| (format!("top.{name}"), kinds.iter().map(|k| k.to_string()).collect())).collect();
    assert_eq!(cone(f, command, path), want, "{command} {path}");
}

#[test]
fn an_inout_port_connects_both_ways() {
    let f = fixture("edges");
    // pad_cell u0(.io(pad), ...), u1(.io(pad), ...): the net and each port drive and load each other.
    expect(&f, DRIVERS, "top.pad", &[("u0.io", &["port"]), ("u1.io", &["port"])]);
    expect(&f, LOADS, "top.pad", &[("u0.io", &["port"]), ("u1.io", &["port"])]);
    expect(&f, DRIVERS, "top.u0.io", &[("pad", &["port"]), ("u0.oe", &["data"]), ("u0.o", &["data"])]);
    expect(&f, LOADS, "top.u0.io", &[("pad", &["port"]), ("u0.i", &["wire"])]);
    // The site is the connection, from either end.
    f.check_sites(DRIVERS, "top.pad", "top.u0.io", &[(f.after("u0(.io("), "port")]);
    f.check_sites(LOADS, "top.u0.io", "top.pad", &[(f.after("u0(.io("), "port")]);
}

#[test]
fn a_trace_goes_through_an_inout_port_to_the_logic_behind_it() {
    let f = fixture("edges");
    // Drivers of the pad end at `assign io = oe ? o : 1'bz;`, loads at the top's `i` (through `assign i = io;`).
    let (line, _) = f.before("assign io = oe").position();
    assert_eq!(f.endpoints("drivers", "top.pad").iter().map(|place| place.0).collect::<Vec<_>>(), [line]);
    assert!(!f.endpoints("loads", "top.pad").is_empty());
    // The pad comes back as a loop under its port, not as a chain without end.
    let entries = f.step("drivers", "top.pad", json!({}));
    assert_eq!(paths(&entries), ["top.u0.io", "top.u1.io"]);
    let next = f.step("drivers", "top.u0.io", json!({ "ancestors": ["top.pad"] }));
    assert_eq!(common::entry(&next, "top.pad")["loop"], true);
}

#[test]
fn an_inout_port_on_a_part_of_a_bus_is_that_part() {
    let f = fixture("edges");
    assert_eq!(paths(&f.step("drivers", "top.bus[1]", json!({ "follow": false }))), ["top.u3.io"]);
    assert_eq!(paths(&f.step("loads", "top.bus[0]", json!({ "follow": false }))), ["top.u2.io"]);
    assert!(paths(&f.step("loads", "top.u3.io", json!({ "follow": false }))).contains(&"top.bus"));
}

#[test]
fn a_ref_port_is_read_and_written_through() {
    let f = fixture("edges");
    // refmod ur(.r(rv), ...): `r <= 1'b1` inside writes rv, `q = r` inside reads it.
    expect(&f, DRIVERS, "top.rv", &[("ur.r", &["port"])]);
    expect(&f, LOADS, "top.rv", &[("ur.r", &["port"])]);
    assert_eq!(cone(&f, DRIVERS, "top.ur.r")["top.rv"], ["port"]);
    assert_eq!(cone(&f, LOADS, "top.ur.r")["top.rv"], ["port"]);
    assert_eq!(cone(&f, LOADS, "top.ur.r")["top.ur.q"], ["wire"]);
}

#[test]
fn a_tasks_output_argument_is_driven_by_its_other_arguments() {
    let f = fixture("edges");
    // setit(tk, av[1]);
    expect(&f, DRIVERS, "top.tk", &[("av", &["data"])]);
    f.check_sites(DRIVERS, "top.tk", "top.av", &[(f.after("setit(tk, "), "data")]);
    f.check_constants("top.tk", &[]);
    assert_eq!(cone(&f, LOADS, "top.av")["top.tk"], ["data"]);
    // clear(tn): no signal goes in, so it is a call's value.
    expect(&f, DRIVERS, "top.tn", &[]);
    f.check_constants("top.tn", &[(f.before("clear(tn)"), "call")]);
}

#[test]
fn a_sensitivity_list_without_edges_is_not_a_clock() {
    let f = fixture("edges");
    // always @(a or b or k1) lv = a & b | k1;
    expect(&f, DRIVERS, "top.lv", &[("a", &["data"]), ("b", &["data"]), ("k1", &["data"])]);
    assert_eq!(cone(&f, LOADS, "top.k1")["top.lv"], ["data"]);
    // always @(lw_a) lw = lw_a; is combinational, so the copy is a wire.
    expect(&f, DRIVERS, "top.lw", &[("lw_a", &["wire"])]);
    // always @(posedge clk or mx_r) mx <= a; has an edge: the list is clocks, as before.
    expect(&f, DRIVERS, "top.mx", &[("clk", &["clock"]), ("mx_r", &["clock"]), ("a", &["data"])]);
}

#[test]
fn a_port_of_a_type_parameters_type_keeps_its_bits() {
    let f = fixture("edges");
    // sub #(.T(pair_t)) ut(.d(p), .q(pq));
    assert_eq!(paths(&f.step("drivers", "top.pq[3:0]", json!({ "follow": false }))), ["top.ut.q.lo"]);
    assert_eq!(paths(&f.step("drivers", "top.ut.d.hi", json!({ "follow": false }))), ["top.p.hi"]);
    assert_eq!(paths(&f.step("loads", "top.p.lo", json!({ "follow": false }))), ["top.ut.d.lo"]);
}

#[test]
fn an_assignment_inside_an_expression_writes_its_target_and_is_read_by_the_one_around_it() {
    let f = fixture("edges");
    // ny = (nb = a) + b;
    expect(&f, DRIVERS, "top.ny", &[("a", &["data"]), ("b", &["data"])]);
    expect(&f, DRIVERS, "top.nb", &[("a", &["wire"])]);
    let loads = cone(&f, LOADS, "top.a");
    assert_eq!((&loads["top.nb"], &loads["top.ny"]), (&vec!["wire".to_string()], &vec!["data".to_string()]));
    assert_eq!(cone(&f, LOADS, "top.b")["top.ny"], ["data"]);
}

#[test]
fn a_specparam_is_a_constant_not_a_signal() {
    let f = fixture("edges");
    // assign sp_y = t_in & sp;
    expect(&f, DRIVERS, "top.sp_y", &[("t_in", &["data"])]);
    assert!(!f.graph.signals().any(|path| path == "top.sp"));
}

#[test]
fn an_escaped_name_is_found_by_slangs_spelling_and_by_the_plain_one() {
    let f = fixture("edges");
    let escaped = f.answer(DRIVERS, "top.\\esc-out ");
    assert_eq!(escaped[0]["path"], "top.\\esc-sig ");
    assert_eq!(f.answer(DRIVERS, "top.esc-out"), escaped);
    assert_eq!(f.answer(CONSTANTS, "top.esc-inst.y")["drivers"][0]["path"], "top.\\esc-inst .x");
    assert!(f.graph.signals().any(|path| path == "top.\\esc-sig "));
}

#[test]
fn a_path_into_an_unpacked_type_must_name_a_part_it_has() {
    let f = fixture("edges");
    // logic [7:0] mem [4]; rec_t rec;  A part that exists is traced as the whole signal.
    for path in ["top.mem[3]", "top.mem[0][7:4]", "top.mem[1:2]", "top.rec.f", "top.rec.f[2]", "top.rec.g"] {
        assert!(!f.step("drivers", path, json!({ "follow": false })).is_empty(), "{path}");
    }
    for path in ["top.mem[4]", "top.mem[999]", "top.mem.nonsense", "top.mem[0][8]", "top.rec.zz", "top.rec.f[4]", "top.rec[0]"] {
        let error = f.step_error(json!({ "path": path }));
        assert_eq!(error, format!("Could not find path in compiled design: {path}"));
    }
}

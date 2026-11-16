//! One site per signal per statement (statements.sv): a signal read in several places of one assignment,
//! continuous assign, port connection or condition is one site, the first by position; a read in another
//! statement is a site of its own.

mod common;

use common::{fixture, DRIVERS, LOADS};

#[test]
fn an_assignment_over_several_lines_reads_a_signal_once() {
    let f = fixture("statements");
    // `a` is read on the first and the third line of the assign: the first.
    f.check_sites(DRIVERS, "top.y", "top.a", &[(f.after("assign y = "), "data")]);
    f.check_sites(DRIVERS, "top.y", "top.b", &[(f.before("b |"), "data")]);
    f.check_sites(LOADS, "top.a", "top.y", &[(f.after("assign y = "), "data")]);
    f.check_sites(LOADS, "top.b", "top.y", &[(f.before("b |"), "data")]);
}

#[test]
fn a_clocked_assignment_over_several_lines_too() {
    let f = fixture("statements");
    f.check_sites(DRIVERS, "top.t", "top.a", &[(f.after("t <= "), "data")]);
    f.check_sites(LOADS, "top.a", "top.t", &[(f.after("t <= "), "data")]);
    // (The event control is a statement of its own, and its signal a site there.)
    f.check_sites(DRIVERS, "top.t", "top.clk", &[(f.after("@(posedge "), "clock")]);
}

#[test]
fn a_condition_over_several_lines_reads_a_signal_once() {
    let f = fixture("statements");
    // `sel` is read on the first and the third line of the condition: one control site, the first.
    f.check_sites(DRIVERS, "top.z", "top.sel", &[(f.after("if ("), "control")]);
    f.check_sites(DRIVERS, "top.z", "top.k", &[(f.before("k ||"), "control")]);
    f.check_sites(LOADS, "top.sel", "top.z", &[(f.after("if ("), "control")]);
    f.check_sites(LOADS, "top.k", "top.z", &[(f.before("k ||"), "control")]);
}

#[test]
fn a_case_selector_and_an_items_expressions_are_conditions() {
    let f = fixture("statements");
    f.check_sites(DRIVERS, "top.r", "top.sel", &[(f.after("case ("), "control")]);
    // The two expressions of the item are one condition: one site.
    f.check_sites(DRIVERS, "top.r", "top.a", &[(f.before("a,\n"), "control")]);
    f.check_sites(LOADS, "top.a", "top.r", &[(f.before("a,\n"), "control")]);
}

#[test]
fn a_port_connection_over_several_lines_reads_a_signal_once() {
    let f = fixture("statements");
    f.check_sites(DRIVERS, "top.u.i", "top.a", &[(f.after("leaf u(.i({"), "port")]);
    f.check_sites(LOADS, "top.a", "top.u.i", &[(f.after("leaf u(.i({"), "port")]);
}

#[test]
fn a_signal_read_in_two_statements_has_a_site_in_each() {
    let f = fixture("statements");
    f.check_sites(LOADS, "top.a", "top.w", &[(f.after("assign w = "), "wire")]);
    f.check_sites(LOADS, "top.a", "top.q", &[(f.after("assign q = ~"), "data")]);
    // The same through the walk, which lists the sites a row has.
    let rows = f.step("loads", "top.a", serde_json::json!({}));
    for (path, count) in [("top.y", 1), ("top.w", 1), ("top.q", 1), ("top.t", 1), ("top.r", 1), ("top.u.i", 1)] {
        assert_eq!(common::entry(&rows, path)["sites"].as_array().unwrap().len(), count, "{path}");
    }
}

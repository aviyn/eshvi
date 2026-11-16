//! Hierarchical paths, which must match the ones slang-server hands out: generate blocks and
//! loops, instance arrays, packages, escaped names, and the names an interface port gives its
//! interface's signals.

mod common;

use common::{fixture, DRIVERS};

fn signals(name: &str) -> Vec<String> {
    fixture(name).graph.signals().map(str::to_string).collect()
}

#[test]
fn generate_loops_use_the_loop_variable_not_the_construct_index() {
    let signals = signals("paths");
    for path in ["top.gen_x[2].v", "top.gen_x[3].v", "top.gen_x[4].v", "top.gen_neg[-1].w", "top.gen_neg[0].w"] {
        assert!(signals.contains(&path.to_string()), "{path} in {signals:?}");
    }
    assert!(!signals.iter().any(|path| path.starts_with("top.gen_x[0]")));
}

#[test]
fn nested_loops_and_unnamed_blocks() {
    let signals = signals("paths");
    for path in [
        "top.outer[0].inner_loop[0].z",
        "top.outer[1].inner_loop[1].z",
        "top.outer[1].inner_loop[0].u_in_gen.inner",
        // Unnamed blocks are named by their construct number, as slang prints them.
        "top.genblk3.first_if",
        "top.genblk4.second_if",
        "top.case_one.only_one",
    ] {
        assert!(signals.contains(&path.to_string()), "{path} in {signals:?}");
    }
    // The case branch that isn't taken isn't in the design.
    assert!(!signals.iter().any(|path| path.contains("case_other")));
}

#[test]
fn instance_arrays_number_their_elements_from_the_lower_bound() {
    let signals = signals("paths");
    for path in [
        // [3:0] and [2:4]: the numbers run from the lower bound whichever way the range is written.
        "top.u_desc[0].inner",
        "top.u_desc[3].inner",
        "top.u_asc[2].inner",
        "top.u_asc[4].inner",
        // [1:0][2:3]: one number per dimension.
        "top.u_2d[0][2].inner",
        "top.u_2d[1][3].inner",
    ] {
        assert!(signals.contains(&path.to_string()), "{path} in {signals:?}");
    }
    assert!(!signals.iter().any(|path| path.starts_with("top.u_asc[0]") || path.starts_with("top.u_asc[1]")));
}

#[test]
fn packages_use_double_colons_and_names_are_escaped_when_needed() {
    let signals = signals("paths");
    assert!(signals.contains(&"pkg::pkg_sig".to_string()));
    // `$` is allowed inside an identifier.
    assert!(signals.contains(&"top.esc$sig".to_string()));
}

#[test]
fn a_signal_in_an_unnamed_generate_block_can_be_traced() {
    let f = fixture("paths");
    let drivers = f.answer(DRIVERS, "top.genblk3.first_if");
    assert_eq!(drivers[0]["path"], "top.d");
}

#[test]
fn a_package_signal_is_traced_from_where_it_is_read() {
    let f = fixture("paths");
    let loads = f.answer(common::LOADS, "pkg::pkg_sig");
    assert_eq!(loads[0]["path"], "top.pkg_copy");
}

#[test]
fn only_signals_are_listed() {
    // No parameters, genvars, instances or enum values.
    let signals = signals("paths");
    assert!(!signals.iter().any(|path| path == "top.P" || path.ends_with(".i") || path == "top.u_desc[0]"));
}

#[test]
fn an_interface_port_names_the_signals_of_its_modport() {
    let f = fixture("cone_test");
    // `qux_in` takes the modport quxIn, which has qux and quz; the signal itself is the interface's.
    for name in ["qux", "quz"] {
        let through_port = f.answer(DRIVERS, &format!("test.the_sub_1.qux_in.{name}"));
        let direct = f.answer(DRIVERS, &format!("test.the_intfs[0].{name}"));
        assert_eq!(through_port, direct);
    }
}

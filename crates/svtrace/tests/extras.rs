//! Constructs the fork's own tests don't cover (extras.sv). Each expectation was also checked
//! against the patched slang-server's answer for the same design.

mod common;

use common::{fixture, DRIVERS, LOADS};

#[test]
fn a_copy_between_named_types_of_the_same_width_is_a_wire() {
    let f = fixture("extras");
    // byte_t is a packed struct of 8 bits, mode_e an enum over logic [2:0].
    f.check_sites(DRIVERS, "top.plain", "top.packed_a", &[(f.after("assign plain = "), "wire")]);
    f.check_sites(DRIVERS, "top.packed_b", "top.plain", &[(f.after("assign packed_b = "), "wire")]);
    f.check_sites(DRIVERS, "top.mode_bits", "top.mode", &[(f.after("assign mode_bits = "), "wire")]);
    f.check_sites(DRIVERS, "top.mode_q", "top.mode", &[(f.after("assign mode_q = "), "wire")]);
}

#[test]
fn a_change_of_width_or_a_cast_is_logic() {
    let f = fixture("extras");
    f.check_sites(DRIVERS, "top.narrowed", "top.narrowed_src", &[(f.after("assign narrowed = "), "data")]);
    f.check_sites(DRIVERS, "top.widened", "top.narrowed", &[(f.after("assign widened = "), "data")]);
    f.check_sites(DRIVERS, "top.cast_dst", "top.cast_src", &[(f.after("assign cast_dst = 8'("), "data")]);
}

#[test]
fn a_change_of_signedness_alone_is_still_a_wire() {
    let f = fixture("extras");
    f.check_sites(DRIVERS, "top.signed_dst", "top.cast_src", &[(f.after("assign signed_dst = "), "wire")]);
}

#[test]
fn fields_assigned_one_by_one_are_drivers_of_the_struct() {
    let f = fixture("extras");
    let hi = f.after("packed_a.hi = ");
    let lo = f.after("packed_a.lo = ");
    f.check_sites(DRIVERS, "top.packed_a", "top.raw", &[(hi, "data"), (lo, "data")]);
}

#[test]
fn operators_read_every_operand() {
    let f = fixture("extras");
    let sites = |path: &str, endpoint: &str, needle: &str| f.check_sites(DRIVERS, path, endpoint, &[(f.after(needle), "data")]);
    sites("top.tern_out", "top.tern_sel", "assign tern_out = ");
    sites("top.tern_out", "top.tern_a", "assign tern_out = tern_sel ? ");
    sites("top.tern_out", "top.tern_b", "assign tern_out = tern_sel ? tern_a : ");
    sites("top.repl", "top.raw", "assign repl = {2{");
}

#[test]
fn a_hierarchical_reference_reaches_into_the_instance() {
    let f = fixture("extras");
    f.check_sites(DRIVERS, "top.seen", "top.u_leaf.out", &[(f.after("assign seen = "), "data")]);
    f.check_sites(LOADS, "top.u_leaf.out", "top.seen", &[(f.after("assign seen = "), "data")]);
}

#[test]
fn a_loop_variable_is_an_endpoint_but_not_a_signal_to_start_from() {
    let f = fixture("extras");
    let read = f.after("looped[i] = loop_src[7 - ");
    // The loop's condition and the index of the target are control sources, the read in the value is data.
    let stop = f.after("for (int i = 0; ");
    let index = f.after("looped[");
    f.check_sites(DRIVERS, "top.looped", "top.i", &[(stop, "control"), (index, "control"), (read, "data")]);
    assert!(f.graph.signals().all(|path| path != "top.i"));
}

#[test]
fn a_reset_makes_a_control_and_a_constant() {
    let f = fixture("extras");
    // (Read in the event list as well: a clock site, then the control one.)
    let reset = [(f.after("negedge "), "clock"), (f.after("if (!"), "control")];
    f.check_sites(DRIVERS, "top.result", "top.rst_n_local", &reset);
    f.check_sites(DRIVERS, "top.result", "top.clk", &[(f.after("always_ff @(posedge "), "clock")]);
    f.check_sites(DRIVERS, "top.result", "top.raw", &[(f.after("else result <= "), "data")]);
    f.check_constants("top.result", &[(f.after("if (!rst_n_local) result <= "), "constant")]);
}

#[test]
fn enum_case_items_and_selectors_control_the_assignments_below() {
    let f = fixture("extras");
    f.check_sites(DRIVERS, "top.decoded", "top.mode", &[(f.after("case ("), "control")]);
    f.check_sites(DRIVERS, "top.decoded", "defs::mode_e.A", &[(f.before("A: decoded"), "control")]);
    f.check_sites(DRIVERS, "top.decoded", "top.tern_sel", &[(f.after("B: if ("), "control")]);
    // The `{3'b0, red}` branch reads a signal, so it isn't a constant.
    f.check_constants(
        "top.decoded",
        &[
            (f.after("decoded = "), "constant"),
            (f.after("A: decoded = "), "constant"),
            (f.after("B: if (tern_sel) decoded = "), "constant"),
            (f.after("default: decoded = "), "constant"),
        ],
    );
}

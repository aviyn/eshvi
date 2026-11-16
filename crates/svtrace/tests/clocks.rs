//! The `clock` site kind: a signal read in a block's event control is a clock of everything the block
//! assigns (clocks.sv), in the drivers and loads commands alike.

mod common;

use common::{fixture, DRIVERS, LOADS};

#[test]
fn a_clock_is_a_driver_of_what_its_block_assigns() {
    let f = fixture("clocks");
    let clk = f.after("always_ff @(posedge ");
    f.check_sites(DRIVERS, "clk_top.u_mixed.reg_q", "clk_top.u_mixed.clk", &[(clk, "clock")]);
    f.check_sites(DRIVERS, "clk_top.u_second.q", "clk_top.u_second.clk", &[(f.after("module stage(input logic clk, input logic d, output logic q);\n    always_ff @(posedge "), "clock")]);
}

#[test]
fn a_reset_in_the_event_list_and_the_if_is_both_a_clock_and_a_control() {
    let f = fixture("clocks");
    let reset = [(f.after("negedge "), "clock"), (f.after("if (!"), "control")];
    f.check_sites(DRIVERS, "clk_top.u_mixed.reg_q", "clk_top.u_mixed.rst_n", &reset);
    f.check_sites(DRIVERS, "clk_top.u_mixed.reg_q", "clk_top.u_mixed.en", &[(f.after("else if ("), "control")]);
    f.check_sites(DRIVERS, "clk_top.u_mixed.reg_q", "clk_top.u_mixed.a", &[(f.after("else if (en) reg_q <= "), "data")]);
}

#[test]
fn an_unclocked_block_has_no_clock() {
    let f = fixture("clocks");
    f.check_cone(DRIVERS, "clk_top.u_mixed.comb_o", &[("clk_top.u_mixed.a", f.after("input logic en,\n    input logic [3:0] "))]);
}

#[test]
fn the_loads_of_a_clock_are_the_signals_its_block_assigns_and_no_others() {
    let f = fixture("clocks");
    let clk = f.after("always_ff @(posedge ");
    f.check_sites(LOADS, "clk_top.u_mixed.clk", "clk_top.u_mixed.reg_q", &[(clk, "clock")]);
    // (The unclocked block that follows in the module, and the other signals, aren't loads of it.)
    let loads = f.answer(LOADS, "clk_top.u_mixed.clk");
    assert_eq!(loads.as_array().unwrap().len(), 1, "{loads}");
    // The reset is read in the event list and in the `if`: a clock site and a control site.
    f.check_sites(LOADS, "clk_top.u_mixed.rst_n", "clk_top.u_mixed.reg_q", &[(f.after("negedge "), "clock"), (f.after("if (!"), "control")]);
}

#[test]
fn a_clock_passed_down_is_a_port_then_a_clock() {
    let f = fixture("clocks");
    let paths: Vec<String> = f.answer(LOADS, "clk_top.clk").as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths, ["clk_top.u_mixed.clk", "clk_top.u_first.clk", "clk_top.u_second.clk"]);
    f.check_sites(LOADS, "clk_top.clk", "clk_top.u_first.clk", &[(f.after("stage u_first(."), "port")]);
}

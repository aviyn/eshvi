//! The sections of the fork's ConeTests.cpp (`Cone Tracing`, `Cone sites show where signals
//! connect` and the two small cases between them), with the same expectations. The call-hierarchy
//! sections ("Prepare ...") are not ported: svtrace has no call hierarchy.

mod common;

use common::{fixture, CONSTANTS, DRIVERS, LOADS};

// ---- Cone Tracing (cone_test.sv) ----

#[test]
fn drivers_multiple() {
    let f = fixture("cone_test");
    let a = f.after("module sub(").after("input logic [31:0] ");
    let b = a.after("input logic [31:0] ");
    // (The clock is a driver of what its block assigns: the fork's tracer has no such kind.)
    let clk = b.after("output logic [31:0] x,").after("input logic ");
    f.check_cone(DRIVERS, "test.the_sub_2.x", &[("test.the_sub_2.a", a), ("test.the_sub_2.b", b), ("test.the_sub_2.clk", clk)]);
    f.check_sites(DRIVERS, "test.the_sub_2.x", "test.the_sub_2.clk", &[(f.after("always_ff @(posedge "), "clock")]);
}

#[test]
fn drivers_single() {
    let f = fixture("cone_test");
    let x1 = f.before("x1;");
    f.check_cone(DRIVERS, "test.the_sub_2.b", &[("test.x1", x1)]);
}

#[test]
fn drivers_single_through_an_output_port() {
    // Points at the port declaration, not the port map.
    let f = fixture("cone_test");
    let x = f.after("module sub").after("output logic [31:0] ");
    f.check_cone(DRIVERS, "test.x1", &[("test.the_sub_1.x", x)]);
}

#[test]
fn drivers_constant() {
    let f = fixture("cone_test");
    let foo = f.after("module sub_sub(").after("input logic ");
    let bar = foo.after("input logic ");
    f.check_cone(
        DRIVERS,
        "test.the_sub_2.the_sub_sub.result",
        &[
            ("test.the_sub_2.the_sub_sub.foo", foo),
            ("test.the_sub_2.the_sub_sub.bar", bar),
            ("test.the_sub_2.the_sub_sub.clk", bar.after("input logic ")),
        ],
    );
}

#[test]
fn drivers_switched() {
    let f = fixture("cone_test");
    let foo = f.after("module sub_sub(").after("input logic ");
    let bar = foo.after("input logic ");
    f.check_cone(
        DRIVERS,
        "test.the_sub_2.the_sub_sub.switched_result",
        &[("test.the_sub_2.the_sub_sub.bar", bar), ("test.the_sub_2.the_sub_sub.foo", foo)],
    );
}

#[test]
fn drivers_interface() {
    let f = fixture("cone_test");
    let qux = f.before("qux;");
    let b = f.after("module sub(").after("input logic [31:0] ").after("input logic [31:0] ");
    f.check_cone(DRIVERS, "test.the_intfs[2].qux", &[("test.the_intfs[1].qux", qux), ("test.the_sub_2.b", b)]);
}

#[test]
fn drivers_interface_reference() {
    // The path goes through the sub's interface port and modport.
    let f = fixture("cone_test");
    let qux = f.before("qux;");
    let b = f.after("module sub(").after("input logic [31:0] ").after("input logic [31:0] ");
    f.check_cone(DRIVERS, "test.the_sub_1.qux_out.qux", &[("test.the_intfs[0].qux", qux), ("test.the_sub_1.b", b)]);
}

#[test]
fn parameters_are_not_drivers() {
    let f = fixture("cone_test");
    f.check_cone(DRIVERS, "test.parameter_driven", &[]);
}

#[test]
fn loads_multiple() {
    let f = fixture("cone_test");
    let a = f.after("module sub(").after("input logic [31:0] ");
    f.check_cone(LOADS, "test.a", &[("test.the_sub_2.a", a), ("test.the_sub_1.a", a)]);
}

#[test]
fn loads_up_down() {
    let f = fixture("cone_test");
    let x = f.after("module sub").after("output logic [31:0] ");
    let foo = f.after("module sub_sub(").after("input logic ");
    f.check_cone(LOADS, "test.the_sub_2.a", &[("test.the_sub_2.x", x), ("test.the_sub_2.the_sub_sub.foo", foo)]);
}

#[test]
fn loads_single() {
    let f = fixture("cone_test");
    let x = f.after("module test(").after("output logic [31:0] ");
    f.check_cone(LOADS, "test.the_sub_2.x", &[("test.x", x)]);
}

#[test]
fn loads_conditional() {
    let f = fixture("cone_test");
    let result = f.before("result;");
    let switched = f.before("switched_result;");
    f.check_cone(
        LOADS,
        "test.the_sub_1.the_sub_sub.foo",
        &[("test.the_sub_1.the_sub_sub.result", result), ("test.the_sub_1.the_sub_sub.switched_result", switched)],
    );
}

#[test]
fn loads_switched() {
    let f = fixture("cone_test");
    let result = f.before("result;");
    let switched = f.before("switched_result;");
    f.check_cone(
        LOADS,
        "test.the_sub_2.the_sub_sub.bar",
        &[("test.the_sub_2.the_sub_sub.result", result), ("test.the_sub_2.the_sub_sub.switched_result", switched)],
    );
}

#[test]
fn loads_through_instance_with_unconnected_input() {
    let f = fixture("cone_test");
    let input = f.after("module open_ports(").after("input logic ");
    f.check_cone(LOADS, "test.open_src", &[("test.the_open.in", input)]);
}

#[test]
fn loads_of_unconnected_output() {
    let f = fixture("cone_test");
    f.check_cone(LOADS, "test.the_open.out", &[]);
}

#[test]
fn loads_interface() {
    let f = fixture("cone_test");
    let quz = f.before("quz;");
    f.check_cone(LOADS, "test.the_intfs[1].quz", &[("test.the_intfs[0].quz", quz)]);
}

#[test]
fn loads_interface_reference() {
    let f = fixture("cone_test");
    let qux = f.before("qux;");
    f.check_cone(LOADS, "test.the_sub_1.qux_out.qux", &[("test.the_intfs[2].qux", qux)]);
}

#[test]
fn cone_locations_deduplicate_generated_multi_bit_drivers() {
    let f = fixture("cone_test");
    let declaration = f.before("bit_driver;");
    f.check_cone(DRIVERS, "test.bit_driven", &[("test.bit_driver", declaration)]);
}

#[test]
fn undriven_signals_have_empty_driver_cones() {
    let f = fixture("cone_undriven");
    f.check_cone(DRIVERS, "top.undriven", &[]);
    f.check_cone(DRIVERS, "top.unused", &[]);
}

// ---- Cone sites show where signals connect (cone_sites.sv) ----

#[test]
fn site_assignment() {
    let f = fixture("cone_sites");
    let read = f.before("data_src + ");
    f.check_sites(DRIVERS, "top.data_dst", "top.data_src", &[(read, "data")]);
    f.check_sites(LOADS, "top.data_src", "top.data_dst", &[(read, "data")]);
}

#[test]
fn site_a_plain_copy_is_a_wire() {
    let f = fixture("cone_sites");
    let assigned = f.before("wire_src;");
    f.check_sites(DRIVERS, "top.wire_dst", "top.wire_src", &[(assigned, "wire")]);
    f.check_sites(LOADS, "top.wire_src", "top.wire_dst", &[(assigned, "wire")]);
    let comb = f.before("comb_src;");
    f.check_sites(DRIVERS, "top.comb_dst", "top.comb_src", &[(comb, "wire")]);
    f.check_sites(LOADS, "top.comb_src", "top.comb_dst", &[(comb, "wire")]);
}

#[test]
fn site_a_copy_that_is_logic_is_not_a_wire() {
    let f = fixture("cone_sites");
    let flop = f.before("flop_d;");
    f.check_sites(DRIVERS, "top.flop_q", "top.flop_d", &[(flop, "data")]);
    f.check_sites(LOADS, "top.flop_d", "top.flop_q", &[(flop, "data")]);
    let gated = f.before("cond_src;");
    f.check_sites(DRIVERS, "top.cond_dst", "top.cond_src", &[(gated, "data")]);
    f.check_sites(LOADS, "top.cond_src", "top.cond_dst", &[(gated, "data")]);
    let slice = f.before("whole_a[1:0]");
    f.check_sites(DRIVERS, "top.sliced", "top.whole_a", &[(slice, "data")]);
    let narrowed = f.after("assign shrunk = ");
    f.check_sites(DRIVERS, "top.shrunk", "top.whole_b", &[(narrowed, "data")]);
}

#[test]
fn site_every_assignment_is_a_site() {
    let f = fixture("cone_sites");
    let first = f.before("twice_src;");
    let second = first.after("twice_src;").before("twice_src;");
    f.check_sites(DRIVERS, "top.twice_dst", "top.twice_src", &[(first, "data"), (second, "data")]);
    f.check_sites(LOADS, "top.twice_src", "top.twice_dst", &[(first, "data"), (second, "data")]);
}

#[test]
fn site_if_condition() {
    let f = fixture("cone_sites");
    let read = f.before("en) gated");
    f.check_sites(DRIVERS, "top.gated", "top.en", &[(read, "control")]);
    f.check_sites(LOADS, "top.en", "top.gated", &[(read, "control")]);
}

#[test]
fn site_assignment_under_a_condition() {
    let f = fixture("cone_sites");
    let read = f.after("if (en) gated = ");
    f.check_sites(DRIVERS, "top.gated", "top.gated_d", &[(read, "data")]);
}

#[test]
fn site_case_selector() {
    let f = fixture("cone_sites");
    let read = f.before("sel)");
    f.check_sites(DRIVERS, "top.cased", "top.sel", &[(read, "control")]);
    f.check_sites(LOADS, "top.sel", "top.cased", &[(read, "control")]);
}

#[test]
fn site_named_input_port() {
    let f = fixture("cone_sites");
    let read = f.before("named_src)");
    f.check_sites(DRIVERS, "top.named.in", "top.named_src", &[(read, "port")]);
    f.check_sites(LOADS, "top.named_src", "top.named.in", &[(read, "port")]);
}

#[test]
fn site_named_output_port() {
    let f = fixture("cone_sites");
    let read = f.before("named_dst)");
    f.check_sites(DRIVERS, "top.named_dst", "top.named.out", &[(read, "port")]);
    f.check_sites(LOADS, "top.named.out", "top.named_dst", &[(read, "port")]);
}

#[test]
fn site_implicit_ports() {
    let f = fixture("cone_sites");
    let input = f.before("in, .out)");
    let output = f.after("implicit(.in, .");
    f.check_sites(DRIVERS, "top.implicit.in", "top.in", &[(input, "port")]);
    f.check_sites(DRIVERS, "top.out", "top.implicit.out", &[(output, "port")]);
}

#[test]
fn site_wildcard_ports() {
    let f = fixture("cone_sites");
    let wildcard = f.before(".*)");
    f.check_sites(DRIVERS, "top.wild.wild_in", "top.wild_in", &[(wildcard, "port")]);
    f.check_sites(LOADS, "top.wild.wild_out", "top.wild_out", &[(wildcard, "port")]);
}

#[test]
fn site_one_signal_of_a_concatenated_output() {
    let f = fixture("cone_sites");
    let read = f.before("low})");
    f.check_sites(DRIVERS, "top.low", "top.concat.pair", &[(read, "port")]);
    f.check_sites(LOADS, "top.concat.pair", "top.low", &[(read, "port")]);
}

#[test]
fn site_macro_argument() {
    let f = fixture("cone_sites");
    let read = f.before("macro_src);");
    f.check_sites(DRIVERS, "top.macro_dst", "top.macro_src", &[(read, "wire")]);
}

#[test]
fn site_macro_body() {
    let f = fixture("cone_sites");
    let usage = f.before("`ASSIGN_FIXED(fixed_dst)");
    f.check_sites(DRIVERS, "top.fixed_dst", "top.fixed_src", &[(usage, "wire")]);
}

#[test]
fn constant_assignments() {
    let f = fixture("cone_sites");
    f.check_constants("top.lit", &[(f.after("assign lit = "), "constant")]);
    f.check_constants("top.param_dst", &[(f.after("assign param_dst = "), "constant")]);
    f.check_constants("top.state", &[(f.after("assign state = "), "constant")]);
}

#[test]
fn constant_assignments_under_conditions() {
    let f = fixture("cone_sites");
    f.check_constants("top.reset_flop", &[(f.after("if (en) reset_flop <= "), "constant")]);
    let one = f.after("1'b0: cased = ");
    let other = f.after("default: cased = ");
    f.check_constants("top.cased", &[(one, "constant"), (other, "constant")]);
}

#[test]
fn a_constant_tied_to_an_input_port() {
    let f = fixture("cone_sites");
    f.check_constants("top.concat.in", &[(f.before("1'b0), .pair"), "constant")]);
}

#[test]
fn not_constant_assignments() {
    // A signal is read, an output port drives it, or nothing does.
    let f = fixture("cone_sites");
    for path in ["top.data_dst", "top.wire_dst", "top.named_dst", "top.named.in", "top.rd_only"] {
        f.check_constants(path, &[]);
    }
}

#[test]
fn a_compound_assignment_reads_its_target() {
    let f = fixture("cone_sites");
    f.check_constants("top.counted", &[(f.after("counted = "), "constant")]);
}

#[test]
fn a_call_that_is_given_no_signal() {
    let f = fixture("cone_sites");
    f.check_constants("top.from_call", &[(f.after("assign from_call = "), "call")]);
}

#[test]
fn a_default_assignment_that_is_replaced_is_not_a_wire() {
    let f = fixture("cone_sites");
    let read = f.after("dflt_dst = ").before("dflt_src;");
    f.check_sites(DRIVERS, "top.dflt_dst", "top.dflt_src", &[(read, "data")]);
    f.check_sites(LOADS, "top.dflt_src", "top.dflt_dst", &[(read, "data")]);
}

#[test]
fn always_star_is_combinational() {
    let f = fixture("cone_sites");
    let read = f.before("star_src;");
    f.check_sites(DRIVERS, "top.star_dst", "top.star_src", &[(read, "wire")]);
}

#[test]
fn an_index_on_the_left_is_not_written() {
    let f = fixture("cone_sites");
    f.check_constants("top.idx", &[]);
    let read = f.before("idx_src;");
    f.check_sites(DRIVERS, "top.idx", "top.idx_src", &[(read, "wire")]);
    f.check_constants("top.mem", &[(f.after("mem[idx] = "), "constant")]);
    let target = f.before("mem2, bus");
    f.check_cone(LOADS, "top.mem_d", &[("top.mem2", target)]);
}

#[test]
fn logic_in_a_port_map_is_not_wiring() {
    let f = fixture("cone_sites");
    let en = f.before("en & sel)");
    let sel = f.before("sel), .out");
    f.check_sites(DRIVERS, "top.logic_port.in", "top.en", &[(en, "data")]);
    f.check_sites(LOADS, "top.sel", "top.logic_port.in", &[(sel, "data")]);
    let picked = f.before("bus[sel])");
    f.check_sites(DRIVERS, "top.picked_port.in", "top.bus", &[(picked, "data")]);
}

#[test]
fn bits_routed_by_a_port_map_are_wiring() {
    let f = fixture("cone_sites");
    let bit = f.before("bus[0])");
    f.check_sites(DRIVERS, "top.bit_port.in", "top.bus", &[(bit, "port")]);
    let bits = f.before("bus[1:1]");
    f.check_sites(DRIVERS, "top.padded_port.in", "top.bus", &[(bits, "port")]);
}

#[test]
fn an_enum_value_is_not_a_wire() {
    let f = fixture("cone_sites");
    let member = f.after("assign state = ");
    f.check_sites(DRIVERS, "top.state", "top.state_e.IDLE", &[(member, "data")]);
}

// ---- The commands' errors, as the fork's server words them ----

#[test]
fn errors_name_the_path() {
    let f = fixture("cone_test");
    assert_eq!(f.error(DRIVERS, "test.nope"), "Could not find path in compiled design: test.nope");
    assert_eq!(f.error(LOADS, "test.nope"), "Could not find path in compiled design: test.nope");
    // An instance is in the design but is not a signal.
    assert_eq!(f.error(DRIVERS, "test.the_sub_1"), "Path is not a value symbol: test.the_sub_1");
    assert_eq!(f.error(LOADS, "test.the_sub_1"), "Could not find reference to: test.the_sub_1");
    assert_eq!(f.error(CONSTANTS, "test.nope"), "Could not find path in compiled design: test.nope");
    // A signal that nothing mentions.
    let f = fixture("cone_undriven");
    assert_eq!(f.error(LOADS, "top.unused"), "Could not find reference to: top.unused");
}

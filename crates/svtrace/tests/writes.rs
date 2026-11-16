//! What counts as a write, a read and a condition (writes.sv): reads outside an assignment, loops,
//! declaration initializers, `++` and `+=`, left-side indices and the items of a `case`. Drivers and
//! loads follow the same rules (tests/symmetry.rs checks that for every signal).

mod common;

use std::collections::BTreeMap;

use common::{fixture, Fixture, DRIVERS, LOADS};

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
fn a_read_outside_an_assignment_is_a_load_of_nothing() {
    let f = fixture("writes");
    // assert (rd_a == 1'b0); $display("%d", rd_a); rd_y = rd_b;
    expect(&f, LOADS, "top.rd_a", &[]);
    expect(&f, DRIVERS, "top.rd_y", &[("rd_b", &["wire"])]);
    expect(&f, LOADS, "top.rd_b", &[("rd_y", &["wire"])]);
}

#[test]
fn a_loop_condition_controls_what_the_loop_assigns_and_nothing_after_it() {
    let f = fixture("writes");
    // for (int i = 0; i < ln; i++) lp_z = lp_z ^ lp_b;  lp_t = lp_a;
    let drivers = cone(&f, DRIVERS, "top.lp_z");
    assert_eq!(drivers["top.ln"], ["control"]);
    assert_eq!(drivers["top.lp_b"], ["data"]);
    assert!(!drivers.contains_key("top.lp_a"), "{drivers:?}");
    f.check_sites(DRIVERS, "top.lp_z", "top.ln", &[(f.after("i < "), "control")]);
    // (The step `i++` runs under the condition too.)
    expect(&f, LOADS, "top.ln", &[("i", &["control"]), ("lp_z", &["control"])]);
    f.check_sites(LOADS, "top.ln", "top.lp_z", &[(f.after("i < "), "control")]);
    expect(&f, LOADS, "top.lp_a", &[("lp_t", &["wire"])]);
    expect(&f, DRIVERS, "top.lp_t", &[("lp_a", &["wire"])]);
}

#[test]
fn while_and_repeat_conditions_are_control_too() {
    let f = fixture("writes");
    expect(&f, DRIVERS, "top.wl_y", &[("wl_c", &["control"]), ("wl_b", &["data"])]);
    expect(&f, LOADS, "top.wl_c", &[("wl_y", &["control"])]);
    expect(&f, DRIVERS, "top.rp_y", &[("rp_n", &["control"]), ("rp_b", &["data"])]);
    expect(&f, LOADS, "top.rp_n", &[("rp_y", &["control"])]);
}

#[test]
fn an_assertions_condition_controls_its_action_blocks() {
    let f = fixture("writes");
    expect(&f, DRIVERS, "top.aa_y", &[("aa_c", &["control"]), ("aa_b", &["data"]), ("aa_d", &["data"])]);
    expect(&f, LOADS, "top.aa_c", &[("aa_y", &["control"])]);
}

#[test]
fn a_net_initializer_is_a_continuous_assignment() {
    let f = fixture("writes");
    // wire w_net = in_a;  wire w_expr = in_a & in_b;
    f.check_sites(DRIVERS, "top.w_net", "top.in_a", &[(f.after("wire w_net = "), "wire")]);
    f.check_sites(LOADS, "top.in_a", "top.w_net", &[(f.after("wire w_net = "), "wire")]);
    expect(&f, DRIVERS, "top.w_expr", &[("in_a", &["data"]), ("in_b", &["data"])]);
    assert_eq!(cone(&f, LOADS, "top.in_a")["top.w_expr"], ["data"]);
}

#[test]
fn a_variable_initializer_is_a_first_value_not_a_wire() {
    let f = fixture("writes");
    // logic v_init = in_b;  logic v_const = 1'b1;
    f.check_sites(DRIVERS, "top.v_init", "top.in_b", &[(f.after("logic v_init = "), "data")]);
    assert_eq!(cone(&f, LOADS, "top.in_b")["top.v_init"], ["data"]);
    expect(&f, DRIVERS, "top.v_const", &[]);
    f.check_constants("top.v_const", &[(f.after("logic v_const = "), "constant")]);
}

#[test]
fn a_local_declaration_assignment_is_in_the_graph() {
    let f = fixture("writes");
    // automatic logic loc = in_a;  loc_y = loc;
    f.check_sites(DRIVERS, "top.loc", "top.in_a", &[(f.after("automatic logic loc = "), "data")]);
    expect(&f, DRIVERS, "top.loc_y", &[("loc", &["wire"])]);
}

#[test]
fn increments_and_compound_assignments_write_and_read_their_operand() {
    let f = fixture("writes");
    // cnt++; --dn; acc += inc_in;
    expect(&f, DRIVERS, "top.cnt", &[("clk", &["clock"]), ("cnt", &["data"])]);
    expect(&f, LOADS, "top.cnt", &[("cnt", &["data"])]);
    f.check_sites(DRIVERS, "top.cnt", "top.cnt", &[(f.before("cnt++"), "data")]);
    expect(&f, DRIVERS, "top.dn", &[("clk", &["clock"]), ("dn", &["data"])]);
    expect(&f, DRIVERS, "top.acc", &[("clk", &["clock"]), ("inc_in", &["data"]), ("acc", &["data"])]);
    expect(&f, LOADS, "top.inc_in", &[("acc", &["data"])]);
    assert_eq!(cone(&f, LOADS, "top.clk")["top.cnt"], ["clock"]);
}

#[test]
fn a_left_side_index_is_a_control_source_of_the_target() {
    let f = fixture("writes");
    // mem[waddr] <= wdata;  assign rd_mem = mem[raddr];
    expect(&f, DRIVERS, "top.mem", &[("clk", &["clock"]), ("wdata", &["data"]), ("waddr", &["control"])]);
    f.check_sites(DRIVERS, "top.mem", "top.waddr", &[(f.after("mem[") , "control")]);
    expect(&f, LOADS, "top.waddr", &[("mem", &["control"])]);
    expect(&f, DRIVERS, "top.rd_mem", &[("mem", &["data"]), ("raddr", &["data"])]);
}

#[test]
fn an_index_in_a_concatenated_target_goes_with_its_own_part() {
    let f = fixture("writes");
    // {sa[i1], sb[i2]} <= pair;
    expect(&f, DRIVERS, "top.sa", &[("clk", &["clock"]), ("pair", &["data"]), ("i1", &["control"])]);
    expect(&f, DRIVERS, "top.sb", &[("clk", &["clock"]), ("pair", &["data"]), ("i2", &["control"])]);
    expect(&f, LOADS, "top.i1", &[("sa", &["control"])]);
    expect(&f, LOADS, "top.i2", &[("sb", &["control"])]);
}

#[test]
fn the_index_of_a_target_works_for_a_selected_path_too() {
    let f = fixture("writes");
    let entries = f.step("drivers", "top.mem[3:0]", serde_json::json!({}));
    assert!(common::paths(&entries).contains(&"top.waddr"), "{:?}", common::paths(&entries));
    let entries = f.step("loads", "top.waddr", serde_json::json!({}));
    assert!(common::paths(&entries).contains(&"top.mem"), "{:?}", common::paths(&entries));
    let entries = f.step("loads", "top.waddr[0]", serde_json::json!({}));
    assert!(common::paths(&entries).contains(&"top.mem"), "{:?}", common::paths(&entries));
}

#[test]
fn case_items_gate_the_items_after_them_in_both_directions() {
    let f = fixture("writes");
    // case (sel) {1'b0, k1}: cy = ca;  {1'b1, k2}: cz = cb;
    expect(&f, DRIVERS, "top.cy", &[("sel", &["control"]), ("k1", &["control"]), ("ca", &["data"])]);
    expect(&f, DRIVERS, "top.cz", &[("sel", &["control"]), ("k1", &["control"]), ("k2", &["control"]), ("cb", &["data"])]);
    expect(&f, LOADS, "top.k1", &[("cy", &["control"]), ("cz", &["control"])]);
    expect(&f, LOADS, "top.k2", &[("cz", &["control"])]);
    expect(&f, LOADS, "top.sel", &[("cy", &["control"]), ("cz", &["control"])]);
}

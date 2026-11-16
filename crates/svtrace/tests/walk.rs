//! `svtrace.step` and `svtrace.endpoints` on the fixtures clocks.sv and steps.sv.

mod common;

use common::{entry, fixture, kinds, paths};
use serde_json::{json, Value};

/// How deep the `next` chain of an entry goes (the entry itself is level 1).
fn depth(entry: &Value) -> usize {
    1 + entry["next"].as_array().map_or(0, |next| next.first().map_or(0, depth))
}

// ---- svtrace.step ----

#[test]
fn a_step_is_sorted_by_path_and_has_each_path_once() {
    let f = fixture("clocks");
    let entries = f.step("drivers", "clk_top.u_mixed.reg_q", json!({ "follow": false }));
    assert_eq!(paths(&entries), ["clk_top.u_mixed.a", "clk_top.u_mixed.clk", "clk_top.u_mixed.en", "clk_top.u_mixed.rst_n"]);
    // The sites are the ones the phase 1 commands give.
    let drivers = f.answer(common::DRIVERS, "clk_top.u_mixed.reg_q");
    for e in &entries {
        let same = drivers.as_array().unwrap().iter().find(|d| d["path"] == e["path"]).unwrap();
        assert_eq!(e["sites"], same["sites"]);
        assert_eq!(e["location"], same["location"]);
    }
    assert_eq!(kinds(entry(&entries, "clk_top.u_mixed.rst_n")), ["clock", "control"]);
    assert!(entries.iter().all(|e| e.get("next").is_none()));
}

#[test]
fn each_entry_counts_its_own_next_step() {
    let f = fixture("steps");
    let entries = f.step("drivers", "steps_top.flop_q", json!({ "follow": false }));
    assert_eq!(paths(&entries), ["steps_top.clk", "steps_top.flop_d"]);
    // flop_d is driven by seed alone; clk by nothing.
    let flop_d = entry(&entries, "steps_top.flop_d");
    assert_eq!((flop_d["count"].as_u64(), flop_d["total"].as_u64()), (Some(1), Some(1)));
    let clk = entry(&entries, "steps_top.clk");
    assert_eq!((clk["count"].as_u64(), clk["total"].as_u64()), (Some(0), Some(0)));
    assert_eq!(clk["constant"], false);
    assert_eq!(clk["loop"], false);
}

#[test]
fn a_flops_clock_neither_breaks_a_chain_nor_counts() {
    let f = fixture("clocks");
    let top = f.step("drivers", "clk_top.dout", json!({}));
    let q = entry(&top, "clk_top.u_second.q");
    // q has two drivers, d and a clock: total 2, count 1.
    assert_eq!(kinds(q), ["port"]);
    assert_eq!((q["total"].as_u64(), q["count"].as_u64()), (Some(2), Some(1)));
    // So the chain goes on: q, then d (and the clock beside it), mid, the first flop's q, its d, din.
    let next = q["next"].as_array().unwrap();
    assert_eq!(paths(next), ["clk_top.u_second.clk", "clk_top.u_second.d"]);
    assert_eq!(kinds(&next[0]), ["clock"]);
    assert_eq!(kinds(&next[1]), ["data"]);
    // (The clock is followed too, being a single step to the top-level clock; it is only not counted.)
    let mid = &next[1]["next"][0];
    assert_eq!(mid["path"], "clk_top.mid");
    let first = &mid["next"][0];
    assert_eq!(first["path"], "clk_top.u_first.q");
    assert_eq!(paths(first["next"].as_array().unwrap()), ["clk_top.u_first.clk", "clk_top.u_first.d"]);
    assert_eq!(first["next"][1]["next"][0]["path"], "clk_top.din");
    assert!(first["next"][1]["next"][0].get("next").is_none());
}

#[test]
fn in_the_loads_direction_a_clock_site_is_an_ordinary_load() {
    let f = fixture("clocks");
    let loads = f.step("loads", "clk_top.u_mixed.clk", json!({ "follow": false }));
    assert_eq!(paths(&loads), ["clk_top.u_mixed.reg_q"]);
    assert_eq!(kinds(&loads[0]), ["clock"]);
    // reg_q is read by the port map that connects it out: one load, counted.
    assert_eq!((loads[0]["total"].as_u64(), loads[0]["count"].as_u64()), (Some(1), Some(1)));
    let top = f.step("loads", "clk_top.clk", json!({ "follow": false }));
    assert_eq!(paths(&top), ["clk_top.u_first.clk", "clk_top.u_mixed.clk", "clk_top.u_second.clk"]);
    assert!(top.iter().all(|e| e["count"] == 1 && e["total"] == 1));
}

#[test]
fn a_chain_of_single_steps_arrives_nested() {
    let f = fixture("steps");
    let entries = f.step("drivers", "steps_top.c5", json!({}));
    assert_eq!(paths(&entries), ["steps_top.c4"]);
    let mut at = &entries[0];
    for want in ["c3", "c2", "c1", "c0", "seed"] {
        at = &at["next"][0];
        assert_eq!(at["path"], format!("steps_top.{want}"));
    }
    // seed is a top-level input: nothing drives it.
    assert!(at.get("next").is_none());
    assert_eq!((at["total"].as_u64(), at["count"].as_u64()), (Some(0), Some(0)));
    assert_eq!(kinds(&entries[0]), ["wire"]);
}

#[test]
fn follow_false_stops_at_the_first_step() {
    let f = fixture("steps");
    let entries = f.step("drivers", "steps_top.c5", json!({ "follow": false }));
    assert_eq!(paths(&entries), ["steps_top.c4"]);
    assert!(entries[0].get("next").is_none());
    assert_eq!((entries[0]["total"].as_u64(), entries[0]["count"].as_u64()), (Some(1), Some(1)));
}

#[test]
fn a_loop_is_marked_and_not_followed() {
    let f = fixture("steps");
    let entries = f.step("drivers", "steps_top.la", json!({}));
    let lb = entry(&entries, "steps_top.lb");
    assert_eq!(lb["loop"], false);
    // lb is driven by la, which is the signal asked for.
    let la = &lb["next"][0];
    assert_eq!(la["path"], "steps_top.la");
    assert_eq!(la["loop"], true);
    assert!(la.get("next").is_none());
}

#[test]
fn the_ancestors_of_an_opened_row_are_loops_too() {
    let f = fixture("steps");
    let entries = f.step("drivers", "steps_top.c5", json!({ "ancestors": ["steps_top.c3"] }));
    let c3 = &entries[0]["next"][0];
    assert_eq!(c3["path"], "steps_top.c3");
    assert_eq!(c3["loop"], true);
    assert!(c3.get("next").is_none());
    // Without them it is not.
    let plain = f.step("drivers", "steps_top.c5", json!({ "ancestors": [] }));
    assert_eq!(plain[0]["next"][0]["loop"], false);
}

#[test]
fn constants_are_marked_by_the_graph_and_not_followed() {
    let f = fixture("steps");
    let st = f.step("drivers", "steps_top.st", json!({}));
    assert_eq!(paths(&st), ["kinds::state_e.BUSY"]);
    assert_eq!(st[0]["constant"], true);
    assert!(st[0].get("next").is_none());
    // A module-local enum's member, which the Lua's path pattern can't tell from a signal.
    let loc = f.step("drivers", "steps_top.loc", json!({}));
    assert_eq!(paths(&loc), ["steps_top.local_e.L_B"]);
    assert_eq!(loc[0]["constant"], true);
    // A parameter is no endpoint at all.
    let param = f.step("drivers", "steps_top.with_param", json!({}));
    assert_eq!(paths(&param), ["steps_top.seed"]);
    assert_eq!(f.step("drivers", "steps_top.with_literal", json!({})).len(), 0);
}

#[test]
fn many_signals_in_several_instances_are_not_followed() {
    let f = fixture("steps");
    let many = f.step("drivers", "steps_top.many", json!({}));
    assert_eq!(many.len(), 9);
    for e in &many {
        // Each is driven by one signal (its instance's input), yet none is followed.
        assert_eq!((e["total"].as_u64(), e["count"].as_u64()), (Some(1), Some(1)));
        assert!(e.get("next").is_none(), "{e}");
    }
}

#[test]
fn many_signals_in_one_instance_are_not_many() {
    let f = fixture("steps");
    let flat = f.step("drivers", "steps_top.flat", json!({}));
    assert_eq!(flat.len(), 9);
    for e in &flat {
        assert_eq!(e["next"][0]["path"], "steps_top.seed", "{e}");
    }
}

#[test]
fn eight_signals_in_several_instances_are_not_many() {
    let f = fixture("steps");
    let eight = f.step("drivers", "steps_top.eight", json!({}));
    assert_eq!(eight.len(), 8);
    for (i, e) in eight.iter().enumerate() {
        assert_eq!(e["next"][0]["path"], format!("steps_top.u{i}.i"), "{e}");
    }
}

#[test]
fn a_chain_is_followed_24_levels_below_the_asked_signal() {
    let f = fixture("steps");
    let entries = f.step("drivers", "steps_top.c26", json!({}));
    assert_eq!(depth(&entries[0]), 24);
    // The 24th entry (c2) is the last: it counts its step but does not carry it.
    let mut at = &entries[0];
    while at.get("next").is_some() {
        at = &at["next"][0];
    }
    assert_eq!(at["path"], "steps_top.c2");
    assert_eq!((at["total"].as_u64(), at["count"].as_u64()), (Some(1), Some(1)));
    // Asked for c1's row (opened by the user), the chain starts again from there.
    let again = f.step("drivers", "steps_top.c2", json!({}));
    assert_eq!(depth(&again[0]), 3);
}

#[test]
fn a_step_asks_no_question_limit() {
    // 27 signals deep and each entry's own step counted: the answer is whole, however many it takes.
    let f = fixture("steps");
    let entries = f.step("loads", "steps_top.seed", json!({}));
    assert!(entries.len() > 20, "{}", entries.len());
}

#[test]
fn step_errors_are_the_phase_1_commands() {
    let f = fixture("steps");
    assert_eq!(f.step_error(json!({ "path": "steps_top.nope", "direction": "drivers" })), "Could not find path in compiled design: steps_top.nope");
    assert_eq!(f.step_error(json!({ "path": "steps_top.nope", "direction": "loads" })), "Could not find path in compiled design: steps_top.nope");
    assert_eq!(f.step_error(json!({ "path": "steps_top", "direction": "drivers" })), "Path is not a value symbol: steps_top");
    assert!(f.step_error(json!({ "direction": "drivers" })).contains("takes { path, direction }"));
    assert!(f.step_error(json!({ "path": "steps_top.c1", "direction": "sideways" })).contains("direction"));
    assert!(f.step_error(json!({ "path": "steps_top.c1", "ancestors": "x" })).contains("ancestors"));
    assert!(f.step_error(json!({ "path": "steps_top.c1", "follow": 3 })).contains("follow"));
    let wrong = svtrace::trace::run(&f.graph, "svtrace.step", &[json!("steps_top.c1")]).unwrap_err();
    assert!(wrong.contains("takes { path, direction }"), "{wrong}");
    let two = svtrace::trace::run(&f.graph, "svtrace.step", &[json!({}), json!({})]).unwrap_err();
    assert_eq!(two, "Expected 0 or 1 argument for command");
}

// ---- svtrace.endpoints ----

fn at(cursor: common::Cursor) -> (u64, u64) {
    cursor.position()
}

#[test]
fn the_driver_endpoint_of_a_flop_chain_is_the_flop_not_the_wiring() {
    let f = fixture("clocks");
    // dout <- port <- u_second.q <- (d: data, on the line `q <= d`); the clock is no endpoint of drivers.
    let d = f.after("input logic d, output logic q);\n    always_ff @(posedge clk) q <= ");
    assert_eq!(f.endpoints("drivers", "clk_top.dout"), [at(d)]);
}

#[test]
fn a_reset_and_a_data_line_are_endpoints_of_a_clocked_block() {
    let f = fixture("clocks");
    // The data line, then the line that assigns the reset value (a constant); controls aren't driver endpoints.
    let data = f.after("else if (en) reg_q <= ");
    let reset = f.after("if (!rst_n) reg_q <= ");
    assert_eq!(f.endpoints("drivers", "clk_top.reg_q"), [at(data), at(reset)]);
}

#[test]
fn the_load_endpoints_of_a_clock_are_the_event_lists() {
    let f = fixture("clocks");
    let stage = f.after("input logic d, output logic q);\n    always_ff @(posedge ");
    let mixed = f.after("always_ff @(posedge ");
    assert_eq!(f.endpoints("loads", "clk_top.clk"), [at(stage), at(mixed)]);
}

#[test]
fn a_reset_has_clock_and_control_load_endpoints() {
    let f = fixture("clocks");
    assert_eq!(f.endpoints("loads", "clk_top.rst_n"), [at(f.after("negedge ")), at(f.after("if (!"))]);
}

#[test]
fn a_top_level_input_with_nothing_further_is_listed_at_its_declaration() {
    let f = fixture("steps");
    // c26 <- ... <- c0 <- seed: the wires are followed, and seed is an input of the top module.
    assert_eq!(f.endpoints("drivers", "steps_top.c26"), [at(f.after("module steps_top(input logic "))]);
    // The asked signal itself is not listed at its declaration.
    assert_eq!(f.endpoints("drivers", "steps_top.seed"), []);
}

#[test]
fn a_flops_data_is_an_endpoint_and_a_loop_ends() {
    let f = fixture("steps");
    assert_eq!(f.endpoints("drivers", "steps_top.flop_q"), [at(f.after("always_ff @(posedge clk) flop_q <= "))]);
    assert_eq!(f.endpoints("drivers", "steps_top.la"), []);
}

#[test]
fn constants_are_driver_endpoints() {
    let f = fixture("steps");
    assert_eq!(f.endpoints("drivers", "steps_top.st"), [at(f.after("assign st = "))]);
    assert_eq!(f.endpoints("drivers", "steps_top.loc"), [at(f.after("assign loc = "))]);
    assert_eq!(f.endpoints("drivers", "steps_top.with_literal"), [at(f.after("assign with_literal = "))]);
}

#[test]
fn the_load_endpoints_of_a_signal_are_one_per_line() {
    let f = fixture("steps");
    let mut got = f.endpoints("loads", "steps_top.seed");
    got.sort();
    let mut want = vec![
        // c0 ... c26 are wires, followed to c26, which nothing reads: its declaration.
        at(f.after("c25, ")),
        // flop_d is a wire too: the flop's data line stands for it.
        at(f.after("always_ff @(posedge clk) flop_q <= ")),
        at(f.after("assign with_param = PARAM & ")),
        // The instances' inputs are followed into `source`, whose outputs are read in two lines (once each).
        at(f.after("assign eight = ")),
        at(f.after("assign many = ")),
        at(f.after("= {9{")),
    ];
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn endpoints_errors() {
    let f = fixture("steps");
    let error = |argument: Value| svtrace::trace::run(&f.graph, "svtrace.endpoints", &[argument]).unwrap_err();
    assert_eq!(error(json!({ "path": "steps_top.nope", "direction": "loads" })), "Could not find path in compiled design: steps_top.nope");
    assert!(error(json!({ "direction": "loads" })).contains("takes { path, direction }"));
    assert!(error(json!({ "path": "steps_top.c1", "direction": 7 })).contains("direction"));
}

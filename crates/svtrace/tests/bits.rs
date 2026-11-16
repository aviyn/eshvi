//! Phase 3: selected paths (fields and constant selects), bits on connections, following only the
//! bits that overlap, and `svtrace.atCursor`, on the fixture bits.sv; CV32E40P's `mstatus_q.mie`
//! in an ignored test.

mod common;

use common::{entry, fixture, kinds, paths, Fixture};
use serde_json::{json, Value};
use svtrace::trace;

const TOP: &str = "bits_top";

fn p(name: &str) -> String {
    format!("{TOP}.{name}")
}

fn drivers(f: &Fixture, path: &str) -> Vec<Value> {
    f.step("drivers", &p(path), json!({ "follow": false }))
}

fn loads(f: &Fixture, path: &str) -> Vec<Value> {
    f.step("loads", &p(path), json!({ "follow": false }))
}

/// The paths of the entries, without the top module's name.
fn names(entries: &[Value]) -> Vec<String> {
    paths(entries).into_iter().map(|path| path.strip_prefix("bits_top.").unwrap_or(path).to_string()).collect()
}

/// (line, column) of each site, 0-based.
fn site_places(entry: &Value) -> Vec<(u64, u64)> {
    entry["sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|site| {
            let start = &site["location"]["range"]["start"];
            (start["line"].as_u64().unwrap(), start["character"].as_u64().unwrap())
        })
        .collect()
}

#[test]
fn a_part_select_takes_only_the_bits_it_writes() {
    let f = fixture("bits");
    let low = drivers(&f, "v[3:0]");
    assert_eq!(names(&low), ["a_in[7:4]"]);
    assert_eq!(low[0]["signal"], p("a_in"));
    assert_eq!(kinds(&low[0]), ["data"]);
    // The site is the signal's use in the source, as before.
    assert_eq!(site_places(&low[0]), [f.after("assign v[3:0] = ").position()]);

    // The bits [7:4] come from all of b_in (a 4-bit signal), so its path is the plain one.
    let high = drivers(&f, "v[7:4]");
    assert_eq!(names(&high), ["b_in"]);
    assert_eq!(high[0]["signal"], high[0]["path"]);

    // A single bit of the second part.
    assert_eq!(names(&drivers(&f, "v[5]")), ["b_in[1]"]);
    // Bits that straddle both parts take both.
    assert_eq!(names(&drivers(&f, "v[5:2]")), ["a_in[7:6]", "b_in[1:0]"]);
}

#[test]
fn a_whole_signal_stays_whole() {
    let f = fixture("bits");
    let entries = drivers(&f, "v");
    assert_eq!(names(&entries), ["a_in", "b_in"]);
    for e in &entries {
        assert_eq!(e["path"], e["signal"]);
    }
    // A select that covers the signal is the signal.
    assert_eq!(names(&drivers(&f, "v[7:0]")), ["a_in", "b_in"]);
    // The phase 1 command ignores bits and takes only whole signals.
    assert!(trace::execute(&f.graph, common::DRIVERS, &p("v[3:0]")).is_err());
}

#[test]
fn ascending_ranges_count_from_the_left() {
    let f = fixture("bits");
    assert_eq!(names(&drivers(&f, "up[0:3]")), ["a_in[3:0]"]);
    assert_eq!(names(&drivers(&f, "up[4]")), ["a_in[7]"]);
    assert_eq!(names(&drivers(&f, "up[5:7]")), ["b_in[2:0]"]);
    // up[0] is the leftmost bit, the most significant, which a_in[3] feeds.
    assert_eq!(names(&drivers(&f, "up[0]")), ["a_in[3]"]);
    // And the other way round.
    let from = loads(&f, "a_in[0]");
    assert!(names(&from).contains(&"up[3]".to_string()));
    assert!(!names(&from).contains(&"up[0:3]".to_string()));
}

#[test]
fn fields_are_found_by_name_and_named_back() {
    let f = fixture("bits");
    assert_eq!(names(&drivers(&f, "cfg.b.hi")), ["hi_in"]);
    assert_eq!(names(&drivers(&f, "cfg.mode")), ["mode_in"]);
    assert_eq!(names(&drivers(&f, "cfg.en")), ["en_in"]);
    assert_eq!(names(&drivers(&f, "cfg.b")), ["hi_in", "lo_in"]);
    // The whole struct has all four.
    assert_eq!(names(&drivers(&f, "cfg")), ["en_in", "hi_in", "lo_in", "mode_in"]);
    // What a flop copies field by field comes back as the field's name.
    let q = drivers(&f, "cfg_q.b");
    assert_eq!(names(&q), ["cfg_n.b", "clk"]);
    assert_eq!(q[0]["signal"], p("cfg_n"));
    assert_eq!(names(&loads(&f, "cfg_n.b.hi")), ["cfg_q.b.hi"]);
    assert_eq!(names(&loads(&f, "cfg_n.mode")), ["cfg_q.mode"]);
    // The pattern's other fields don't come into it.
    assert_eq!(names(&drivers(&f, "cfg_n.b.hi")), ["hi_in"]);
    assert_eq!(names(&drivers(&f, "cfg_n.en")), ["en_in"]);
    // Nothing reads lo.
    assert!(loads(&f, "cfg.b.lo").is_empty());
    assert_eq!(names(&loads(&f, "cfg.b.hi")), ["hi_out"]);
}

#[test]
fn bits_that_are_not_a_field_are_a_range() {
    let f = fixture("bits");
    // cfg is en[10] b.hi[9:6] b.lo[5:2] mode[1:0]: [5:0] is lo and mode.
    let entries = f.step("drivers", &p("cfg_q"), json!({ "follow": false }));
    assert!(names(&entries).contains(&"cfg_n".to_string()));
    let bits = f.step("loads", &p("cfg_n"), json!({ "follow": false }));
    assert_eq!(names(&bits), ["cfg_q"]);
}

#[test]
fn a_concatenation_on_the_left_splits_the_right_by_position() {
    let f = fixture("bits");
    // {c1, c2} = d_in: c1 gets the upper four bits.
    assert_eq!(names(&loads(&f, "d_in[7:4]")).iter().filter(|name| name.starts_with('c')).collect::<Vec<_>>(), ["c1"]);
    assert_eq!(names(&loads(&f, "d_in[3:0]")).iter().filter(|name| name.starts_with('c')).collect::<Vec<_>>(), ["c2"]);
    assert_eq!(names(&loads(&f, "d_in[5]")).iter().filter(|name| name.starts_with("c1")).collect::<Vec<_>>(), ["c1[1]"]);
    assert_eq!(names(&drivers(&f, "c1")), ["d_in[7:4]"].map(|_| "d_in".to_string()));
    // A trace of c1 itself is of all of c1: d_in is a driver, as a whole signal since c1 is asked whole.
}

#[test]
fn a_concatenation_on_the_right_places_each_part() {
    let f = fixture("bits");
    // pq = {p_in, q_in}
    assert_eq!(names(&drivers(&f, "pq[7:4]")), ["p_in"]);
    assert_eq!(site_places(&drivers(&f, "pq[7:4]")[0]), [f.after("assign pq = {").position()]);
    assert_eq!(names(&drivers(&f, "pq[3:0]")), ["q_in"]);
    assert_eq!(names(&drivers(&f, "pq[5:2]")), ["p_in[1:0]", "q_in[3:2]"].map(String::from));
    assert_eq!(names(&drivers(&f, "pq[3]")), ["q_in[3]"]);
    // The loads in the other direction.
    let from_p = loads(&f, "p_in[3]");
    let from_names = names(&from_p);
    assert!(from_names.contains(&"pq[7]".to_string()), "{from_names:?}");
    assert!(!from_names.contains(&"pq[3]".to_string()));
    assert_eq!(names(&loads(&f, "pq[7:4]")), ["split_hi"]);
    assert_eq!(names(&loads(&f, "pq[3:0]")), ["split_lo"]);
}

#[test]
fn a_replication_maps_each_copy() {
    let f = fixture("bits");
    assert_eq!(names(&drivers(&f, "rep[7:4]")), ["p_in"]);
    assert_eq!(names(&drivers(&f, "rep[7:6]")), ["p_in[3:2]"]);
    assert_eq!(names(&drivers(&f, "rep[1:0]")), ["p_in[1:0]"]);
    // p_in[3] reaches the top bit of each copy: two endpoints.
    let from = names(&loads(&f, "p_in[3]"));
    // (Two disjoint bits of one target are reported as the range that holds both.)
    assert!(from.contains(&"rep[7:3]".to_string()), "{from:?}");
}

#[test]
fn a_port_connected_to_a_part_maps_bit_for_bit() {
    let f = fixture("bits");
    let a = drivers(&f, "u_leaf.a[3:0]");
    assert_eq!(names(&a), ["leaf_in[3:0]"]);
    assert_eq!(kinds(&a[0]), ["port"]);
    // The output side: only the part of the net the port bits reach.
    let out = drivers(&f, "part_out[7:4]");
    assert_eq!(names(&out), ["u_leaf.y[7:4]"]);
    assert_eq!(kinds(&out[0]), ["port"]);
    // u_part's input is {4'h0, p_in}: the upper bits are a constant, the lower ones p_in.
    assert!(drivers(&f, "u_part.a[7:4]").is_empty());
    assert_eq!(names(&drivers(&f, "u_part.a[3:0]")), ["p_in"]);
    assert_eq!(names(&drivers(&f, "leaf_low")), ["wide"]);
    // wide[3:0] is what leaf_low reads; nothing reads the upper half.
    assert!(loads(&f, "wide[7:4]").is_empty());
    assert_eq!(names(&loads(&f, "wide[3:0]")), ["leaf_low"]);
    // Loads through the ports.
    assert_eq!(names(&loads(&f, "u_leaf.a[7:4]")), ["u_leaf.y[7:4]"]);
    assert_eq!(names(&loads(&f, "u_leaf.y[7:4]")), ["part_out[7:4]"]);
}

#[test]
fn only_the_bits_that_overlap_are_followed() {
    let f = fixture("bits");
    // part_out[7:4] <- u_leaf.y[7:4] <- u_leaf.a[7:4] <- leaf_in[7:4] <- q_in.
    let top = f.step("drivers", &p("part_out[7:4]"), json!({}));
    assert_eq!(paths(&top), [p("u_leaf.y[7:4]")]);
    let mut chain = vec![];
    let mut level = &top;
    while let Some(first) = level.first() {
        chain.push(first["path"].as_str().unwrap().to_string());
        level = match first["next"].as_array() {
            Some(next) => next,
            None => break,
        };
    }
    assert_eq!(chain, [p("u_leaf.y[7:4]"), p("u_leaf.a[7:4]"), p("leaf_in[7:4]"), p("q_in")]);
    // The same chain for the low bits goes to p_in instead.
    let low = f.step("drivers", &p("part_out[3:0]"), json!({}));
    let mut last = &low[0];
    while let Some(next) = last["next"].as_array().and_then(|next| next.first()) {
        last = next;
    }
    assert_eq!(last["path"], p("p_in"));
    // count and total work on the selected path of the entry.
    let y = &top[0];
    assert_eq!((y["count"].as_u64(), y["total"].as_u64()), (Some(1), Some(1)));
}

#[test]
fn a_selected_path_is_a_loop_when_it_repeats() {
    let f = fixture("bits");
    let entries = f.step("drivers", &p("v[3:0]"), json!({ "ancestors": [p("a_in[7:4]")] }));
    assert_eq!(entries[0]["loop"], true);
    let entries = f.step("drivers", &p("v[3:0]"), json!({ "ancestors": [p("a_in")] }));
    assert_eq!(entries[0]["loop"], false);
}

#[test]
fn a_non_constant_select_covers_its_whole_side() {
    let f = fixture("bits");
    // d_in[idx] reads all of d_in and idx.
    assert_eq!(names(&drivers(&f, "bit_sel")), ["d_in", "idx"]);
    let from = names(&loads(&f, "d_in[5]"));
    assert!(from.contains(&"bit_sel".to_string()));
    // m[idx] = en_in writes any bit of m, and idx picks which: a control source of every bit.
    assert_eq!(names(&drivers(&f, "m[2]")), ["en_in", "idx"]);
    assert_eq!(names(&drivers(&f, "m[7:4]")), ["en_in", "idx"]);
    assert_eq!(kinds(&drivers(&f, "m[2]")[1]), ["control"]);
    assert!(names(&loads(&f, "idx")).contains(&"m".to_string()));
}

#[test]
fn conditions_are_read_at_the_bits_they_select() {
    let f = fixture("bits");
    // if (a_in[0]) f[3:0] <= d_in[3:0]; f[7:4] <= d_in[7:4]
    let low = drivers(&f, "f[3:0]");
    assert_eq!(names(&low), ["a_in[0]", "clk", "d_in[3:0]"]);
    assert_eq!(kinds(&low[0]), ["control"]);
    assert_eq!(kinds(&low[1]), ["clock"]);
    let high = drivers(&f, "f[7:4]");
    assert_eq!(names(&high), ["clk", "d_in[7:4]"]);
    // A load of a bit of the condition reaches only what it gates.
    let from = names(&loads(&f, "a_in[0]"));
    assert!(from.contains(&"f[3:0]".to_string()), "{from:?}");
    assert!(!from.contains(&"f[7:4]".to_string()));
    assert!(!names(&loads(&f, "a_in[7:4]")).contains(&"f[3:0]".to_string()));
}

#[test]
fn an_unpacked_array_stays_whole() {
    let f = fixture("bits");
    assert_eq!(names(&drivers(&f, "mem[1]")), ["p_in"]);
    assert_eq!(names(&drivers(&f, "mem")), ["p_in"]);
    assert_eq!(names(&loads(&f, "mem[3]")), ["mem_out"]);
}

#[test]
fn unknown_fields_and_selects_out_of_range_are_errors() {
    let f = fixture("bits");
    for path in ["cfg.nope", "v[8]", "v[9:4]", "v[x]", "cfg.b.hi.deeper", "cfg.b.hi[4]", "nothing[1]", "v.f"] {
        for direction in ["drivers", "loads"] {
            let error = f.step_error(json!({ "path": p(path), "direction": direction }));
            assert_eq!(error, format!("Could not find path in compiled design: {}", p(path)), "{path}");
        }
    }
    // A select of a select, and a field of a field.
    assert_eq!(names(&drivers(&f, "cfg.b.hi[3:2]")), ["hi_in[3:2]"]);
    assert_eq!(names(&drivers(&f, "v[7:4][1]")), ["b_in[1]"]);
}

#[test]
fn endpoints_follow_the_bits_too() {
    let f = fixture("bits");
    let argument = |path: &str, direction: &str| json!({ "path": p(path), "direction": direction });
    let lines = |path: &str, direction: &str| -> Vec<u64> {
        let answer = trace::run(&f.graph, common::ENDPOINTS, &[argument(path, direction)]).unwrap();
        answer.as_array().unwrap().iter().map(|l| l["range"]["start"]["line"].as_u64().unwrap()).collect()
    };
    // part_out[7:4]: through the port, the leaf and leaf_in[7:4] to q_in's assignment.
    let line = |needle: &str| f.before(needle).position().0;
    assert_eq!(lines("part_out[7:4]", "drivers"), [line("assign leaf_in[7:4] = q_in"), ]);
    assert_eq!(lines("part_out[3:0]", "drivers"), [line("assign leaf_in[3:0] = p_in")]);
    // Loads of d_in[7:4]: c1's line, not c2's (the same line) but the select and the flop's half.
    let loads_lines = lines("d_in[5]", "loads");
    assert!(loads_lines.contains(&line("assign {c1, c2}")));
    assert!(loads_lines.contains(&line("f[7:4] <= d_in[7:4]")));
    assert!(!loads_lines.contains(&line("if (a_in[0]) f[3:0]")));
    // The constant in {4'h0, p_in} is an endpoint of the upper half only.
    assert_eq!(lines("u_part.a[7:4]", "drivers"), [line(".a({4'h0")]);
    // The lower half follows p_in to the top module port.
    assert_eq!(lines("u_part.a[3:0]", "drivers"), [line("input logic [3:0] p_in")]);
}

// ---- svtrace.atCursor ----

fn uri() -> String {
    svtrace::source::file_uri(&common::data_dir().canonicalize().unwrap().join("bits.sv"))
}

fn at_cursor(f: &Fixture, line: u64, character: u64) -> Vec<(String, String)> {
    let argument = json!({ "textDocument": { "uri": uri() }, "position": { "line": line, "character": character } });
    let answer = trace::run(&f.graph, trace::AT_CURSOR, &[argument]).expect("an answer");
    answer
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| (item["path"].as_str().unwrap().to_string(), item["signal"].as_str().unwrap().to_string()))
        .collect()
}

fn cursor(f: &Fixture, needle: &str) -> Vec<(String, String)> {
    let (line, character) = f.before(needle).position();
    at_cursor(f, line, character)
}

fn pair(path: &str, signal: &str) -> (String, String) {
    (p(path), p(signal))
}

#[test]
fn the_cursor_on_a_name_is_the_whole_signal() {
    let f = fixture("bits");
    assert_eq!(cursor(&f, "a_in[7:4];"), [pair("a_in", "a_in")]);
    assert_eq!(cursor(&f, "b_in;"), [pair("b_in", "b_in")]);
    // Inside the name, and just past its end.
    let (line, character) = f.before("b_in;").position();
    assert_eq!(at_cursor(&f, line, character + 2), [pair("b_in", "b_in")]);
    assert_eq!(at_cursor(&f, line, character + 4), [pair("b_in", "b_in")]);
}

#[test]
fn the_cursor_on_a_select_or_a_field_is_the_selected_path() {
    let f = fixture("bits");
    // On the 7 of a_in[7:4]: the constant select.
    let (line, character) = f.before("7:4];").position();
    assert_eq!(at_cursor(&f, line, character), [pair("a_in[7:4]", "a_in")]);
    assert_eq!(cursor(&f, "v[5];"), [pair("v", "v")]);
    let (line, character) = f.before("5];").position();
    assert_eq!(at_cursor(&f, line, character), [pair("v[5]", "v")]);
    // Fields: on cfg, on b, on hi.
    let needle = "cfg.b.hi;";
    let (line, character) = f.before(needle).position();
    assert_eq!(at_cursor(&f, line, character), [pair("cfg", "cfg")]);
    assert_eq!(at_cursor(&f, line, character + 4), [pair("cfg.b", "cfg")]);
    assert_eq!(at_cursor(&f, line, character + 6), [pair("cfg.b.hi", "cfg")]);
    // On the 3 of up[0:3]'s left side: the bits it names.
    let (line, character) = f.before("0:3] = ").position();
    assert_eq!(at_cursor(&f, line, character), [pair("up[0:3]", "up")]);
}

#[test]
fn the_cursor_on_a_declaration_is_the_whole_signal() {
    let f = fixture("bits");
    assert_eq!(cursor(&f, "v_low, v_high"), [pair("v_low", "v_low")]);
    assert_eq!(cursor(&f, "v_high;"), [pair("v_high", "v_high")]);
    // A port's declaration.
    assert_eq!(cursor(&f, "a_in,\n"), [pair("a_in", "a_in")]);
    assert_eq!(cursor(&f, "cfg;"), [pair("cfg", "cfg")]);
}

#[test]
fn the_cursor_gives_one_path_per_instance_of_the_module() {
    let f = fixture("bits");
    let (line, character) = f.before("y = a;").position();
    assert_eq!(
        at_cursor(&f, line, character),
        [("bits_top.u_leaf.y", "bits_top.u_leaf.y"), ("bits_top.u_part.y", "bits_top.u_part.y")]
            .map(|(path, signal)| (path.to_string(), signal.to_string()))
    );
    // And a port declaration inside the module, by its name.
    let (line, character) = f.before("a,\n").position();
    assert_eq!(at_cursor(&f, line, character).len(), 2);
}

#[test]
fn nothing_traceable_under_the_cursor_is_an_empty_list() {
    let f = fixture("bits");
    // A comment, a keyword, white space, a non-constant select's brackets, a literal, past the file.
    assert!(cursor(&f, "// Part selects").is_empty());
    assert!(cursor(&f, "assign v[3:0]").is_empty());
    // (Just past a name still counts as the name: the bracket after d_in is d_in.)
    let (line, character) = f.before("[idx];").position();
    assert_eq!(at_cursor(&f, line, character), [pair("d_in", "d_in")]);
    assert!(cursor(&f, "8'h00").is_empty());
    assert!(at_cursor(&f, 9999, 0).is_empty());
    // A file that is not in the design.
    let argument = json!({ "textDocument": { "uri": "file:///elsewhere.sv" }, "position": { "line": 0, "character": 0 } });
    assert_eq!(trace::run(&f.graph, trace::AT_CURSOR, &[argument]).unwrap(), json!([]));
    // A bad argument.
    assert!(trace::run(&f.graph, trace::AT_CURSOR, &[json!({ "position": {} })]).is_err());
}

#[test]
fn columns_are_utf16_units() {
    let f = fixture("bits");
    let text = &f.text;
    let at = text.find("v[6];").unwrap();
    let line_start = text[..at].rfind('\n').unwrap() + 1;
    let line = text[..at].matches('\n').count() as u64;
    let utf16 = text[line_start..at].encode_utf16().count() as u64;
    assert!(utf16 < (at - line_start) as u64, "the line has wide characters before the use");
    assert_eq!(at_cursor(&f, line, utf16), [pair("v", "v")]);
    assert_eq!(at_cursor(&f, line, utf16 + 2), [pair("v[6]", "v")]);
    // Counting bytes would be somewhere else.
    assert_ne!(at_cursor(&f, line, (at - line_start) as u64), [pair("v", "v")]);
}

#[test]
fn at_cursor_is_advertised_and_follows_a_trace() {
    assert!(trace::COMMANDS.contains(&"svtrace.atCursor"));
    let f = fixture("bits");
    let found = cursor(&f, "7:4];");
    let (path, signal) = &found[0];
    assert_eq!(path, &p("a_in[7:4]"));
    assert_eq!(signal, &p("a_in"));
    // The path it gives is one a trace takes.
    let entries = f.step("loads", path, json!({ "follow": false }));
    assert_eq!(names(&entries), ["up[4]", "v[3:0]"]);
}

// ---- nothing is lost ----

/// Asking bit by bit finds every signal that asking the whole signal finds, in both directions.
#[test]
fn bit_by_bit_finds_everything_the_whole_signal_finds() {
    let mut compared = 0;
    for name in ["bits", "cone_test", "cone_sites", "cone_undriven", "clocks", "steps", "extras", "paths"] {
        let f = fixture(name);
        for signal in f.graph.signals().map(str::to_string).collect::<Vec<_>>() {
            let Some(svtrace::graph::Target::Value(id)) = f.graph.lookup(&signal) else { continue };
            let width = f.graph.width(&f.graph.symbols[id].ty);
            if !(2..=64).contains(&width) {
                continue;
            }
            for direction in ["drivers", "loads"] {
                let argument = json!({ "path": signal, "direction": direction, "follow": false });
                let Ok(whole) = trace::run(&f.graph, common::STEP, &[argument]) else { continue };
                let whole: std::collections::BTreeSet<String> =
                    whole.as_array().unwrap().iter().map(|e| e["signal"].as_str().unwrap().to_string()).collect();
                let mut parts = std::collections::BTreeSet::new();
                for bit in 0..width {
                    let argument = json!({ "path": format!("{signal}[{bit}]"), "direction": direction, "follow": false });
                    if let Ok(answer) = trace::run(&f.graph, common::STEP, &[argument]) {
                        parts.extend(answer.as_array().unwrap().iter().map(|e| e["signal"].as_str().unwrap().to_string()));
                    }
                }
                assert!(whole.is_subset(&parts), "{name}: {direction} {signal}: whole {whole:?}, bits {parts:?}");
                compared += 1;
            }
        }
    }
    assert!(compared > 50, "only {compared} compared");
}

// ---- CV32E40P ----

/// `SVTRACE_DESIGN=~/projects/cv32e40p cargo test --release -p svtrace --test bits -- --ignored`.
/// Needs the slang at `~/tools/slang/slang`.
#[test]
#[ignore = "needs SVTRACE_DESIGN and slang"]
fn mstatus_mie_has_only_the_lines_that_write_mie() {
    let Some(design) = std::env::var_os("SVTRACE_DESIGN") else {
        eprintln!("skipped: SVTRACE_DESIGN is not set");
        return;
    };
    let slang = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/slang/slang");
    let design = std::path::Path::new(&design).canonicalize().expect("SVTRACE_DESIGN is a folder");
    let built = svtrace::build(&slang, &design).expect("slang runs on the design");
    let registers = "cv32e40p_top.core_i.cs_registers_i";
    let path = format!("{registers}.mstatus_q.mie");
    let run = |command: &str, argument: Value| trace::run(&built.graph, command, &[argument]).expect("an answer");
    let step = run(common::STEP, json!({ "path": path, "direction": "drivers", "follow": false }));
    let step = step.as_array().unwrap();
    // The data: mstatus_n.mie only, at `mstatus_q <= mstatus_n` and at `mie: mstatus_n.mie`.
    let n = entry(step, &format!("{registers}.mstatus_n.mie"));
    assert_eq!(n["signal"], format!("{registers}.mstatus_n"));
    let source = std::fs::read_to_string(design.join("rtl/cv32e40p_cs_registers.sv")).unwrap();
    let lines: Vec<&str> = source.lines().collect();
    let site_lines: Vec<(u64, String)> = n["sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|site| {
            let line = site["location"]["range"]["start"]["line"].as_u64().unwrap();
            (line + 1, lines[line as usize].trim().to_string())
        })
        .collect();
    assert_eq!(
        site_lines,
        [(1221, "mstatus_q <= mstatus_n;".to_string()), (1225, "mie: mstatus_n.mie,".to_string())]
    );
    // Beside it only the clock and reset of the flop.
    let rest: Vec<&str> = step.iter().map(|e| e["signal"].as_str().unwrap()).filter(|s| !s.ends_with("mstatus_n")).collect();
    assert_eq!(rest, [format!("{registers}.clk"), format!("{registers}.rst_n")]);
    // The whole walk's endpoints add the reset's constant (`mstatus_q <= '{ ... mie: 1'b0 ...}`).
    let endpoints = run(common::ENDPOINTS, json!({ "path": path, "direction": "drivers" }));
    let lines_of: Vec<u64> = endpoints.as_array().unwrap().iter().map(|l| l["range"]["start"]["line"].as_u64().unwrap() + 1).collect();
    assert_eq!(lines_of, [1221, 1225, 1187]);
    // What mie feeds: the lines that read it.
    let loads = run(common::STEP, json!({ "path": path, "direction": "loads", "follow": false }));
    let mut read: Vec<u64> = vec![];
    for e in loads.as_array().unwrap() {
        for site in e["sites"].as_array().unwrap() {
            read.push(site["location"]["range"]["start"]["line"].as_u64().unwrap() + 1);
        }
    }
    read.sort();
    assert_eq!(read, [478, 903, 1059, 1104]);
    // The phase 1 command still gives the whole struct.
    let whole = trace::execute(&built.graph, common::DRIVERS, &format!("{registers}.mstatus_q")).unwrap();
    assert!(whole.as_array().unwrap().iter().any(|e| e["path"].as_str().unwrap().ends_with("PRIV_LVL_M")));
}

//! `svtrace.atCursor` inside a macro invocation, on a design elaborated by the real slang (the
//! dump gives everything a macro expands to the whole invocation as its place).

mod common;

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Map};

fn at(graph: &svtrace::graph::Graph, uri: &str, line: usize, character: usize) -> Vec<String> {
    let mut argument = Map::new();
    argument.insert("textDocument".into(), json!({ "uri": uri }));
    argument.insert("position".into(), json!({ "line": line, "character": character }));
    let answer = svtrace::cursor::at_cursor(graph, &argument).expect("an answer");
    answer.as_array().unwrap().iter().map(|entry| entry["path"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_cursor_in_a_macro_invocation_picks_the_signal_it_is_on_or_nothing() {
    let slang = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/slang/slang");
    if !slang.exists() {
        eprintln!("skipped: no slang at {}", slang.display());
        return;
    }
    let dir = std::env::temp_dir().join(format!("svtrace-macro-cursor-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".slang")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let text = "`define ASSIGN(dst, src) assign dst = src\n\
                module top(input logic [3:0] a, input logic [3:0] b, output logic k, output logic m);\n\
                \x20 `ASSIGN(k, a[0] & b[0]);\n\
                \x20 `ASSIGN(m, a[1]);\n\
                endmodule\n";
    fs::write(dir.join("top.sv"), text).unwrap();
    fs::write(dir.join("top.f"), "top.sv\n").unwrap();
    fs::write(dir.join(".slang/server.json"), r#"{ "build": "top.f" }"#).unwrap();
    let built = svtrace::build(&slang, &dir).expect("slang runs");
    let uri = svtrace::source::file_uri(&dir.join("top.sv"));
    let column = |line: usize, needle: &str| text.lines().nth(line).unwrap().find(needle).unwrap();

    // On each signal of `a[0] & b[0]`: that signal, not the first one of the invocation (the signal
    // as a whole: the dump doesn't say where in the invocation the select is).
    assert_eq!(at(&built.graph, &uri, 2, column(2, "b[0]")), ["top.b"]);
    assert_eq!(at(&built.graph, &uri, 2, column(2, "a[0]")), ["top.a"]);
    // An argument that is a signal's name.
    assert_eq!(at(&built.graph, &uri, 2, column(2, "k,")), ["top.k"]);
    assert_eq!(at(&built.graph, &uri, 3, column(3, "a[1]")), ["top.a"]);
    // Where no signal is written: nothing, not a signal of the invocation.
    assert_eq!(at(&built.graph, &uri, 2, column(2, "&")), Vec::<String>::new());
    assert_eq!(at(&built.graph, &uri, 2, column(2, "ASSIGN")), Vec::<String>::new());
    let _ = fs::remove_dir_all(dir);
}

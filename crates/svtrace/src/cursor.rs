//! `svtrace.atCursor`: the signals (and the bits of them) that a position in a source file names,
//! one per instance of the module the file's text is in.

use serde_json::{json, Map, Value};

use crate::ast::{Node, NodeKind, Pos, ValueKind};
use crate::bits::Bits;
use crate::graph::{Container, Graph, SymId};
use crate::trace::CommandError;

/// A name under the cursor.
struct Hit {
    /// How big the text it covers is: the smallest hit is the innermost.
    extent: (u32, i64),
    sym: SymId,
    bits: Bits,
}

fn extent(start: Pos, end: Pos) -> (u32, i64) {
    (end.line - start.line, if end.line == start.line { i64::from(end.col) - i64::from(start.col) } else { i64::from(end.col) })
}

fn contains(start: Pos, end: Pos, at: Pos, inclusive: bool) -> bool {
    start <= at && (at < end || (inclusive && at == end))
}

/// `svtrace.atCursor`, argument `{ textDocument = { uri }, position = { line, character } }`:
/// `[{ path, signal }]`, one per instance, `path` being the selected path.
pub fn at_cursor(graph: &Graph, argument: &Map<String, Value>) -> Result<Value, CommandError> {
    let bad = || "svtrace.atCursor takes { textDocument, position } as its argument".to_string();
    let uri = argument.get("textDocument").and_then(|doc| doc.get("uri")).and_then(Value::as_str).ok_or_else(bad)?;
    let position = argument.get("position").ok_or_else(bad)?;
    let line = position.get("line").and_then(Value::as_u64).ok_or_else(bad)? as u32;
    let character = position.get("character").and_then(Value::as_u64).ok_or_else(bad)? as u32;
    let none = Ok(Value::Array(Vec::new()));
    let Some(file) = graph.sources.file_by_uri(uri) else { return none };
    let Some(at) = graph.sources.dump_pos(file, line, character) else { return none };

    // (A cursor just past a name, as in insert mode, counts when nothing is under it.)
    for inclusive in [false, true] {
        let mut hits = declarations(graph, file, at, inclusive);
        for container in &graph.containers {
            match container {
                Container::Assign(node) | Container::Procedural { body: node, .. } => {
                    hits.extend(search(graph, node, file, at, inclusive));
                }
                Container::Instance(connections) => {
                    for node in connections.iter().filter_map(|connection| connection.expr.as_ref()) {
                        hits.extend(search(graph, node, file, at, inclusive));
                    }
                }
            }
        }
        let Some(best) = hits.iter().map(|hit| hit.extent).min() else { continue };
        let mut answers: Vec<(String, String)> = Vec::new();
        for hit in hits.into_iter().filter(|hit| hit.extent == best) {
            let answer = (graph.selected_path(hit.sym, hit.bits), graph.symbols[hit.sym].path.clone());
            if !answers.contains(&answer) {
                answers.push(answer);
            }
        }
        return Ok(answers.into_iter().map(|(path, signal)| json!({ "path": path, "signal": signal })).collect());
    }
    none
}

/// The signals declared at the cursor: their names are the whole signal.
fn declarations(graph: &Graph, file: u32, at: Pos, inclusive: bool) -> Vec<Hit> {
    graph
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, symbol)| !symbol.local && matches!(symbol.kind, ValueKind::Variable | ValueKind::Net))
        .filter_map(|(sym, symbol)| {
            let loc = symbol.loc.filter(|loc| loc.file == file)?;
            let end = Pos { line: loc.pos.line, col: loc.pos.col + symbol.name.len() as u32 };
            contains(loc.pos, end, at, inclusive).then(|| Hit { extent: extent(loc.pos, end), sym, bits: None })
        })
        .collect()
}

/// The innermost name, field access or constant select under the position in an expression tree.
fn search(graph: &Graph, node: &Node, file: u32, at: Pos, inclusive: bool) -> Option<Hit> {
    for child in &node.children {
        if let Some(hit) = search(graph, child, file, at, inclusive) {
            return Some(hit);
        }
    }
    let range = node.range.filter(|range| range.file == file && contains(range.start, range.end, at, inclusive))?;
    if !matches!(node.kind, NodeKind::Value { .. } | NodeKind::Select(_) | NodeKind::Member { .. }) {
        return None;
    }
    let access = graph.access(node)?;
    let symbol = &graph.symbols[access.sym];
    // Inside a macro invocation every node has the whole invocation as its range: the signal is the
    // one whose name is under the cursor, and when none is, nothing (not another signal's).
    if graph.sources.at_name_in_macro(range, at, &symbol.name) == Some(false) {
        return None;
    }
    (!symbol.local && !graph.is_constant(access.sym)).then(|| Hit {
        extent: extent(range.start, range.end),
        sym: access.sym,
        bits: access.bits(),
    })
}

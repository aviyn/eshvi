//! Shared by the tests: a fixture design read from its checked-in dump, positions found in its
//! source the way ConeTests.cpp's harness finds them, and checks of the answers.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use svtrace::graph::Graph;
use svtrace::trace;

pub const DRIVERS: &str = "slang.getDriversWithLocation";
pub const LOADS: &str = "slang.getLoadsWithLocation";
pub const CONSTANTS: &str = "slang.getDriversWithConstants";

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// A fixture design and its source text.
pub struct Fixture {
    pub graph: Graph,
    pub file: String,
    pub text: String,
}

/// Reads `tests/data/<name>.json`, the dump `scripts/svtrace-fixtures.sh` made of `<name>.sv`.
pub fn fixture(name: &str) -> Fixture {
    let dir = data_dir();
    let dump = fs::read_to_string(dir.join(format!("{name}.json"))).expect("the fixture's dump");
    let design = svtrace::ast::read(&svtrace::elaborate::parse(dump.as_bytes()).expect("valid JSON")).expect("a dump");
    let file = format!("{name}.sv");
    let text = fs::read_to_string(dir.join(&file)).expect("the fixture's source");
    Fixture { graph: Graph::build(&dir, design), file, text }
}

/// A place in the fixture's source, found like `doc.before("...")` and `cursor.after("...")` in the
/// fork's test harness: the first match from the cursor on.
#[derive(Clone, Copy)]
pub struct Cursor<'a> {
    text: &'a str,
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub fn before(self, needle: &str) -> Cursor<'a> {
        Cursor { offset: self.find(needle), ..self }
    }

    pub fn after(self, needle: &str) -> Cursor<'a> {
        Cursor { offset: self.find(needle) + needle.len(), ..self }
    }

    fn find(&self, needle: &str) -> usize {
        match self.text[self.offset..].find(needle) {
            Some(at) => self.offset + at,
            None => panic!("{needle:?} not found after offset {}", self.offset),
        }
    }

    /// The 0-based line and column (the fixtures are ASCII).
    pub fn position(self) -> (u64, u64) {
        let before = &self.text[..self.offset];
        let line = before.matches('\n').count() as u64;
        let column = before.rfind('\n').map_or(before.len(), |at| before.len() - at - 1);
        (line, column as u64)
    }
}

impl Fixture {
    pub fn doc(&self) -> Cursor<'_> {
        Cursor { text: &self.text, offset: 0 }
    }

    pub fn before(&self, needle: &str) -> Cursor<'_> {
        self.doc().before(needle)
    }

    pub fn after(&self, needle: &str) -> Cursor<'_> {
        self.doc().after(needle)
    }

    pub fn answer(&self, command: &str, path: &str) -> Value {
        trace::execute(&self.graph, command, path).unwrap_or_else(|error| panic!("{command} {path}: {error}"))
    }

    pub fn error(&self, command: &str, path: &str) -> String {
        trace::execute(&self.graph, command, path).expect_err("an error")
    }

    /// The endpoints of `command` for `path` are exactly these signals, declared at these places.
    pub fn check_cone(&self, command: &str, path: &str, expected: &[(&str, Cursor)]) {
        let mut got: Vec<(String, u64, u64)> = self
            .answer(command, path)
            .as_array()
            .expect("a list")
            .iter()
            .map(|entry| {
                self.check_file(&entry["location"]);
                let start = &entry["location"]["range"]["start"];
                (
                    entry["path"].as_str().unwrap().to_string(),
                    start["line"].as_u64().unwrap(),
                    start["character"].as_u64().unwrap(),
                )
            })
            .collect();
        let mut want: Vec<(String, u64, u64)> = expected
            .iter()
            .map(|(name, cursor)| (name.to_string(), cursor.position().0, cursor.position().1))
            .collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "{command} {path}");
    }

    /// The sites of the endpoint `endpoint` in the answer to `command` for `path`, in order.
    pub fn check_sites(&self, command: &str, path: &str, endpoint: &str, expected: &[(Cursor, &str)]) {
        let answer = self.answer(command, path);
        let entry = answer
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == endpoint)
            .unwrap_or_else(|| panic!("{endpoint} is not an endpoint of {path}: {answer}"));
        self.assert_sites(entry["sites"].as_array().unwrap(), expected, &format!("{command} {path} -> {endpoint}"));
    }

    /// The constants assigned to `path`, in order.
    pub fn check_constants(&self, path: &str, expected: &[(Cursor, &str)]) {
        let answer = self.answer(CONSTANTS, path);
        self.assert_sites(answer["constants"].as_array().unwrap(), expected, &format!("constants of {path}"));
    }

    fn assert_sites(&self, sites: &[Value], expected: &[(Cursor, &str)], what: &str) {
        let got: Vec<(u64, u64, String)> = sites
            .iter()
            .map(|site| {
                self.check_file(&site["location"]);
                let start = &site["location"]["range"]["start"];
                (start["line"].as_u64().unwrap(), start["character"].as_u64().unwrap(), site["kind"].as_str().unwrap().to_string())
            })
            .collect();
        let want: Vec<(u64, u64, String)> =
            expected.iter().map(|(cursor, kind)| (cursor.position().0, cursor.position().1, kind.to_string())).collect();
        assert_eq!(got, want, "{what}");
    }

    fn check_file(&self, location: &Value) {
        let uri = location["uri"].as_str().unwrap();
        assert!(uri.starts_with("file:///") && uri.ends_with(&self.file), "{uri}");
    }
}

pub const STEP: &str = "svtrace.step";
pub const ENDPOINTS: &str = "svtrace.endpoints";

impl Fixture {
    /// `svtrace.step` for `path` and `direction`, with the rest of the argument in `extra`.
    pub fn step(&self, direction: &str, path: &str, extra: Value) -> Vec<Value> {
        let mut argument = serde_json::json!({ "path": path, "direction": direction });
        for (key, value) in extra.as_object().into_iter().flatten() {
            argument[key] = value.clone();
        }
        let answer = trace::run(&self.graph, STEP, &[argument]).unwrap_or_else(|error| panic!("step {path}: {error}"));
        answer.as_array().expect("a list").clone()
    }

    pub fn step_error(&self, argument: Value) -> String {
        trace::run(&self.graph, STEP, &[argument]).expect_err("an error")
    }

    /// `svtrace.endpoints`: the (line, column) of each location, 0-based, in order.
    pub fn endpoints(&self, direction: &str, path: &str) -> Vec<(u64, u64)> {
        let argument = serde_json::json!({ "path": path, "direction": direction });
        let answer = trace::run(&self.graph, ENDPOINTS, &[argument]).unwrap_or_else(|error| panic!("endpoints {path}: {error}"));
        answer
            .as_array()
            .expect("a list")
            .iter()
            .map(|location| {
                self.check_file(location);
                let start = &location["range"]["start"];
                (start["line"].as_u64().unwrap(), start["character"].as_u64().unwrap())
            })
            .collect()
    }
}

/// The entry for `path` in a step's answer.
pub fn entry<'a>(entries: &'a [Value], path: &str) -> &'a Value {
    entries.iter().find(|entry| entry["path"] == path).unwrap_or_else(|| panic!("{path} in {:?}", paths(entries)))
}

pub fn paths(entries: &[Value]) -> Vec<&str> {
    entries.iter().map(|entry| entry["path"].as_str().unwrap()).collect()
}

/// The kinds of an entry's sites, in order.
pub fn kinds(entry: &Value) -> Vec<&str> {
    entry["sites"].as_array().unwrap().iter().map(|site| site["kind"].as_str().unwrap()).collect()
}

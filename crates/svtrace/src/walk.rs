//! Questions that walk the graph more than one step: `svtrace.step` (a signal's drivers or loads, each
//! entry's own step counted and single-step chains followed) and `svtrace.endpoints` (where a signal
//! really ends up, skipping the interconnect). They reproduce what `slang/trace.lua` works out one
//! question at a time (`build_rows`, `follows`, `collect_endpoints`), without its question limits.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde_json::{json, Map, Value};

use crate::bits::Bits;
use crate::graph::{Graph, SymId, Target};
use crate::trace::{self, CommandError, Leaf};

/// An answer of more entries than this, in 2 instances or more, is "many": not followed.
const MANY_OVER: usize = 8;
/// How many levels below the asked signal a chain of single steps is followed.
const FOLLOW_DEPTH: u32 = 24;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Drivers,
    Loads,
}

struct Question<'a> {
    /// The signals to start from: `path`, or `paths` (merged instances), without duplicates.
    paths: Vec<&'a str>,
    direction: Direction,
    /// An instance path whose rows come first where a merged row has several paths.
    prefer: Option<&'a str>,
}

fn question<'a>(argument: &'a Map<String, Value>, command: &str) -> Result<Question<'a>, CommandError> {
    let bad = || format!("{command} takes {{ path, direction }} as its argument");
    let mut paths: Vec<&str> = Vec::new();
    match argument.get("paths") {
        None | Some(Value::Null) => paths.push(argument.get("path").and_then(Value::as_str).ok_or_else(bad)?),
        Some(Value::Array(list)) => {
            for path in list {
                let path = path.as_str().ok_or_else(|| format!("{command}: paths are paths"))?;
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            if paths.is_empty() {
                return Err(format!("{command}: paths has no path"));
            }
        }
        Some(_) => return Err(format!("{command}: paths are paths")),
    }
    let direction = match argument.get("direction").map(Value::as_str) {
        None | Some(Some("drivers")) => Direction::Drivers,
        Some(Some("loads")) => Direction::Loads,
        Some(_) => return Err(format!("{command}: direction is \"drivers\" or \"loads\"")),
    };
    let prefer = match argument.get("prefer") {
        None | Some(Value::Null) => None,
        Some(Value::String(prefer)) => Some(prefer.as_str()),
        Some(_) => return Err(format!("{command}: prefer is an instance path")),
    };
    Ok(Question { paths, direction, prefer })
}

/// The signal a question starts from and the bits of it (a field, a select) the path names, with the
/// errors the phase 1 commands give.
fn start(graph: &Graph, path: &str, direction: Direction) -> Result<(SymId, Bits), CommandError> {
    let (target, bits) = match graph.lookup(path) {
        Some(target) => (target, None),
        None => match graph.lookup_selected(path) {
            Some((id, bits)) => (Target::Value(id), bits),
            None => return Err(format!("Could not find path in compiled design: {path}")),
        },
    };
    match (target, direction) {
        (Target::Value(id), Direction::Drivers) => Ok((id, bits)),
        (Target::Value(id), Direction::Loads) if !graph.used_by(id).is_empty() => Ok((id, bits)),
        (_, Direction::Drivers) => Err(format!("Path is not a value symbol: {path}")),
        (_, Direction::Loads) => Err(format!("Could not find reference to: {path}")),
    }
}

/// All the starts of a question.
fn starts(graph: &Graph, question: &Question) -> Result<Vec<Start>, CommandError> {
    question
        .paths
        .iter()
        .map(|path| start(graph, path, question.direction).map(|(id, bits)| Start { id, bits, path: path.to_string() }))
        .collect()
}

/// A signal a step is asked about, as the path it was asked by.
#[derive(Clone)]
struct Start {
    id: SymId,
    bits: Bits,
    path: String,
}

/// One entry of a step.
struct Entry {
    id: SymId,
    /// The path to ask next: the signal's, with the field or select when only some bits were involved.
    path: String,
    /// The signal's own path.
    signal: String,
    /// The bits the entry's own step is asked about (None: all of them, and always so when the
    /// question named a whole signal).
    next_bits: Bits,
    location: Value,
    sites: Vec<(Value, &'static str)>,
    /// An enum member or a parameter, which the graph knows.
    constant: bool,
}

impl Entry {
    fn from(graph: &Graph, leaf: Leaf, asked: Bits) -> Entry {
        let (path, next_bits) = match asked {
            Some(_) => (graph.selected_path(leaf.id, leaf.bits), leaf.bits),
            None => (leaf.path.clone(), None),
        };
        Entry {
            id: leaf.id,
            constant: graph.is_constant(leaf.id),
            path,
            signal: leaf.path,
            next_bits,
            location: leaf.location,
            sites: leaf.sites,
        }
    }
}

/// A signal's one step, sorted by path, once each.
struct Step {
    entries: Vec<Entry>,
    /// Where the signal is assigned a value no signal feeds (drivers only).
    constants: Vec<Value>,
}

/// One row of a merged step: the entries of the same declaration under the same module-relative name.
struct Row {
    /// The first of `paths`, and its signal.
    path: String,
    signal: String,
    location: Value,
    sites: Vec<(Value, &'static str)>,
    constant: bool,
    /// The row's distinct paths, those under `prefer` first, then sorted.
    paths: Vec<String>,
    /// The same as starts for the row's own step, one per path.
    members: Vec<Start>,
    /// The asked paths (of the step's starts) whose entries are in the row, in the starts' order.
    from: Vec<String>,
}

/// What a merged row is being gathered from.
struct Group {
    location: Value,
    constant: bool,
    sites: Vec<(Value, &'static str)>,
    /// (start, signal) per distinct path.
    members: Vec<(Start, String)>,
    from: Vec<String>,
}

struct Walker<'g> {
    graph: &'g Graph,
    direction: Direction,
    prefer: Option<String>,
    steps: HashMap<(SymId, Bits), Rc<Step>>,
    merges: HashMap<Vec<(SymId, Bits)>, Rc<Vec<Row>>>,
}

impl<'g> Walker<'g> {
    fn new(graph: &'g Graph, direction: Direction, prefer: Option<&str>) -> Self {
        Walker { graph, direction, prefer: prefer.map(str::to_string), steps: HashMap::new(), merges: HashMap::new() }
    }

    /// Whether a row is a signal for counting: not a constant and, for drivers, not only a clock.
    fn is_signal(&self, row: &Row) -> bool {
        let clock_only = !row.sites.is_empty() && row.sites.iter().all(|(_, kind)| *kind == "clock");
        !row.constant && !(self.direction == Direction::Drivers && clock_only)
    }

    /// The step of `id`. A signal nothing reads has no loads (the command itself calls that an error).
    fn step(&mut self, id: SymId, asked: Bits) -> Rc<Step> {
        if let Some(step) = self.steps.get(&(id, asked)) {
            return step.clone();
        }
        let graph = self.graph;
        let cone = match self.direction {
            Direction::Drivers => Some(trace::drivers_of(graph, id, asked)),
            Direction::Loads => trace::loads_of(graph, id, asked),
        };
        let step = cone.map_or_else(
            || Step { entries: Vec::new(), constants: Vec::new() },
            |cone| {
                let mut entries: Vec<Entry> = cone.leaves(graph).into_iter().map(|leaf| Entry::from(graph, leaf, asked)).collect();
                // (A stable sort keeps the first of two entries with one path.)
                entries.sort_by(|a, b| a.signal.cmp(&b.signal).then_with(|| a.path.cmp(&b.path)));
                entries.dedup_by(|later, first| later.path == first.path);
                let constants = if self.direction == Direction::Drivers { cone.constant_locations(graph) } else { Vec::new() };
                Step { entries, constants }
            },
        );
        let step = Rc::new(step);
        self.steps.insert((id, asked), step.clone());
        step
    }

    /// Whether a path is in the instance `prefer` (or under it).
    fn preferred(&self, path: &str) -> bool {
        self.prefer.as_deref().is_some_and(|prefer| path.strip_prefix(prefer).is_some_and(|rest| rest.starts_with('.')))
    }

    /// The merged step of `starts`: the union of each one's entries, in rows by `(local, location, constant)`.
    /// Of one start it is that start's step, entry for entry, in its order. (Two entries of one start are never
    /// one row, which a start's `u[0].o` and `u[1].o` would otherwise be: the k-th entry of a key in a start
    /// goes with the k-th of the others.)
    fn merged(&mut self, starts: &[Start]) -> Rc<Vec<Row>> {
        let key: Vec<(SymId, Bits)> = starts.iter().map(|start| (start.id, start.bits)).collect();
        if let Some(rows) = self.merges.get(&key) {
            return rows.clone();
        }
        let mut rows: Vec<Row>;
        if let [only] = starts {
            let step = self.step(only.id, only.bits);
            rows = step
                .entries
                .iter()
                .map(|e| Row {
                    path: e.path.clone(),
                    signal: e.signal.clone(),
                    location: e.location.clone(),
                    sites: e.sites.clone(),
                    constant: e.constant,
                    paths: vec![e.path.clone()],
                    members: vec![Start { id: e.id, bits: e.next_bits, path: e.path.clone() }],
                    from: vec![only.path.clone()],
                })
                .collect();
        } else {
            type Key = (String, String, bool, usize);
            let mut order: Vec<Key> = Vec::new();
            let mut groups: HashMap<Key, Group> = HashMap::new();
            for start in starts {
                let step = self.step(start.id, start.bits);
                let mut count: HashMap<(String, String, bool), usize> = HashMap::new();
                for e in &step.entries {
                    let base = (local_of(&e.signal, &e.path).to_string(), e.location.to_string(), e.constant);
                    let nth = count.entry(base.clone()).or_insert(0);
                    let key = (base.0, base.1, base.2, *nth);
                    *nth += 1;
                    let group = groups.entry(key.clone()).or_insert_with(|| {
                        order.push(key);
                        Group { location: e.location.clone(), constant: e.constant, sites: Vec::new(), members: Vec::new(), from: Vec::new() }
                    });
                    if !group.members.iter().any(|(member, _)| member.path == e.path) {
                        group.members.push((Start { id: e.id, bits: e.next_bits, path: e.path.clone() }, e.signal.clone()));
                    }
                    for site in &e.sites {
                        if !group.sites.contains(site) {
                            group.sites.push(site.clone());
                        }
                    }
                    if !group.from.contains(&start.path) {
                        group.from.push(start.path.clone());
                    }
                }
            }
            let mut keyed: Vec<(String, Row)> = Vec::new();
            for key in order {
                let mut group = groups.remove(&key).unwrap();
                group.members.sort_by(|a, b| (!self.preferred(&a.0.path)).cmp(&!self.preferred(&b.0.path)).then_with(|| a.0.path.cmp(&b.0.path)));
                let (first, signal) = group.members[0].clone();
                keyed.push((
                    key.0,
                    Row {
                        path: first.path.clone(),
                        signal,
                        location: group.location,
                        sites: group.sites,
                        constant: group.constant,
                        paths: group.members.iter().map(|(member, _)| member.path.clone()).collect(),
                        members: group.members.into_iter().map(|(member, _)| member).collect(),
                        from: group.from,
                    },
                ));
            }
            keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
            rows = keyed.into_iter().map(|(_, row)| row).collect();
        }
        let rows = Rc::new(std::mem::take(&mut rows));
        self.merges.insert(key, rows.clone());
        rows
    }

    /// Whether an answer is "many": more than `MANY_OVER` signals, in 2 instances or more.
    fn many(&self, rows: &[Row]) -> bool {
        let signals: Vec<&Row> = rows.iter().filter(|row| self.is_signal(row)).collect();
        if signals.len() <= MANY_OVER {
            return false;
        }
        let first = instance_of(&signals[0].signal);
        signals.iter().any(|row| instance_of(&row.signal) != first)
    }

    /// The rows of the merged step of `starts` as answers, each with its own step counted and, when `follow`,
    /// a chain of single steps followed. `seen` holds the paths above, which a row must not repeat.
    fn level(&mut self, starts: &[Start], seen: &mut Vec<String>, depth: u32, follow: bool) -> Vec<Value> {
        let rows = self.merged(starts);
        let many = self.many(&rows);
        let mut out = Vec::with_capacity(rows.len());
        for row in rows.iter() {
            let own = self.merged(&row.members);
            let total = own.len();
            let count = own.iter().filter(|r| self.is_signal(r)).count();
            let looped = row.paths.iter().any(|path| seen.contains(path));
            let mut answer = json!({
                "path": row.path,
                "signal": row.signal,
                "location": row.location,
                "sites": row.sites.iter().map(|(location, kind)| json!({ "location": location, "kind": kind })).collect::<Vec<_>>(),
                "constant": row.constant,
                "loop": looped,
                "count": count,
                "total": total,
                "paths": row.paths,
                "from": row.from,
            });
            if follow && !looped && !row.constant && !many && depth < FOLLOW_DEPTH && count == 1 {
                let above = seen.len();
                seen.extend(row.paths.iter().cloned());
                answer["next"] = Value::Array(self.level(&row.members, seen, depth + 1, true));
                seen.truncate(above);
            }
            out.push(answer);
        }
        out
    }
}

/// An entry's path without the instance its signal is in and the `.` or `::` after it.
fn local_of<'a>(signal: &str, path: &'a str) -> &'a str {
    let instance = instance_of(signal);
    if instance.is_empty() {
        return path;
    }
    let rest = path.strip_prefix(instance).unwrap_or(path);
    rest.strip_prefix("::").or_else(|| rest.strip_prefix('.')).unwrap_or(rest)
}

/// The instance a path is in: up to its last `.`, else its last `::`.
fn instance_of(path: &str) -> &str {
    path.rfind('.').or_else(|| path.rfind("::")).map_or("", |at| &path[..at])
}

/// `svtrace.step`.
pub fn step(graph: &Graph, argument: &Map<String, Value>) -> Result<Value, CommandError> {
    crate::deep(|| step_deep(graph, argument))
}

fn step_deep(graph: &Graph, argument: &Map<String, Value>) -> Result<Value, CommandError> {
    let question = question(argument, trace::STEP)?;
    let starts = starts(graph, &question)?;
    let mut seen: Vec<String> = starts.iter().map(|start| start.path.clone()).collect();
    match argument.get("ancestors") {
        None | Some(Value::Null) => {}
        Some(Value::Array(paths)) => {
            for path in paths {
                seen.push(path.as_str().ok_or("svtrace.step: ancestors are paths")?.to_string());
            }
        }
        Some(_) => return Err("svtrace.step: ancestors are paths".to_string()),
    }
    let follow = match argument.get("follow") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(follow)) => *follow,
        Some(_) => return Err("svtrace.step: follow is true or false".to_string()),
    };
    let mut walker = Walker::new(graph, question.direction, question.prefer);
    Ok(Value::Array(walker.level(&starts, &mut seen, 1, follow)))
}

/// A signal right under the top module: with nothing further, it is where the design's edge is.
fn at_top(path: &str) -> bool {
    !path.contains(':') && path.matches('.').count() == 1 && !path.starts_with('.') && !path.ends_with('.')
}

/// `svtrace.endpoints`: the walk `collect_endpoints` does in `slang/trace.lua`. From the signal, breadth
/// first, an entry is followed while one of its sites is `port` or `wire`; each `data` site (`control` and
/// `clock` too, for loads) is an endpoint, and for drivers each constant; one per file and line.
pub fn endpoints(graph: &Graph, argument: &Map<String, Value>) -> Result<Value, CommandError> {
    crate::deep(|| endpoints_deep(graph, argument))
}

fn endpoints_deep(graph: &Graph, argument: &Map<String, Value>) -> Result<Value, CommandError> {
    let question = question(argument, trace::ENDPOINTS)?;
    let starts = starts(graph, &question)?;
    let loads = question.direction == Direction::Loads;
    let mut walker = Walker::new(graph, question.direction, question.prefer);
    let mut lines: HashSet<String> = HashSet::new();
    let mut found: Vec<Value> = Vec::new();
    let mut add = |location: &Value| {
        let key = format!("{}:{}", location["uri"], location["range"]["start"]["line"]);
        if lines.insert(key) {
            found.push(location.clone());
        }
    };
    // The walk of each start in turn, one list (a line once).
    for Start { id: root, bits: root_bits, path } in &starts {
        let root_signal = if root_bits.is_some() { graph.symbols[*root].path.clone() } else { path.clone() };
        let mut seen: HashSet<String> = HashSet::from([path.clone()]);
        // (The location is the declaration of an entry reached on the way; the signal asked for has none.)
        let mut frontier: Vec<(SymId, Bits, String, Option<Value>)> = vec![(*root, *root_bits, root_signal, None)];
        while !frontier.is_empty() {
            let mut following = Vec::new();
            for (id, bits, signal, declaration) in &frontier {
                // (A signal that can't be asked about, such as one nothing reads, is passed over.)
                if loads && graph.used_by(*id).is_empty() {
                    continue;
                }
                let step = walker.step(*id, *bits);
                for entry in &step.entries {
                    if entry.constant {
                        continue;
                    }
                    for (location, kind) in &entry.sites {
                        match *kind {
                            "port" | "wire" => {
                                if seen.insert(entry.path.clone()) {
                                    following.push((entry.id, entry.next_bits, entry.signal.clone(), Some(entry.location.clone())));
                                }
                            }
                            "data" => add(location),
                            "control" | "clock" if loads => add(location),
                            _ => {}
                        }
                    }
                }
                for location in &step.constants {
                    add(location);
                }
                if step.entries.is_empty() && step.constants.is_empty() && at_top(signal) {
                    if let Some(declaration) = declaration {
                        add(declaration);
                    }
                }
            }
            frontier = following;
        }
    }
    Ok(Value::Array(found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_of_paths() {
        assert_eq!(instance_of("top.u[0].d"), "top.u[0]");
        assert_eq!(instance_of("pkg::x"), "pkg");
        assert_eq!(instance_of("top"), "");
    }

    #[test]
    fn the_top_of_the_design() {
        assert!(at_top("top.clk"));
        assert!(!at_top("top.u.clk"));
        assert!(!at_top("pkg::x"));
        assert!(!at_top("top"));
    }
}

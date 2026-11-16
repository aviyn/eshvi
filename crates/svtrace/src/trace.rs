//! The drivers and loads of a signal, found the way slang-server's cone tracers find them
//! (`ConeTracer.h` in the cone-connection-sites branch of the fork): each assignment or port map
//! that writes or reads the signal is walked, and every signal on its other side is an endpoint,
//! with the places in the source where the two connect.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{json, Value};

use crate::ast::{Addr, Connection, Direction, Node, NodeKind, Port, Range, ValueKind};
use crate::bits::{self, overlaps, ptr, Bits, Fed};
use crate::graph::{Container, Graph, SymId, Target};

/// How a signal takes part in the connection a site was found on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteKind {
    /// Read as a value in the connecting assignment.
    Data,
    /// Copied whole to the other signal, its only assignment there, by logic that has no
    /// condition or clock, so the two are effectively one wire.
    Wire,
    /// Decides whether the connecting assignment executes.
    Control,
    /// Connected through an instance's port map, without logic in the connection.
    Port,
    /// Read in the event control of the block that assigns (`@(posedge clk or negedge rst_n)`).
    Clock,
    /// Assigned a value that reads no signal.
    Constant,
    /// Assigned the result of a call that is given no signal, so what it reads can't be seen here.
    Call,
}

impl SiteKind {
    fn name(self) -> &'static str {
        match self {
            SiteKind::Data => "data",
            SiteKind::Wire => "wire",
            SiteKind::Control => "control",
            SiteKind::Port => "port",
            SiteKind::Clock => "clock",
            SiteKind::Constant => "constant",
            SiteKind::Call => "call",
        }
    }
}

/// Where an endpoint connects to the traced signal: the use of the signal being traced from, in the
/// assignment or port connection that links the two (the value assigned, for a constant).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Site {
    range: Option<Range>,
    kind: SiteKind,
    /// The bits of the endpoint that this connection involves; None is all of them.
    bits: Bits,
    /// The statement that holds the read: the innermost assignment, continuous assign or port connection,
    /// or the condition of an `if` or `case`, or an event control. An endpoint has one site of a kind per
    /// statement, the first by position; None (no place to tell it by) is a statement of its own.
    statement: Option<Range>,
}

/// What tracing a signal found.
#[derive(Default)]
pub(crate) struct Cone {
    leaves: BTreeMap<SymId, BTreeSet<Site>>,
    /// Where the traced signal is assigned a value that no signal feeds, so no leaf stands for it.
    constants: BTreeSet<Site>,
    /// The cone is for some bits of the signal: leaves are told apart by the bits they supply.
    narrow: bool,
}

/// One endpoint of a cone, as the answers list it.
pub(crate) struct Leaf {
    pub id: SymId,
    pub path: String,
    pub location: Value,
    /// The sites in source order, each a `Location` and the name of its kind.
    pub sites: Vec<(Value, &'static str)>,
    /// The bits of the endpoint the sites involve (the whole signal unless the cone was asked for bits).
    pub bits: Bits,
}

impl Cone {
    pub(crate) fn leaves(&self, graph: &Graph) -> Vec<Leaf> {
        leaf_list(graph, self)
    }

    /// The places the traced signal is assigned a value no signal feeds.
    pub(crate) fn constant_locations(&self, graph: &Graph) -> Vec<Value> {
        site_answers(graph, &self.constants).into_iter().map(|site| site.location).collect()
    }
}

/// An error for the client, with the text slang-server gives.
pub type CommandError = String;

pub const COMMANDS: [&str; 6] = [
    "slang.getDriversWithLocation",
    "slang.getLoadsWithLocation",
    "slang.getDriversWithConstants",
    STEP,
    ENDPOINTS,
    AT_CURSOR,
];

pub const STEP: &str = "svtrace.step";
pub const ENDPOINTS: &str = "svtrace.endpoints";
pub const AT_CURSOR: &str = "svtrace.atCursor";

/// Answers one of `COMMANDS` given the command's arguments, as `workspace/executeCommand` carries
/// them: a signal's path for the three slang-server commands, an object for `svtrace.*`.
pub fn run(graph: &Graph, command: &str, arguments: &[Value]) -> Result<Value, CommandError> {
    crate::deep(|| run_deep(graph, command, arguments))
}

fn run_deep(graph: &Graph, command: &str, arguments: &[Value]) -> Result<Value, CommandError> {
    match command {
        AT_CURSOR => match arguments {
            [Value::Object(argument)] => crate::cursor::at_cursor(graph, argument),
            [_] | [] => Err(format!("{command} takes {{ textDocument, position }} as its argument")),
            _ => Err("Expected 0 or 1 argument for command".to_string()),
        },
        STEP | ENDPOINTS => match arguments {
            [Value::Object(argument)] => {
                if command == STEP {
                    crate::walk::step(graph, argument)
                } else {
                    crate::walk::endpoints(graph, argument)
                }
            }
            [_] | [] => Err(format!("{command} takes {{ path, direction }} as its argument")),
            _ => Err("Expected 0 or 1 argument for command".to_string()),
        },
        _ => match arguments {
            [Value::String(path)] => execute(graph, command, path),
            [_] | [] => Err(format!("{command} takes the path of a signal as its argument")),
            _ => Err("Expected 0 or 1 argument for command".to_string()),
        },
    }
}

/// Answers one of the three slang-server commands for the signal at `path`.
pub fn execute(graph: &Graph, command: &str, path: &str) -> Result<Value, CommandError> {
    crate::deep(|| execute_deep(graph, command, path))
}

fn execute_deep(graph: &Graph, command: &str, path: &str) -> Result<Value, CommandError> {
    match command {
        "slang.getDriversWithLocation" => Ok(entries(graph, &drivers(graph, path)?)),
        "slang.getDriversWithConstants" => {
            let cone = drivers(graph, path)?;
            Ok(json!({ "drivers": entries(graph, &cone), "constants": sites(graph, &cone.constants) }))
        }
        "slang.getLoadsWithLocation" => Ok(entries(graph, &loads(graph, path)?)),
        _ => Err(format!("Unknown command: {command}")),
    }
}

pub(crate) fn lookup(graph: &Graph, path: &str) -> Result<Target, CommandError> {
    graph.lookup(path).ok_or_else(|| format!("Could not find path in compiled design: {path}"))
}

fn drivers(graph: &Graph, path: &str) -> Result<Cone, CommandError> {
    let Target::Value(root) = lookup(graph, path)? else {
        return Err(format!("Path is not a value symbol: {path}"));
    };
    Ok(drivers_of(graph, root, None))
}

pub(crate) fn drivers_of(graph: &Graph, root: SymId, asked: Bits) -> Cone {
    let mut tracer = Drivers::new(graph, root, asked);
    for &index in graph.driven_by(root) {
        tracer.container(&graph.containers[index]);
    }
    tracer.base.cone.narrow = asked.is_some();
    tracer.base.cone
}

fn loads(graph: &Graph, path: &str) -> Result<Cone, CommandError> {
    let not_found = || format!("Could not find reference to: {path}");
    let Target::Value(root) = lookup(graph, path)? else {
        return Err(not_found());
    };
    loads_of(graph, root, None).ok_or_else(not_found)
}

/// None when nothing reads the signal, which the command reports as an error.
pub(crate) fn loads_of(graph: &Graph, root: SymId, asked: Bits) -> Option<Cone> {
    if graph.used_by(root).is_empty() {
        return None;
    }
    let mut tracer = Loads::new(graph, root, asked);
    for &index in graph.used_by(root) {
        tracer.container(&graph.containers[index]);
    }
    tracer.base.cone.narrow = asked.is_some();
    Some(tracer.base.cone)
}

/// The state both tracers keep while walking an assignment or port map.
struct Base<'g> {
    graph: &'g Graph,
    root: SymId,
    /// The bits of the root asked about; None is the whole signal, and nothing is narrowed.
    asked: Bits,
    cone: Cone,
    /// Whether the current expression is an assignment target.
    is_lhs: bool,
    /// Whether visited assignments are neither clocked nor latched.
    in_combinational: bool,
    /// Whether visited statements only execute under a condition.
    under_condition: bool,
    /// Whether visited value expressions are routed by an input port's connection.
    in_port_connection: bool,
    /// Whether visited value expressions are copied whole to the assignment's target.
    in_wire: bool,
    /// How many assignments in the current combinational block write each signal.
    block_writes: HashMap<Addr, u32>,
    /// The statement being walked (`Site::statement`).
    statement: Option<Range>,
}

impl<'g> Base<'g> {
    fn new(graph: &'g Graph, root: SymId, asked: Bits) -> Self {
        Base {
            graph,
            root,
            asked,
            cone: Cone::default(),
            is_lhs: false,
            in_combinational: false,
            under_condition: false,
            in_port_connection: false,
            in_wire: false,
            block_writes: HashMap::new(),
            statement: None,
        }
    }

    /// A site of the statement being walked.
    fn site(&self, range: Option<Range>, kind: SiteKind, bits: Bits) -> Site {
        Site { range, kind, bits, statement: self.statement }
    }

    fn read_kind(&self) -> SiteKind {
        if self.in_port_connection {
            SiteKind::Port
        } else if self.in_wire {
            SiteKind::Wire
        } else {
            SiteKind::Data
        }
    }

    fn add_leaf(&mut self, leaf: Option<SymId>, site: Site) {
        // (A symbol the dump doesn't describe, such as a loop variable, has nothing to report.)
        let Some(leaf) = leaf else { return };
        if !matches!(self.graph.symbols[leaf].kind, ValueKind::Parameter | ValueKind::Specparam) {
            self.cone.leaves.entry(leaf).or_default().insert(site);
        }
    }

    /// Records a value that no signal feeds, which no leaf can stand for.
    fn add_if_no_signal(&mut self, value: &Node) {
        let sources = self.sources(value);
        if !sources.signal {
            let kind = if sources.call { SiteKind::Call } else { SiteKind::Constant };
            self.cone.constants.insert(self.site(value.range, kind, None));
        }
    }

    /// What the value of an expression comes from.
    fn sources(&self, node: &Node) -> Sources {
        let mut sources = Sources::default();
        self.collect_sources(node, &mut sources);
        sources
    }

    fn collect_sources(&self, node: &Node, sources: &mut Sources) {
        match &node.kind {
            NodeKind::Value { addr, .. } => {
                sources.signal = sources.signal || !self.is_constant(*addr);
            }
            kind => {
                sources.call = sources.call || *kind == NodeKind::Call;
                for child in &node.children {
                    self.collect_sources(child, sources);
                }
            }
        }
    }

    fn is_constant(&self, addr: Addr) -> bool {
        self.graph.symbol(addr).is_some_and(|id| self.graph.is_constant(id))
    }

    /// For a driver trace of some bits: what feeds the bits of the root that an assignment (or an
    /// output port's connection, `port`) writes.
    fn drivers_fed(&self, assign: &Node, port: Option<Option<SymId>>) -> Fed {
        let (Some(asked), Some(left)) = (self.asked, assign.children.first()) else { return Fed::default() };
        let right = assign.children.get(1);
        let rhs = match port {
            Some(Some(port)) => vec![self.graph.whole_seg(port)],
            Some(None) => Vec::new(),
            None => right.map(|right| self.graph.segs(right, false)).unwrap_or_default(),
        };
        if let Some(link) = bits::link(self.graph.segs(left, true), rhs) {
            let positions = bits::wanted(&link.lhs, self.root, asked, self.graph);
            return bits::fed(&link.rhs, &positions, self.graph);
        }
        let mut fed = self.everything(right);
        if let Some(Some(port)) = port {
            fed.synthetic.push((port, None));
        }
        fed
    }

    /// For a driver trace of some bits of an input port: what the connection feeds them with.
    fn port_fed(&self, expr: &Node) -> Fed {
        let Some(asked) = self.asked else { return Fed::default() };
        let rhs = self.graph.segs(expr, false);
        if let Some(link) = bits::link(vec![self.graph.whole_seg(self.root)], rhs) {
            let positions = bits::wanted(&link.lhs, self.root, asked, self.graph);
            return bits::fed(&link.rhs, &positions, self.graph);
        }
        self.everything(Some(expr))
    }

    /// Every use in an expression, whole: the answer when the two sides can't be paired.
    fn everything(&self, expr: Option<&Node>) -> Fed {
        let Some(expr) = expr else { return Fed::default() };
        let mut values = Vec::new();
        bits::collect_values(expr, false, &mut values);
        let sources = self.sources(expr);
        let mut fed = Fed::everything(self.graph, &values, sources.call);
        fed.signal = sources.signal;
        fed
    }

    /// A connection that only routes bits to the port: anything else is logic.
    fn is_wiring(&self, node: &Node) -> bool {
        match &node.kind {
            NodeKind::Value { .. } | NodeKind::Literal => true,
            NodeKind::Select(_) => {
                let Some((value, indices)) = node.children.split_first() else { return false };
                indices.iter().all(|index| !self.sources(index).signal) && self.is_wiring(value)
            }
            NodeKind::Member { .. } => node.children.iter().all(|value| self.is_wiring(value)),
            NodeKind::Concat => node.children.iter().all(|operand| self.is_wiring(operand)),
            NodeKind::Replicate => node.children.last().is_some_and(|concat| self.is_wiring(concat)),
            NodeKind::Convert { implicit } => *implicit && node.children.iter().all(|operand| self.is_wiring(operand)),
            _ => false,
        }
    }

    fn is_wire(&self, assign: &Node) -> bool {
        self.in_combinational && !self.under_condition && self.is_plain_copy(assign) && self.is_only_write(assign)
    }

    /// Only a whole signal on each side: a select, an expression or a change of width is logic, and
    /// a parameter or an enum value isn't a signal.
    fn is_plain_copy(&self, assign: &Node) -> bool {
        let NodeKind::Assign { compound: false, .. } = assign.kind else {
            return false;
        };
        let (Some(left), Some(right)) = (assign.children.first(), assign.children.get(1)) else {
            return false;
        };
        let NodeKind::Value { width: left_width, .. } = &left.kind else {
            return false;
        };
        let mut source = right;
        while let NodeKind::Convert { implicit } = source.kind {
            match source.children.first() {
                Some(operand) if implicit => source = operand,
                _ => return false,
            }
        }
        match &source.kind {
            NodeKind::Value { addr, width } => {
                !self.is_constant(*addr) && self.graph.width(width) == self.graph.width(left_width)
            }
            _ => false,
        }
    }

    /// A default assignment that later statements replace doesn't make the two signals one.
    fn is_only_write(&self, assign: &Node) -> bool {
        let Some(NodeKind::Value { addr, .. }) = assign.children.first().map(|left| &left.kind) else {
            return true;
        };
        self.block_writes.get(&self.concrete(*addr)).is_none_or(|&writes| writes == 1)
    }

    /// The address that identifies a symbol however it is reached: a modport's port is the
    /// interface's own signal.
    fn concrete(&self, addr: Addr) -> Addr {
        self.graph.symbol(addr).map_or(addr, |id| self.graph.symbols[id].addr)
    }

    /// Counts the assignments to each signal in a block.
    fn count_writes(&mut self, node: &Node) {
        if matches!(node.kind, NodeKind::Assign { .. }) {
            if let Some(target) = node.children.first() {
                self.count_target(target);
            }
        }
        for child in &node.children {
            self.count_writes(child);
        }
    }

    fn count_target(&mut self, target: &Node) {
        if target.kind == NodeKind::Concat {
            for operand in &target.children {
                self.count_target(operand);
            }
        } else if let Some(symbol) = self.symbol_reference(target) {
            let key = self.concrete(symbol);
            *self.block_writes.entry(key).or_insert(0) += 1;
        }
    }

    /// The symbol an expression stands for: a signal, one of its bits, or one of its fields.
    fn symbol_reference(&self, node: &Node) -> Option<Addr> {
        match &node.kind {
            NodeKind::Value { addr, .. } => Some(*addr),
            NodeKind::Member { field } => Some(*field),
            NodeKind::Select(_) => self.symbol_reference(node.children.first()?),
            NodeKind::Convert { implicit: true } => self.symbol_reference(node.children.first()?),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Sources {
    signal: bool,
    call: bool,
}

/// The walk both tracers share; each decides what a value, an assignment, an `if`, a `case` and an
/// instance mean.
trait Tracer<'g> {
    fn base(&mut self) -> &mut Base<'g>;
    fn value(&mut self, node: &Node);
    fn assign(&mut self, node: &Node);
    fn conditional(&mut self, node: &Node);
    fn case(&mut self, node: &Node);
    /// A loop: its condition is a `control` source of what the rest of it assigns.
    fn looped(&mut self, node: &Node);
    /// A statement that isn't an assignment or a branch (a block, a call, an assertion, a declaration):
    /// what it reads is not a source of the statements after it.
    fn statement(&mut self, node: &Node);
    /// A select on the left of an assignment: the value is the target; the indices are `control` sources
    /// of it (they pick what is written), whatever the select's own value is.
    fn lhs_select(&mut self, node: &Node);
    /// A statement under an event control: `children[0]` is the event list, which is not read at all
    /// when it is a list of levels (`@(a or b)`: when to run, not a clock).
    fn timed(&mut self, node: &Node);
    fn instance(&mut self, connections: &[Connection]);
    /// What a new assignment or port map starts with.
    fn reset(&mut self) {}

    fn visit(&mut self, node: &Node) {
        match node.kind {
            NodeKind::Value { .. } => self.value(node),
            NodeKind::Assign { .. } => self.assign(node),
            NodeKind::If { .. } => self.conditional(node),
            NodeKind::Case => self.case(node),
            NodeKind::Timed { .. } => self.timed(node),
            NodeKind::Loop { .. } => self.looped(node),
            NodeKind::Stmt => self.statement(node),
            // An index on the left only picks which bits are written: it isn't a target.
            NodeKind::Select(_) if self.base().is_lhs => self.lhs_select(node),
            _ => self.visit_children(node),
        }
    }

    fn visit_children(&mut self, node: &Node) {
        for child in &node.children {
            self.visit(child);
        }
    }

    fn visit_assigned_value(&mut self, assign: &Node) {
        let wire = self.base().is_wire(assign);
        self.base().in_wire = wire;
        if let Some(right) = assign.children.get(1) {
            self.visit(right);
        }
        self.base().in_wire = false;
    }

    fn container(&mut self, container: &Container) {
        self.reset();
        match container {
            Container::Assign(assign) => {
                self.base().in_combinational = true;
                self.visit(assign);
                self.base().in_combinational = false;
            }
            Container::Procedural { combinational, body } => {
                self.base().in_combinational = *combinational;
                if *combinational {
                    self.base().block_writes.clear();
                    self.base().count_writes(body);
                }
                self.visit(body);
                self.base().in_combinational = false;
                self.base().block_writes.clear();
            }
            Container::Instance(connections) => self.instance(connections),
        }
    }
}

/// The signals that feed the root.
struct Drivers<'g> {
    base: Base<'g>,
    /// Signals that control whether the current assignment executes, and where they are read.
    condition_drivers: BTreeMap<SymId, BTreeSet<Site>>,
    /// Whether visited value expressions should be recorded as drivers of the root.
    is_driven: bool,
    /// Whether visited value expressions control which assignments execute.
    in_condition: bool,
    /// Whether visited value expressions are in an event control.
    in_clock: bool,
    /// The signals in the event controls the current statement is under, and where they are read.
    clock_drivers: BTreeMap<SymId, BTreeSet<Site>>,
    /// Where the current assignment writes the root.
    root_range: Option<Range>,
    /// How many times the root has been found on the left of an assignment.
    hits: usize,
    /// Whether visited value expressions are the index of a target.
    in_index: bool,
    /// The port whose output connection is being visited: Some(its internal signal).
    port: Option<Option<SymId>>,
    /// While the value of a connection that writes some asked bits is visited: what feeds them.
    feed: Option<Fed>,
}

impl<'g> Drivers<'g> {
    fn new(graph: &'g Graph, root: SymId, asked: Bits) -> Self {
        Drivers {
            base: Base::new(graph, root, asked),
            condition_drivers: BTreeMap::new(),
            is_driven: false,
            in_condition: false,
            in_clock: false,
            clock_drivers: BTreeMap::new(),
            root_range: None,
            hits: 0,
            in_index: false,
            port: None,
            feed: None,
        }
    }

    /// Records that the root is assigned a value, when no signal feeds the bits asked about.
    fn constant(&mut self, value: &Node) {
        match &self.feed {
            None => self.base.add_if_no_signal(value),
            Some(fed) if !fed.signal => {
                let kind = if fed.call { SiteKind::Call } else { SiteKind::Constant };
                let site = self.base.site(value.range, kind, None);
                self.base.cone.constants.insert(site);
            }
            Some(_) => {}
        }
    }

    fn visit_conditions(&mut self, nodes: &[Node]) {
        self.in_condition = true;
        // (The condition is a statement: a signal read twice in it is one site. A case item's list of
        // expressions is one condition.)
        let outer = std::mem::replace(&mut self.base.statement, span_of(nodes));
        for node in nodes {
            self.visit(node);
        }
        self.base.statement = outer;
        self.in_condition = false;
    }
}

impl<'g> Tracer<'g> for Drivers<'g> {
    fn base(&mut self) -> &mut Base<'g> {
        &mut self.base
    }

    fn reset(&mut self) {
        self.clock_drivers.clear();
    }

    fn timed(&mut self, node: &Node) {
        let Some((timing, statements)) = node.children.split_first() else { return };
        let saved = self.clock_drivers.clone();
        if node.kind != (NodeKind::Timed { level: true }) {
            self.in_clock = true;
            let outer = std::mem::replace(&mut self.base.statement, timing.range);
            self.visit(timing);
            self.base.statement = outer;
            self.in_clock = false;
        }
        for statement in statements {
            self.visit(statement);
        }
        self.clock_drivers = saved;
    }

    fn statement(&mut self, node: &Node) {
        self.visit_children(node);
    }

    fn lhs_select(&mut self, node: &Node) {
        let Some((value, indices)) = node.children.split_first() else { return };
        let hits = self.hits;
        self.visit(value);
        if self.hits != hits {
            self.base.is_lhs = false;
            self.in_index = true;
            for index in indices {
                self.visit(index);
            }
            self.in_index = false;
            self.base.is_lhs = true;
        }
    }

    fn looped(&mut self, node: &Node) {
        let NodeKind::Loop { init, conds } = node.kind else { return };
        let saved = (self.condition_drivers.clone(), self.clock_drivers.clone(), self.base.under_condition);
        for statement in &node.children[..init] {
            self.visit(statement);
        }
        self.base.under_condition = true;
        self.visit_conditions(&node.children[init..init + conds]);
        for statement in &node.children[init + conds..] {
            self.visit(statement);
        }
        (self.condition_drivers, self.clock_drivers, self.base.under_condition) = saved;
    }

    fn value(&mut self, node: &Node) {
        let NodeKind::Value { addr, .. } = node.kind else { return };
        let symbol = self.base.graph.symbol(addr);
        if self.in_clock {
            if let Some(symbol) = symbol {
                let site = self.base.site(node.range, SiteKind::Clock, node.span);
                self.clock_drivers.entry(symbol).or_default().insert(site);
            }
            return;
        }
        if self.base.is_lhs && symbol == Some(self.base.root) {
            if overlaps(node.span, self.base.asked) {
                self.is_driven = true;
                self.hits += 1;
                self.root_range = node.range;
            }
        } else if !self.base.is_lhs && self.is_driven {
            let bits = match &self.feed {
                _ if self.in_index => Some(node.span),
                Some(fed) => fed.nodes.get(&ptr(node)).copied(),
                None => Some(node.span),
            };
            if let Some(bits) = bits {
                let kind = if self.in_index { SiteKind::Control } else { self.base.read_kind() };
                let site = self.base.site(node.range, kind, bits);
                self.base.add_leaf(symbol, site);
            }
        }
        if self.in_condition {
            if let Some(symbol) = symbol {
                let site = self.base.site(node.range, SiteKind::Control, node.span);
                self.condition_drivers.entry(symbol).or_default().insert(site);
            }
        }
    }

    fn assign(&mut self, node: &Node) {
        let NodeKind::Assign { compound, .. } = node.kind else { return };
        // (An assignment can be inside an expression, `y = (b = c) + d`: what is around it goes on after it.)
        let (outer_driven, outer_lhs, outer_range) = (self.is_driven, self.base.is_lhs, self.root_range);
        let outer_statement = std::mem::replace(&mut self.base.statement, node.range);
        let mut outer_feed = self.feed.take();
        self.is_driven = false;
        self.base.is_lhs = true;
        if let Some(left) = node.children.first() {
            self.visit(left);
        }
        self.base.is_lhs = false;
        if !self.is_driven {
            // Not a write of the root. Its value is read by whatever it is part of, and an assignment
            // inside it may be the root's.
            if let Some(right) = node.children.get(1) {
                (self.is_driven, self.feed) = (outer_driven, outer_feed.take());
                self.visit(right);
                outer_feed = self.feed.take();
            }
        } else {
            self.feed = self.base.asked.map(|_| self.base.drivers_fed(node, self.port));
            self.visit_assigned_value(node);
            if let Some(port) = self.port {
                let bits = match &self.feed {
                    Some(fed) => fed.synthetic.first().map(|(_, bits)| *bits),
                    None => Some(None),
                };
                if let Some(bits) = bits {
                    let site = self.base.site(self.root_range, SiteKind::Port, bits);
                    self.base.add_leaf(port, site);
                }
            // (A compound assignment reads the root, which its right side doesn't show.)
            } else if !compound {
                if let Some(right) = node.children.get(1) {
                    self.constant(right);
                }
            }
            self.feed = None;
            for (&driver, sites) in self.condition_drivers.clone().iter().chain(&self.clock_drivers.clone()) {
                for site in sites {
                    self.base.add_leaf(Some(driver), site.clone());
                }
            }
        }
        (self.is_driven, self.base.is_lhs, self.root_range, self.feed) = (outer_driven, outer_lhs, outer_range, outer_feed);
        self.base.statement = outer_statement;
    }

    fn conditional(&mut self, node: &Node) {
        let NodeKind::If { conditions, .. } = node.kind else { return };
        let saved = (self.condition_drivers.clone(), self.clock_drivers.clone(), self.base.under_condition);
        self.base.under_condition = true;
        self.visit_conditions(&node.children[..conditions]);
        for branch in &node.children[conditions..] {
            self.visit(branch);
        }
        (self.condition_drivers, self.clock_drivers, self.base.under_condition) = saved;
    }

    fn case(&mut self, node: &Node) {
        let saved = (self.condition_drivers.clone(), self.clock_drivers.clone(), self.base.under_condition);
        self.base.under_condition = true;
        let Some((selector, items)) = node.children.split_first() else { return };
        self.visit_conditions(std::slice::from_ref(selector));
        for item in items {
            match item.kind {
                NodeKind::CaseItem { expressions } => {
                    self.visit_conditions(&item.children[..expressions]);
                    for statement in &item.children[expressions..] {
                        self.visit(statement);
                    }
                }
                _ => self.visit(item),
            }
        }
        (self.condition_drivers, self.clock_drivers, self.base.under_condition) = saved;
    }

    fn instance(&mut self, connections: &[Connection]) {
        for connection in connections {
            let (Port::Port { direction, internal }, Some(expr)) = (&connection.port, &connection.expr) else {
                continue;
            };
            let internal = internal.and_then(|addr| self.base.graph.symbol(addr));
            // (A port connection is a statement.)
            let outer_statement = std::mem::replace(&mut self.base.statement, expr.range);
            // What is connected feeds the port (an input, and an `inout` or `ref` port, which goes both ways)...
            if *direction != Direction::Out && internal == Some(self.base.root) {
                let outside = outside(*direction, expr);
                self.is_driven = true;
                self.base.in_port_connection = self.base.is_wiring(outside);
                self.feed = self.base.asked.map(|_| self.base.port_fed(outside));
                self.visit(outside);
                self.constant(outside);
                self.feed = None;
                self.base.in_port_connection = false;
                self.is_driven = false;
            }
            // ...and the port feeds what is connected (an output, `inout`, `ref`).
            if *direction != Direction::In {
                self.port = Some(internal);
                self.visit(expr);
                self.port = None;
            }
            self.base.statement = outer_statement;
        }
    }
}

/// The signals the root feeds.
struct Loads<'g> {
    base: Base<'g>,
    /// Whether visited value expressions decide which assignments execute.
    in_condition: bool,
    /// Whether visited value expressions are in an event control.
    in_clock: bool,
    /// Whether the current assignment is the connection of the root, which is an output port.
    in_root_output_connection: bool,
    /// Whether visited value expressions are the index of a target.
    in_index: bool,
    /// Whether the value of an assignment is being visited (an assignment found now is inside an expression).
    in_value: bool,
    /// Where the root is read on the current data or control path.
    root_sites: Vec<RootSite>,
    /// While the target of an assignment is visited for some asked bits: where they go.
    lhs: Option<LhsFilter>,
}

/// A place the root is read, with the use it is.
#[derive(Clone)]
struct RootSite {
    site: Site,
    node: usize,
}

/// Which targets the sites of the current assignment reach.
struct LhsFilter {
    /// The sites before this many come from conditions around the assignment: they reach every target.
    outer: usize,
    /// For each later site: the targets it reaches and the bits of them.
    targets: Vec<Fed>,
    /// For the connection of the root, an output port: the same.
    port: Option<Fed>,
}

impl<'g> Loads<'g> {
    fn new(graph: &'g Graph, root: SymId, asked: Bits) -> Self {
        Loads {
            base: Base::new(graph, root, asked),
            in_condition: false,
            in_clock: false,
            in_root_output_connection: false,
            in_index: false,
            in_value: false,
            root_sites: Vec::new(),
            lhs: None,
        }
    }

    /// Which targets of an assignment the new reads of the root (from `outer` on) reach.
    fn filter(&self, assign: &Node, outer: usize) -> LhsFilter {
        let graph = self.base.graph;
        let Some(asked) = self.base.asked else { return LhsFilter { outer, targets: Vec::new(), port: None } };
        let (Some(left), right) = (assign.children.first(), assign.children.get(1)) else {
            return LhsFilter { outer, targets: Vec::new(), port: None };
        };
        let everything = || {
            let mut values = Vec::new();
            bits::collect_values(left, true, &mut values);
            Fed::everything(graph, &values, false)
        };
        let port = self.in_root_output_connection;
        let rhs = if port { vec![graph.whole_seg(self.base.root)] } else { right.map(|r| graph.segs(r, false)).unwrap_or_default() };
        let link = bits::link(graph.segs(left, true), rhs);
        if port {
            let port = Some(match &link {
                Some(link) => bits::fed(&link.lhs, &bits::wanted(&link.rhs, self.base.root, asked, graph), graph),
                None => everything(),
            });
            return LhsFilter { outer, targets: Vec::new(), port };
        }
        let targets = self.root_sites[outer..]
            .iter()
            .map(|site| match &link {
                Some(link) => {
                    let positions = bits::wanted_at(&link.rhs, site.node, self.base.root, asked);
                    bits::fed(&link.lhs, &positions, graph)
                }
                None => everything(),
            })
            .collect();
        LhsFilter { outer, targets, port: None }
    }

    /// Whether the root is read in an index of the assignment's target.
    fn indexes_root(&self, assign: &Node) -> bool {
        fn indices<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
            if matches!(node.kind, NodeKind::Select(_)) {
                out.extend(node.children.iter().skip(1));
            }
            for child in &node.children {
                indices(child, out);
            }
        }
        let Some(left) = assign.children.first() else { return false };
        let mut found = Vec::new();
        indices(left, &mut found);
        let mut values = Vec::new();
        for index in found {
            bits::collect_values(index, false, &mut values);
        }
        values.iter().any(|value| matches!(value.kind, NodeKind::Value { addr, .. } if self.base.graph.symbol(addr) == Some(self.base.root)))
    }

    fn load_kind(&self) -> SiteKind {
        if self.in_clock {
            SiteKind::Clock
        } else if self.in_index || (self.in_condition && !self.base.in_port_connection) {
            SiteKind::Control
        } else {
            self.base.read_kind()
        }
    }

    fn visit_conditions(&mut self, nodes: &[Node]) {
        self.in_condition = true;
        // (The condition is a statement: a signal read twice in it is one site. A case item's list of
        // expressions is one condition.)
        let outer = std::mem::replace(&mut self.base.statement, span_of(nodes));
        for node in nodes {
            self.visit(node);
        }
        self.base.statement = outer;
        self.in_condition = false;
    }
}

impl<'g> Tracer<'g> for Loads<'g> {
    fn base(&mut self) -> &mut Base<'g> {
        &mut self.base
    }

    // A signal read in an event control (a clock, a reset) stays on the path of everything the rest
    // of the block assigns. The fork carries it into later blocks too; here each starts clean.
    fn reset(&mut self) {
        self.root_sites.clear();
    }

    fn timed(&mut self, node: &Node) {
        let Some((timing, statements)) = node.children.split_first() else { return };
        let saved = self.root_sites.clone();
        if node.kind != (NodeKind::Timed { level: true }) {
            self.in_clock = true;
            let outer = std::mem::replace(&mut self.base.statement, timing.range);
            self.visit(timing);
            self.base.statement = outer;
            self.in_clock = false;
        }
        for statement in statements {
            self.visit(statement);
        }
        self.root_sites = saved;
    }

    fn statement(&mut self, node: &Node) {
        let saved = self.root_sites.clone();
        self.visit_children(node);
        self.root_sites = saved;
    }

    fn looped(&mut self, node: &Node) {
        let NodeKind::Loop { init, conds } = node.kind else { return };
        let saved = (self.root_sites.clone(), self.base.under_condition);
        for statement in &node.children[..init] {
            self.visit(statement);
        }
        self.base.under_condition = true;
        self.visit_conditions(&node.children[init..init + conds]);
        for statement in &node.children[init + conds..] {
            self.visit(statement);
        }
        (self.root_sites, self.base.under_condition) = saved;
    }

    fn lhs_select(&mut self, node: &Node) {
        let Some((value, indices)) = node.children.split_first() else { return };
        // Where the root is read in the indices goes with the target, and only with it.
        let kept = self.root_sites.len();
        self.base.is_lhs = false;
        self.in_index = true;
        for index in indices {
            self.visit(index);
        }
        self.in_index = false;
        self.base.is_lhs = true;
        self.visit(value);
        self.root_sites.truncate(kept);
    }

    fn value(&mut self, node: &Node) {
        let NodeKind::Value { addr, .. } = node.kind else { return };
        let symbol = self.base.graph.symbol(addr);
        let key = ptr(node);
        if self.base.is_lhs {
            if self.in_root_output_connection {
                let bits = match &self.lhs {
                    Some(filter) => filter.port.as_ref().and_then(|port| port.nodes.get(&key).copied()),
                    None => Some(node.span),
                };
                if let Some(bits) = bits {
                    let site = self.base.site(node.range, SiteKind::Port, bits);
                    self.base.add_leaf(symbol, site);
                }
            }
            for (at, root_site) in self.root_sites.clone().into_iter().enumerate() {
                let bits = match &self.lhs {
                    // (A site past the filter's targets is a read in an index of the target: it reaches it whole.)
                    Some(filter) if at >= filter.outer => match filter.targets.get(at - filter.outer) {
                        Some(target) => target.nodes.get(&key).copied(),
                        None => Some(node.span),
                    },
                    _ => Some(node.span),
                };
                if let Some(bits) = bits {
                    self.base.add_leaf(symbol, Site { bits, ..root_site.site });
                }
            }
        } else if symbol == Some(self.base.root) && overlaps(node.span, self.base.asked) {
            let site = self.base.site(node.range, self.load_kind(), None);
            self.root_sites.push(RootSite { site, node: key });
        }
    }

    fn assign(&mut self, node: &Node) {
        let outer_statement = std::mem::replace(&mut self.base.statement, node.range);
        let saved = self.root_sites.clone();
        let nested = std::mem::replace(&mut self.in_value, true);
        self.visit_assigned_value(node);
        self.in_value = nested;
        if self.in_root_output_connection || !self.root_sites.is_empty() || self.indexes_root(node) {
            self.lhs = self.base.asked.map(|_| self.filter(node, saved.len()));
            self.base.is_lhs = true;
            if let Some(left) = node.children.first() {
                self.visit(left);
            }
            self.base.is_lhs = false;
            self.lhs = None;
        }
        if nested {
            // An assignment inside an expression, `y = (b = c) + d`: what it read, the one around it reads
            // too, as a value (a plain copy to `b` is not one to `y`).
            for read in &mut self.root_sites[saved.len()..] {
                if read.site.kind == SiteKind::Wire {
                    read.site.kind = SiteKind::Data;
                }
            }
        } else {
            self.root_sites = saved;
        }
        self.base.statement = outer_statement;
    }

    fn conditional(&mut self, node: &Node) {
        let NodeKind::If { conditions, .. } = node.kind else { return };
        let saved = (self.root_sites.clone(), self.base.under_condition);
        self.base.under_condition = true;
        self.visit_conditions(&node.children[..conditions]);
        for branch in &node.children[conditions..] {
            self.visit(branch);
        }
        (self.root_sites, self.base.under_condition) = saved;
    }

    fn case(&mut self, node: &Node) {
        let saved = (self.root_sites.clone(), self.base.under_condition);
        self.base.under_condition = true;
        let Some((selector, items)) = node.children.split_first() else { return };
        self.visit_conditions(std::slice::from_ref(selector));
        for item in items {
            // An item is reached when the ones before it didn't match: their expressions stay on the path
            // (as the drivers' side keeps them).
            match item.kind {
                NodeKind::CaseItem { expressions } => {
                    self.visit_conditions(&item.children[..expressions]);
                    for statement in &item.children[expressions..] {
                        self.visit(statement);
                    }
                }
                _ => self.visit(item),
            }
        }
        (self.root_sites, self.base.under_condition) = saved;
    }

    fn instance(&mut self, connections: &[Connection]) {
        for connection in connections {
            let (Port::Port { direction, internal }, Some(expr)) = (&connection.port, &connection.expr) else {
                continue;
            };
            let internal = internal.and_then(|addr| self.base.graph.symbol(addr));
            // (A port connection is a statement.)
            let outer_statement = std::mem::replace(&mut self.base.statement, expr.range);
            // The port feeds what is connected (an output, and an `inout` or `ref` port, which goes both ways)...
            if *direction != Direction::In && internal == Some(self.base.root) {
                self.in_root_output_connection = true;
                self.visit(expr);
                self.in_root_output_connection = false;
            }
            // ...and what is connected feeds the port (an input, `inout`, `ref`).
            if *direction != Direction::Out {
                let expr = outside(*direction, expr);
                let saved = self.root_sites.clone();
                self.base.in_port_connection = self.base.is_wiring(expr);
                self.visit(expr);
                let link = match (self.base.asked, internal) {
                    (Some(_), Some(internal)) => {
                        bits::link(vec![self.base.graph.whole_seg(internal)], self.base.graph.segs(expr, false))
                    }
                    _ => None,
                };
                for root_site in self.root_sites.clone() {
                    let mut site = root_site.site;
                    if let (Some(link), Some(asked)) = (&link, self.base.asked) {
                        let positions = bits::wanted_at(&link.rhs, root_site.node, self.base.root, asked);
                        let fed = bits::fed(&link.lhs, &positions, self.base.graph);
                        match fed.synthetic.first() {
                            Some((_, bits)) => site.bits = *bits,
                            None => continue,
                        }
                    }
                    self.base.add_leaf(internal, site);
                }
                self.base.in_port_connection = false;
                self.root_sites = saved;
            }
            self.base.statement = outer_statement;
        }
    }
}

/// The place the nodes cover together, when they all have one in a file.
fn span_of(nodes: &[Node]) -> Option<Range> {
    let mut covered: Option<Range> = None;
    for node in nodes {
        let range = node.range?;
        covered = Some(match covered {
            None => range,
            Some(so_far) if so_far.file == range.file => Range { file: range.file, start: so_far.start.min(range.start), end: so_far.end.max(range.end) },
            Some(_) => return None,
        });
    }
    covered
}

/// What a port is connected to. For an output, `inout` or `ref` port the connection is `x = <the port>`
/// (see `ast.rs`), and `x` is what an `inout` or `ref` port also reads.
fn outside(direction: Direction, expr: &Node) -> &Node {
    match (direction, &expr.kind) {
        (Direction::InOut | Direction::Ref, NodeKind::Assign { .. }) => expr.children.first().unwrap_or(expr),
        _ => expr,
    }
}

/// A site's place and kind, as the answer lists it.
struct SiteAnswer {
    key: (String, u32, u32, &'static str),
    location: Value,
    statement: Option<Range>,
}

fn location(graph: &Graph, range: Range) -> Value {
    let (start_line, start_character) = graph.sources.position(range.file, range.start);
    let (end_line, end_character) = graph.sources.position(range.file, range.end);
    json!({
        "uri": graph.sources.uri(range.file),
        "range": {
            "start": { "line": start_line, "character": start_character },
            "end": { "line": end_line, "character": end_character },
        },
    })
}

/// The sites in source order. Several expansions of a macro can land in one place, so sites that
/// start at the same place with the same kind are listed once; and an endpoint has one site of a kind in a
/// statement (`Site::statement`).
fn site_answers(graph: &Graph, sites: &BTreeSet<Site>) -> Vec<SiteAnswer> {
    let mut answers: Vec<SiteAnswer> = sites
        .iter()
        .filter_map(|site| {
            let range = site.range?;
            let location = location(graph, range);
            let start = &location["range"]["start"];
            let key = (
                graph.sources.uri(range.file).to_string(),
                start["line"].as_u64()? as u32,
                start["character"].as_u64()? as u32,
                site.kind.name(),
            );
            Some(SiteAnswer { key, location, statement: site.statement })
        })
        .collect();
    answers.sort_by(|a, b| a.key.cmp(&b.key));
    answers.dedup_by(|a, b| a.key == b.key);
    // A signal read in several places of one statement (an assignment over several lines, a long condition)
    // is one site: the first, by position. (Sorted, so the first seen is it.)
    let mut seen = HashSet::new();
    answers.retain(|answer| answer.statement.is_none_or(|statement| seen.insert((statement, answer.key.3))));
    answers
}

fn sites(graph: &Graph, sites: &BTreeSet<Site>) -> Value {
    site_answers(graph, sites)
        .into_iter()
        .map(|site| json!({ "location": site.location, "kind": site.key.3 }))
        .collect()
}

/// The endpoints with a declaration to point at, in the order slang-server lists them (by where
/// the symbols were created).
fn entries(graph: &Graph, cone: &Cone) -> Value {
    leaf_list(graph, cone)
        .into_iter()
        .map(|leaf| json!({ "path": leaf.path, "location": leaf.location, "sites": site_values(leaf.sites) }))
        .collect()
}

fn site_values(sites: Vec<(Value, &'static str)>) -> Vec<Value> {
    sites.into_iter().map(|(location, kind)| json!({ "location": location, "kind": kind })).collect()
}

fn leaf_list(graph: &Graph, cone: &Cone) -> Vec<Leaf> {
    let mut leaves: Vec<_> = cone.leaves.iter().collect();
    leaves.sort_by_key(|(&id, _)| graph.symbols[id].addr);
    let mut list = Vec::new();
    for (&id, leaf_sites) in leaves {
        let symbol = &graph.symbols[id];
        let Some(loc) = symbol.loc else { continue };
        let end = crate::ast::Pos { line: loc.pos.line, col: loc.pos.col + symbol.name.len() as u32 };
        let declaration = location(graph, Range { file: loc.file, start: loc.pos, end });
        for (bits, group) in clusters(cone.narrow, leaf_sites) {
            let sites = site_answers(graph, &group).into_iter().map(|site| (site.location, site.key.3)).collect();
            list.push(Leaf { id, path: symbol.path.clone(), location: declaration.clone(), sites, bits });
        }
    }
    list
}

/// A leaf's sites by the bits they involve. A cone for the whole signal has one group; for some
/// bits, the sites whose bits overlap go together (so `mstatus_n.mie` and `mstatus_n.mpie` are two
/// endpoints, not `mstatus_n[5:3]`), and a site that involves the whole signal takes them all.
fn clusters(narrow: bool, sites: &BTreeSet<Site>) -> Vec<(Bits, BTreeSet<Site>)> {
    if !narrow || sites.iter().any(|site| site.bits.is_none()) {
        return vec![(None, sites.clone())];
    }
    let mut spans: Vec<_> = sites.iter().filter_map(|site| site.bits).collect();
    spans.sort();
    let mut merged: Vec<bits::Span> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.lo <= last.hi => last.hi = last.hi.max(span.hi),
            _ => merged.push(span),
        }
    }
    merged
        .into_iter()
        .map(|span| {
            let group = sites.iter().filter(|site| site.bits.is_some_and(|bits| bits.lo >= span.lo && bits.hi <= span.hi));
            (Some(span), group.cloned().collect())
        })
        .collect()
}

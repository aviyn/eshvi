//! The design as the traces see it: every traceable symbol with its hierarchical path, and the
//! places that assign or read signals (continuous assigns, procedural blocks, instance port maps)
//! indexed by the symbols they write and use.

use std::collections::HashMap;
use std::path::Path;

use crate::ast::{
    Addr, Connection, Design, Direction, Item, Loc, Node, NodeKind, Port, ScopeKind, Symbol, ValueKind, Width,
};
use crate::source::{Sources, Texts};

pub type SymId = usize;

pub struct SymbolInfo {
    pub addr: Addr,
    pub name: String,
    /// The path slang's `Symbol::getHierarchicalPath` gives, which slang-server hands out too.
    pub path: String,
    pub kind: ValueKind,
    /// The declared type, for bits and fields.
    pub ty: Width,
    pub loc: Option<Loc>,
    /// Declared in a statement block (a loop variable, a local): slang's name lookup can't reach it
    /// from outside, so it is an endpoint of traces but not a signal to start one from.
    pub local: bool,
}

/// What a path names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Value(SymId),
    /// An instance, a generate block, a package...: there, but not a signal.
    Other,
}

/// Something that assigns or reads signals.
pub enum Container {
    Assign(Node),
    Procedural { combinational: bool, body: Node },
    Instance(Vec<Connection>),
}

pub struct Graph {
    pub symbols: Vec<SymbolInfo>,
    pub containers: Vec<Container>,
    pub sources: Sources,
    by_addr: HashMap<Addr, SymId>,
    paths: HashMap<String, Target>,
    types: HashMap<Addr, Width>,
    /// Where each packed struct field starts, by address.
    fields: HashMap<Addr, u32>,
    /// The containers that assign each symbol, in design order.
    driven_by: HashMap<SymId, Vec<usize>>,
    /// The containers that mention each symbol, in design order.
    used_by: HashMap<SymId, Vec<usize>>,
}

impl Drop for Graph {
    /// The expression trees can be deeper than the stack the dropping thread has.
    fn drop(&mut self) {
        let containers = std::mem::take(&mut self.containers);
        crate::deep(move || drop(containers));
    }
}

impl Graph {
    /// `root` is the folder slang ran in, which the dump's file names are relative to.
    pub fn build(root: &Path, design: Design) -> Graph {
        Self::build_shared(root, design, &Texts::default())
    }

    /// [`Graph::build`], sharing the source files' text with the build's other reader.
    pub fn build_shared(root: &Path, design: Design, texts: &Texts) -> Graph {
        crate::deep(|| {
            let mut builder = Builder::default();
            let unit = Scope { path: String::new(), root_like: true, package: false, array: Vec::new(), lower: None, local: false };
            builder.members(design.root, &unit);
            builder.finish(root, design.files, design.types, design.fields, texts)
        })
    }

    pub fn lookup(&self, path: &str) -> Option<Target> {
        self.paths.get(path).copied()
    }

    /// The symbol an address stands for. A modport's port stands for the interface's own signal.
    pub fn symbol(&self, addr: Addr) -> Option<SymId> {
        self.by_addr.get(&addr).copied()
    }

    /// The paths of the signals (variables and nets) a trace can start from, in design order.
    pub fn signals(&self) -> impl Iterator<Item = &str> {
        self.symbols
            .iter()
            .filter(|symbol| !symbol.local && matches!(symbol.kind, ValueKind::Variable | ValueKind::Net))
            .map(|symbol| symbol.path.as_str())
    }

    /// Parameters, specparams and enum values name a fixed value, not a signal.
    pub fn is_constant(&self, id: SymId) -> bool {
        matches!(self.symbols[id].kind, ValueKind::Parameter | ValueKind::Specparam | ValueKind::EnumValue)
    }

    /// The containers that write `id`, and those that mention it.
    pub fn driven_by(&self, id: SymId) -> &[usize] {
        self.driven_by.get(&id).map_or(&[], Vec::as_slice)
    }

    pub fn used_by(&self, id: SymId) -> &[usize] {
        self.used_by.get(&id).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn type_of(&self, addr: Addr) -> Option<&Width> {
        self.types.get(&addr)
    }

    pub(crate) fn field_offset(&self, field: Addr) -> Option<u32> {
        self.fields.get(&field).copied()
    }

    /// Notes on each use of a signal which bits its selects pick.
    fn annotate_containers(&mut self) {
        let mut containers = std::mem::take(&mut self.containers);
        for container in &mut containers {
            match container {
                Container::Assign(node) | Container::Procedural { body: node, .. } => self.annotate(node),
                Container::Instance(connections) => {
                    for node in connections.iter_mut().filter_map(|connection| connection.expr.as_mut()) {
                        self.annotate(node);
                    }
                }
            }
        }
        self.containers = containers;
    }

    pub fn width(&self, width: &Width) -> u32 {
        self.width_at(width, 0)
    }

    fn width_at(&self, width: &Width, depth: u32) -> u32 {
        // (A typedef that names itself would otherwise never end.)
        if depth > 32 {
            return 0;
        }
        match width {
            Width::Bits(bits) => *bits,
            Width::Link(addr) => self.types.get(addr).map_or(0, |target| self.width_at(target, depth + 1)),
            Width::Sum(parts) => parts.iter().map(|part| self.width_at(&part.ty, depth + 1)).sum(),
            Width::Max(parts) => parts.iter().map(|part| self.width_at(&part.ty, depth + 1)).max().unwrap_or(0),
            Width::Times(left, right, element) => {
                (left.abs_diff(*right) as u32 + 1) * self.width_at(element, depth + 1)
            }
            // (Not packed: no bits to pair or select.)
            Width::Unpacked(..) | Width::Record(_) => 0,
        }
    }
}

/// The scope a symbol is declared in, as far as its path is concerned.
struct Scope {
    path: String,
    /// The root and compilation units add nothing to the path.
    root_like: bool,
    /// Members of a package are reached with `::`.
    package: bool,
    /// The indices of the instance arrays around this scope.
    array: Vec<i64>,
    /// The first index of this scope's elements when it is an instance array.
    lower: Option<i64>,
    /// Whether this is (or is inside) a statement block.
    local: bool,
}

#[derive(Default)]
struct Builder {
    symbols: Vec<SymbolInfo>,
    by_addr: HashMap<Addr, SymId>,
    paths: HashMap<String, Target>,
    containers: Vec<Container>,
    /// Modport ports by address: they are looked up through their interface's own signal.
    modport_ports: Vec<(Addr, Option<Addr>, String)>,
    /// The ports of each modport, by the modport's address.
    modports: HashMap<Addr, Vec<(String, Addr)>>,
    /// The signals declared directly in each instance, by the instance's address.
    instance_signals: HashMap<Addr, Vec<(String, Addr)>>,
    /// `instance.port` names for interface ports: (path, interface instance, modport).
    interface_ports: Vec<(String, Option<Addr>, Option<Addr>)>,
}

impl Builder {
    fn members(&mut self, members: Vec<Symbol>, parent: &Scope) {
        for (position, symbol) in members.into_iter().enumerate() {
            self.symbol(symbol, position, parent);
        }
    }

    fn symbol(&mut self, symbol: Symbol, position: usize, parent: &Scope) {
        let Symbol { addr, name, loc, item } = symbol;
        match item {
            Item::Value { kind, ty, init, .. } => {
                let path = member_path(parent, &name, "");
                let id = self.symbols.len();
                self.by_addr.insert(addr, id);
                // A name declared twice (an error slang reports, and still dumps) is the first one, as
                // slang's lookup and the hierarchy have it; a scope's path gives way to a signal's.
                match self.paths.get(&path) {
                    Some(Target::Value(_)) => {}
                    _ => drop(self.paths.insert(path.clone(), Target::Value(id))),
                }
                self.symbols.push(SymbolInfo { addr, name, path, kind, ty, loc, local: parent.local });
                // A net's initializer is a continuous assignment; a variable's is its value at the start,
                // which is not logic that the net's wire rule could apply to.
                match (init, kind) {
                    (Some(init), ValueKind::Net) => self.containers.push(Container::Assign(init)),
                    (Some(init), _) => self.containers.push(Container::Procedural { combinational: false, body: init }),
                    (None, _) => {}
                }
            }
            Item::ModportPort { internal } => {
                // The path under the modport; the signal is known once everything is read.
                self.modport_ports.push((addr, internal, member_path(parent, &name, "")));
            }
            Item::Scope { kind, members } => {
                let array = match kind {
                    ScopeKind::InstanceArray { .. } => {
                        let mut array = parent.array.clone();
                        array.extend(parent.lower.map(|lower| lower + position as i64));
                        array
                    }
                    _ => Vec::new(),
                };
                let suffix = match kind {
                    ScopeKind::GenerateBlock { index: Some(index), .. } => format!("[{index}]"),
                    _ => String::new(),
                };
                let path = member_path(parent, &name, &suffix);
                self.paths.entry(path.clone()).or_insert(Target::Other);
                if let ScopeKind::Other = kind {
                    // A modport lists the signals of its interface that it exposes.
                    let ports = members.iter().filter_map(|member| match member.item {
                        Item::ModportPort { internal: Some(internal) } => Some((member.name.clone(), internal)),
                        _ => None,
                    });
                    let ports: Vec<_> = ports.collect();
                    if !ports.is_empty() {
                        self.modports.insert(addr, ports);
                    }
                }
                let lower = match kind {
                    ScopeKind::InstanceArray { lower } => Some(lower),
                    _ => None,
                };
                let scope = Scope {
                    path,
                    root_like: matches!(kind, ScopeKind::Unit) && parent.root_like,
                    package: matches!(kind, ScopeKind::Package),
                    array,
                    lower,
                    local: parent.local || matches!(kind, ScopeKind::Other),
                };
                self.members(members, &scope);
            }
            Item::Instance { members, connections } => {
                // An element of an instance array is numbered by the arrays around it.
                let suffix: String = match parent.lower {
                    Some(lower) => parent
                        .array
                        .iter()
                        .copied()
                        .chain([lower + position as i64])
                        .map(|index| format!("[{index}]"))
                        .collect(),
                    None => String::new(),
                };
                let path = member_path(parent, &name, &suffix);
                self.paths.entry(path.clone()).or_insert(Target::Other);
                let signals = members
                    .iter()
                    .filter(|member| matches!(member.item, Item::Value { .. }))
                    .map(|member| (member.name.clone(), member.addr))
                    .collect();
                self.instance_signals.insert(addr, signals);
                for connection in &connections {
                    if let Port::Interface { name, instance, modport } = &connection.port {
                        self.interface_ports.push((format!("{path}.{name}"), *instance, *modport));
                    }
                }
                self.containers.push(Container::Instance(connections));
                let scope = Scope { path, root_like: false, package: false, array: Vec::new(), lower: None, local: false };
                self.members(members, &scope);
            }
            Item::Assign(node) => self.containers.push(Container::Assign(node)),
            Item::Procedural { combinational, body } => {
                self.containers.push(Container::Procedural { combinational, body });
            }
            Item::Other => {}
        }
    }

    fn finish(mut self, root: &Path, files: Vec<String>, types: HashMap<Addr, Width>, fields: HashMap<Addr, u32>, texts: &Texts) -> Graph {
        // Modport ports stand for the interface's signal, by address and by path.
        for (addr, internal, path) in std::mem::take(&mut self.modport_ports) {
            if let Some(&id) = internal.and_then(|internal| self.by_addr.get(&internal)) {
                self.by_addr.insert(addr, id);
                self.paths.insert(path, Target::Value(id));
            }
        }
        // Inside a module, an interface port is a name for the connected interface instance (or
        // modport): `inst.port.signal` finds that interface's signal.
        for (prefix, instance, modport) in std::mem::take(&mut self.interface_ports) {
            let exposed: Vec<(String, Addr)> = match (modport, instance) {
                (Some(modport), _) if self.modports.contains_key(&modport) => self.modports[&modport].clone(),
                (_, Some(instance)) => self.instance_signals.get(&instance).cloned().unwrap_or_default(),
                _ => Vec::new(),
            };
            self.paths.entry(prefix.clone()).or_insert(Target::Other);
            for (name, addr) in exposed {
                if let Some(&id) = self.by_addr.get(&addr) {
                    self.paths.insert(format!("{prefix}.{name}"), Target::Value(id));
                }
            }
        }

        // A path with an escaped name is found by its plain spelling too (the hierarchy view's).
        let plain: Vec<(String, Target)> =
            self.paths.iter().filter(|(path, _)| path.contains('\\')).map(|(path, target)| (plain_path(path).into_owned(), *target)).collect();
        for (path, target) in plain {
            self.paths.entry(path).or_insert(target);
        }

        let sources = Sources::load_shared(root, &files, texts);
        let mut graph = Graph {
            symbols: self.symbols,
            containers: self.containers,
            sources,
            by_addr: self.by_addr,
            paths: self.paths,
            types,
            fields,
            driven_by: HashMap::new(),
            used_by: HashMap::new(),
        };
        graph.refine_macro_arguments();
        graph.annotate_containers();
        graph.index();
        graph
    }
}

/// The path of a member of `parent`, the way slang prints it: names joined with `.`, or `::` below
/// a package, and a name that isn't a plain identifier escaped. An unnamed member (a loop
/// iteration, an element of an instance array) adds only its suffix, the index.
fn member_path(parent: &Scope, name: &str, suffix: &str) -> String {
    let mut path = parent.path.clone();
    if !name.is_empty() {
        if !parent.root_like {
            path.push_str(if parent.package { "::" } else { "." });
        }
        path.push_str(&path_name(name));
    }
    path.push_str(suffix);
    path
}

/// A name as slang writes it in a path: escaped (`\\a-b `, with the space that ends it) unless it is a
/// plain identifier.
pub(crate) fn path_name(name: &str) -> String {
    if needs_escaping(name) { format!("\\{name} ") } else { name.to_string() }
}

/// A path with its escaped names written plainly (`top.\\a-b .x` as `top.a-b.x`), the way the hierarchy
/// view joins names into paths. Both spellings are understood wherever a path is asked about.
pub(crate) fn plain_path(path: &str) -> std::borrow::Cow<'_, str> {
    if !path.contains('\\') {
        return path.into();
    }
    let mut plain = String::with_capacity(path.len());
    let mut escaped = false;
    for c in path.chars() {
        match (c, escaped) {
            ('\\', false) => escaped = true,
            (' ', true) => escaped = false,
            _ => plain.push(c),
        }
    }
    plain.into()
}

fn needs_escaping(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    !first_ok || chars.any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
}

impl Graph {
    /// What comes out of a macro is placed by the dump at the macro's name, with no extent. Slang-server
    /// puts a signal handed to a macro at the argument, and the rest over the whole invocation;
    /// look both up in the source.
    fn refine_macro_arguments(&mut self) {
        let mut containers = std::mem::take(&mut self.containers);
        for container in &mut containers {
            match container {
                Container::Assign(node) | Container::Procedural { body: node, .. } => self.refine(node),
                Container::Instance(connections) => {
                    for node in connections.iter_mut().filter_map(|connection| connection.expr.as_mut()) {
                        self.refine(node);
                    }
                }
            }
        }
        self.containers = containers;
    }

    fn refine(&self, node: &mut Node) {
        if let Some(range) = node.range.filter(|range| range.start == range.end) {
            let name = match node.kind {
                NodeKind::Value { addr, .. } => self.symbol(addr).map(|id| self.symbols[id].name.as_str()),
                _ => None,
            };
            let argument = name.and_then(|name| self.sources.macro_argument(range.file, range.start, name));
            node.range = argument.or_else(|| self.sources.macro_invocation(range.file, range.start)).or(node.range);
        }
        for child in &mut node.children {
            self.refine(child);
        }
    }

    fn index(&mut self) {
        let mut driven_by: HashMap<SymId, Vec<usize>> = HashMap::new();
        let mut used_by: HashMap<SymId, Vec<usize>> = HashMap::new();
        for (index, container) in self.containers.iter().enumerate() {
            let mut driven = Vec::new();
            let mut used = Vec::new();
            match container {
                Container::Assign(node) | Container::Procedural { body: node, .. } => {
                    self.scan(node, true, &mut driven, &mut used);
                }
                Container::Instance(connections) => {
                    for connection in connections {
                        if let Port::Port { direction, internal: Some(internal) } = &connection.port {
                            if let Some(id) = self.symbol(*internal) {
                                // Every port is used by its instance; an input is driven by the connection,
                                // and so is an `inout` or `ref` port, which is written from both sides.
                                used.push(id);
                                if *direction != Direction::Out {
                                    driven.push(id);
                                }
                            }
                        }
                        if let Some(node) = &connection.expr {
                            self.scan(node, true, &mut driven, &mut used);
                        }
                    }
                }
            }
            for (symbols, index_map) in [(driven, &mut driven_by), (used, &mut used_by)] {
                for id in symbols {
                    let list = index_map.entry(id).or_default();
                    if list.last() != Some(&index) {
                        list.push(index);
                    }
                }
            }
        }
        self.driven_by = driven_by;
        self.used_by = used_by;
    }

    /// Collects the symbols assigned and the symbols mentioned in a tree. A branch that a constant
    /// condition never takes (`if (FPU == 1)` with `FPU = 0`) assigns nothing: slang's driver
    /// analysis doesn't count it, so neither does slang-server.
    fn scan(&self, node: &Node, live: bool, driven: &mut Vec<SymId>, used: &mut Vec<SymId>) {
        match &node.kind {
            NodeKind::Value { addr, .. } => used.extend(self.symbol(*addr)),
            NodeKind::Assign { .. } if live => {
                if let Some(left) = node.children.first() {
                    self.assigned(left, driven);
                }
            }
            _ => {}
        }
        let (conditions, taken) = match node.kind {
            NodeKind::If { conditions, taken } => (conditions, taken),
            _ => (node.children.len(), None),
        };
        for (position, child) in node.children.iter().enumerate() {
            let branch = match position.checked_sub(conditions) {
                Some(0) => taken != Some(false),
                Some(_) => taken != Some(true),
                None => true,
            };
            self.scan(child, live && branch, driven, used);
        }
    }

    /// The signals an assignment writes: an index on the left only picks which bits.
    fn assigned(&self, left: &Node, driven: &mut Vec<SymId>) {
        match &left.kind {
            NodeKind::Value { addr, .. } => driven.extend(self.symbol(*addr)),
            NodeKind::Select(_) => {
                if let Some(value) = left.children.first() {
                    self.assigned(value, driven);
                }
            }
            _ => {
                for child in &left.children {
                    self.assigned(child, driven);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_declared_twice_is_the_first_one() {
        // (An error in the design, which slang reports and still dumps: both readers of the dump take the first.)
        let value = |addr| Symbol {
            addr,
            name: "x".into(),
            loc: None,
            item: Item::Value { kind: ValueKind::Variable, value: None, ty: Width::Bits(1), init: None },
        };
        let top = Symbol { addr: 1, name: "top".into(), loc: None, item: Item::Instance { members: vec![value(2), value(3)], connections: Vec::new() } };
        let design = Design { files: Vec::new(), root: vec![top], types: HashMap::new(), fields: HashMap::new() };
        let graph = Graph::build(Path::new("/nowhere"), design);
        let Some(Target::Value(id)) = graph.lookup("top.x") else { panic!("top.x is a signal") };
        assert_eq!(graph.symbols[id].addr, 2);
        assert_eq!(graph.symbol(3).map(|id| graph.symbols[id].addr), Some(3));
    }

    #[test]
    fn escaping_follows_slang() {
        assert!(!needs_escaping("esc$sig"));
        assert!(!needs_escaping("_x1"));
        assert!(needs_escaping("1x"));
        assert!(needs_escaping("$x"));
        assert!(needs_escaping("a-b"));
        let scope = Scope { path: "top".into(), root_like: false, package: false, array: vec![], lower: None, local: false };
        assert_eq!(member_path(&scope, "a-b", ""), "top.\\a-b ");
        assert_eq!(member_path(&scope, "", "[3]"), "top[3]");
        assert_eq!(plain_path("top.\\a-b .\\c.d [2].x"), "top.a-b.c.d[2].x");
        assert_eq!(plain_path("top.u.x"), "top.u.x");
    }
}

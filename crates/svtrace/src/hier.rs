//! The design hierarchy, answered in slang-server's shapes (`HierarchicalView.h` and
//! `ServerCompilation::getScope`/`getScopes` in the fork): `slang.getScope` gives the children of a
//! scope and `slang.getScopes` the steps the hierarchy view needs to reveal a path.
//!
//! The dump has what the view shows (instances, generate blocks, parameters, ports, signals, interface
//! ports and their modports, packages, the types to spell) but only the position of each symbol's
//! name, where the server has the range of its whole syntax. So a location here starts at the name (at
//! the `module` or `package` keyword for a declaration) and ends where the name ends. The view uses
//! only the file and the start. The dump itself is dropped once this is read.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::ast::{Addr, Loc, Pos};
use crate::source::{Sources, Texts};

pub const GET_SCOPE: &str = "slang.getScope";
pub const GET_SCOPES: &str = "slang.getScopes";
pub const COMMANDS: [&str; 2] = [GET_SCOPE, GET_SCOPES];

type NodeId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    In,
    Out,
    InOut,
    Ref,
}

impl Dir {
    fn word(self) -> &'static str {
        match self {
            Dir::In => "input",
            Dir::Out => "output",
            Dir::InOut => "inout",
            Dir::Ref => "ref",
        }
    }

    fn parse(text: &str) -> Dir {
        match text {
            "In" => Dir::In,
            "Out" => Dir::Out,
            "InOut" => Dir::InOut,
            _ => Dir::Ref,
        }
    }
}

/// A type as the dump describes it, with what `getTypeString` needs to spell it.
#[derive(Clone, Debug)]
enum Ty {
    Scalar { name: String, signed: bool },
    /// `int`, `integer`...: the dump does not say whether it was written `unsigned`; the source does.
    Predef { name: String },
    /// Spelled by its name: `real`, `string`, `event`...
    Plain(String),
    Packed { elem: Box<Ty>, left: i64, right: i64 },
    Unpacked { elem: Box<Ty>, dim: Dim },
    Enum { base: Box<Ty>, values: Vec<(String, String)> },
    Struct { packed: bool, union: bool, tagged: bool, fields: Vec<Field> },
    Alias { name: String, target: Box<Ty> },
    /// A type the dump wrote once and refers to by address afterwards, with the name it gave.
    Link(Addr, String),
    Error,
}

#[derive(Clone, Debug)]
struct Field {
    name: String,
    ty: Ty,
    loc: Option<Loc>,
}

#[derive(Clone, Debug)]
enum Dim {
    Fixed(i64, i64),
    Dynamic,
    Assoc(Option<Box<Ty>>),
    Queue(u64),
}

enum Kind {
    Instance { def: Option<Addr> },
    /// `left` and `right` as declared, `text` as the dump writes the range (`[3:0]`).
    InstanceArray { left: i64, right: i64, text: String },
    GenBlock { uninstantiated: bool },
    GenArray,
    Package,
    Param { ty: Ty, value: Option<String> },
    TypeParam { value: Option<Ty> },
    Value { ty: Ty, addr: Addr },
    InterfacePort { interface: String, modport: String, conn: Option<(Option<Addr>, Option<Addr>)> },
    Modport,
    ModportPort { ty: Ty, dir: Dir },
}

struct Node {
    /// The symbol's name in the dump (empty for an array's element).
    name: String,
    /// The name the view shows: the name, or `[index]`.
    label: String,
    loc: Option<Loc>,
    path: String,
    kind: Kind,
    children: Vec<NodeId>,
}

struct Definition {
    name: String,
    loc: Option<Loc>,
    kind: &'static str,
}

/// The hierarchy of an elaborated design.
pub struct Hier {
    pub(crate) sources: Sources,
    nodes: Vec<Node>,
    /// The scope each node is a member of, by node.
    parents: Vec<Option<NodeId>>,
    tops: Vec<NodeId>,
    /// Sorted by name, as `Compilation::getPackages` has them.
    packages: Vec<NodeId>,
    by_path: HashMap<String, NodeId>,
    by_addr: HashMap<Addr, NodeId>,
    types: HashMap<Addr, Ty>,
    definitions: HashMap<Addr, Definition>,
    /// The direction of the port each internal symbol stands for.
    ports: HashMap<Addr, Dir>,
}

// ---- Reading the dump ----

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `"<addr> <name>"`, as the dump refers to a symbol.
fn split_link(link: &str) -> Option<(Addr, &str)> {
    let (addr, name) = link.split_once(' ')?;
    Some((addr.parse().ok()?, name))
}

fn link(v: &Value) -> Option<Addr> {
    split_link(v.as_str()?).map(|(addr, _)| addr)
}

/// `[3:0]`.
fn parse_range(text: &str) -> Option<(i64, i64)> {
    let (left, right) = text.strip_prefix('[')?.strip_suffix(']')?.split_once(':')?;
    Some((left.trim().parse().ok()?, right.trim().parse().ok()?))
}

struct Reader {
    hier: Hier,
    files: Vec<String>,
    file_ids: HashMap<String, u32>,
    /// Interface ports by address, to give them the connections the instance holding them has.
    interface_ports: HashMap<Addr, NodeId>,
    connections: Vec<(Addr, Option<Addr>, Option<Addr>)>,
}

impl Hier {
    /// Reads the hierarchy from slang's `{"design": ..., "definitions": [...]}`. `root` is the folder
    /// slang ran in, which the dump's file names are relative to.
    pub fn read(root: &Path, json: &Value) -> Hier {
        Self::read_shared(root, json, &Texts::default())
    }

    /// [`Hier::read`], sharing the source files' text with the build's other reader.
    pub fn read_shared(root: &Path, json: &Value, texts: &Texts) -> Hier {
        crate::deep(|| Self::read_deep(root, json, texts))
    }

    fn read_deep(root: &Path, json: &Value, texts: &Texts) -> Hier {
        let mut reader = Reader {
            hier: Hier {
                sources: Sources::load(root, &[]),
                nodes: Vec::new(),
                parents: Vec::new(),
                tops: Vec::new(),
                packages: Vec::new(),
                by_path: HashMap::new(),
                by_addr: HashMap::new(),
                types: HashMap::new(),
                definitions: HashMap::new(),
                ports: HashMap::new(),
            },
            files: Vec::new(),
            file_ids: HashMap::new(),
            interface_ports: HashMap::new(),
            connections: Vec::new(),
        };
        reader.run(json);
        let Reader { mut hier, files, connections, interface_ports, .. } = reader;
        for (port, instance, modport) in connections {
            if let Some(&id) = interface_ports.get(&port) {
                if let Kind::InterfacePort { conn, .. } = &mut hier.nodes[id].kind {
                    *conn = Some((instance, modport));
                }
            }
        }
        let nodes = &hier.nodes;
        hier.packages.sort_by(|a, b| nodes[*a].name.cmp(&nodes[*b].name));
        hier.sources = Sources::load_shared(root, &files, texts);
        hier
    }
}

impl Reader {
    fn run(&mut self, json: &Value) {
        if let Some(Value::Array(definitions)) = json.get("definitions") {
            for definition in definitions {
                let kind = match text(definition, "definitionKind") {
                    "Interface" => "Interface",
                    "Program" => "Program",
                    _ => "Module",
                };
                let loc = self.loc(definition);
                let name = text(definition, "name").to_string();
                if let Some(addr) = definition.get("addr").and_then(Value::as_u64) {
                    self.hier.definitions.insert(addr, Definition { name, loc, kind });
                }
            }
        }
        let Some(Value::Array(members)) = json.get("design").and_then(|design| design.get("members")) else {
            return;
        };
        for member in members {
            match text(member, "kind") {
                "CompilationUnit" => {
                    if let Some(Value::Array(inner)) = member.get("members") {
                        for symbol in inner.iter().filter(|symbol| text(symbol, "kind") == "Package") {
                            let id = self.package(symbol);
                            self.hier.packages.push(id);
                        }
                    }
                }
                "Instance" => {
                    let name = text(member, "name").to_string();
                    let id = self.instance(member, None, &name, &name, name.clone());
                    self.hier.tops.push(id);
                }
                _ => {}
            }
        }
    }

    fn loc(&mut self, v: &Value) -> Option<Loc> {
        let name = v.get("source_file")?.as_str()?;
        let file = match self.file_ids.get(name) {
            Some(&id) => id,
            None => {
                let id = self.files.len() as u32;
                self.files.push(name.to_string());
                self.file_ids.insert(name.to_string(), id);
                id
            }
        };
        let line = v.get("source_line")?.as_u64()? as u32;
        let col = v.get("source_column")?.as_u64()? as u32;
        Some(Loc { file, pos: Pos { line, col } })
    }

    fn add(&mut self, parent: Option<NodeId>, node: Node) -> NodeId {
        let id = self.hier.nodes.len();
        self.hier.by_path.entry(node.path.clone()).or_insert(id);
        self.hier.nodes.push(node);
        self.hier.parents.push(parent);
        if let Some(parent) = parent {
            self.hier.nodes[parent].children.push(id);
        }
        id
    }

    /// The path of a named member: `.` below most scopes, `::` below a package.
    fn member_path(&self, parent: NodeId, name: &str) -> String {
        let parent = &self.hier.nodes[parent];
        let separator = if matches!(parent.kind, Kind::Package) { "::" } else { "." };
        format!("{}{}{}", parent.path, separator, name)
    }

    fn package(&mut self, v: &Value) -> NodeId {
        let name = text(v, "name").to_string();
        let loc = self.loc(v);
        let id = self.add(None, Node { name: name.clone(), label: name.clone(), loc, path: name, kind: Kind::Package, children: Vec::new() });
        self.members(v, id);
        id
    }

    fn members(&mut self, scope: &Value, parent: NodeId) {
        let Some(Value::Array(members)) = scope.get("members") else {
            return;
        };
        for member in members {
            self.member(member, parent);
        }
    }

    fn member(&mut self, v: &Value, parent: NodeId) {
        let name = text(v, "name");
        match text(v, "kind") {
            "Instance" => {
                let path = self.member_path(parent, name);
                self.instance(v, Some(parent), name, name, path);
            }
            "InstanceArray" => {
                let path = self.member_path(parent, name);
                self.instance_array(v, parent, name, name, path);
            }
            "GenerateBlock" => {
                let path = self.member_path(parent, name);
                self.generate_block(v, parent, name, name, path);
            }
            "GenerateBlockArray" => {
                let path = self.member_path(parent, name);
                let loc = self.loc(v);
                let id = self.add(
                    Some(parent),
                    Node { name: name.to_string(), label: name.to_string(), loc, path: path.clone(), kind: Kind::GenArray, children: Vec::new() },
                );
                let variable = split_link(text(v, "loopVariable")).map_or("", |(_, name)| name);
                let Some(Value::Array(blocks)) = v.get("members") else {
                    return;
                };
                for block in blocks.iter().filter(|block| text(block, "kind") == "GenerateBlock") {
                    // The iteration is named by the loop variable's value, which is also in its path.
                    let index = loop_value(block, variable).or_else(|| block.get("constructIndex").and_then(Value::as_i64)).unwrap_or(0);
                    let label = format!("[{index}]");
                    self.generate_block(block, id, "", &label, format!("{path}{label}"));
                }
            }
            "Parameter" => {
                let ty = self.ty(v.get("type").unwrap_or(&Value::Null));
                let value = v.get("value").and_then(Value::as_str).map(str::to_string);
                self.leaf(v, parent, Kind::Param { ty, value });
            }
            "TypeParameter" => {
                let value = v.get("type").map(|ty| self.ty(ty));
                self.leaf(v, parent, Kind::TypeParam { value });
            }
            "Variable" | "Net" | "Specparam" => {
                let ty = self.ty(v.get("type").unwrap_or(&Value::Null));
                let addr = v.get("addr").and_then(Value::as_u64).unwrap_or(0);
                self.leaf(v, parent, Kind::Value { ty, addr });
            }
            "Port" => {
                if let Some(internal) = v.get("internalSymbol").and_then(link) {
                    self.hier.ports.insert(internal, Dir::parse(text(v, "direction")));
                }
            }
            "InterfacePort" => {
                let kind = Kind::InterfacePort {
                    interface: text(v, "interfaceDef").to_string(),
                    modport: text(v, "modport").to_string(),
                    conn: None,
                };
                let id = self.leaf(v, parent, kind);
                if let Some(addr) = v.get("addr").and_then(Value::as_u64) {
                    self.interface_ports.insert(addr, id);
                    self.hier.by_addr.insert(addr, id);
                }
            }
            "Modport" => {
                let id = self.leaf(v, parent, Kind::Modport);
                if let Some(addr) = v.get("addr").and_then(Value::as_u64) {
                    self.hier.by_addr.insert(addr, id);
                }
                if let Some(Value::Array(ports)) = v.get("members") {
                    for port in ports.iter().filter(|port| text(port, "kind") == "ModportPort") {
                        let ty = self.ty(port.get("type").unwrap_or(&Value::Null));
                        let dir = Dir::parse(text(port, "direction"));
                        self.leaf(port, id, Kind::ModportPort { ty, dir });
                    }
                }
            }
            // (Types are read for the links to them.)
            "TypeAlias" | "EnumType" => {
                self.ty(v);
            }
            _ => {}
        }
    }

    /// A named member with no members of interest of its own.
    fn leaf(&mut self, v: &Value, parent: NodeId, kind: Kind) -> NodeId {
        let name = text(v, "name").to_string();
        let path = self.member_path(parent, &name);
        let loc = self.loc(v);
        self.add(Some(parent), Node { label: name.clone(), name, loc, path, kind, children: Vec::new() })
    }

    fn instance(&mut self, v: &Value, parent: Option<NodeId>, name: &str, label: &str, path: String) -> NodeId {
        let loc = self.loc(v);
        let body = v.get("body");
        let def = body.and_then(|body| body.get("definition")).and_then(link);
        let id = self.add(
            parent,
            Node { name: name.to_string(), label: label.to_string(), loc, path, kind: Kind::Instance { def }, children: Vec::new() },
        );
        if let Some(addr) = v.get("addr").and_then(Value::as_u64) {
            self.hier.by_addr.insert(addr, id);
        }
        if let Some(body) = body {
            self.members(body, id);
        }
        if let Some(Value::Array(connections)) = v.get("connections") {
            for connection in connections {
                let Some(port) = connection.get("port").filter(|port| text(port, "kind") == "InterfacePort") else {
                    continue;
                };
                if let Some(addr) = port.get("addr").and_then(Value::as_u64) {
                    self.connections.push((addr, connection.get("ifaceInstance").and_then(link), connection.get("modport").and_then(link)));
                }
            }
        }
        id
    }

    fn instance_array(&mut self, v: &Value, parent: NodeId, name: &str, label: &str, path: String) -> NodeId {
        let loc = self.loc(v);
        let range = text(v, "range");
        let (left, right) = parse_range(range).unwrap_or((0, 0));
        let id = self.add(
            Some(parent),
            Node {
                name: name.to_string(),
                label: label.to_string(),
                loc,
                path: path.clone(),
                kind: Kind::InstanceArray { left, right, text: range.to_string() },
                children: Vec::new(),
            },
        );
        let Some(Value::Array(elements)) = v.get("members") else {
            return id;
        };
        // The elements run from the range's lower bound up, whichever way it is written.
        let lower = left.min(right);
        for (position, element) in elements.iter().enumerate() {
            let label = format!("[{}]", lower + position as i64);
            let element_path = format!("{path}{label}");
            match text(element, "kind") {
                "Instance" => {
                    self.instance(element, Some(id), name, &label, element_path);
                }
                "InstanceArray" => {
                    self.instance_array(element, id, name, &label, element_path);
                }
                _ => {}
            }
        }
        id
    }

    fn generate_block(&mut self, v: &Value, parent: NodeId, name: &str, label: &str, path: String) {
        let loc = self.loc(v);
        let uninstantiated = v.get("isUninstantiated").and_then(Value::as_bool).unwrap_or(false);
        let label = if label.is_empty() { name } else { label };
        let id = self.add(
            Some(parent),
            Node { name: name.to_string(), label: label.to_string(), loc, path, kind: Kind::GenBlock { uninstantiated }, children: Vec::new() },
        );
        self.members(v, id);
    }

    fn ty(&mut self, v: &Value) -> Ty {
        let object = match v {
            Value::String(s) => {
                return match split_link(s) {
                    Some((addr, name)) => Ty::Link(addr, name.to_string()),
                    None => Ty::Plain(s.clone()),
                };
            }
            Value::Object(_) => v,
            _ => return Ty::Error,
        };
        let name = text(object, "name").to_string();
        let ty = match text(object, "kind") {
            "ScalarType" => Ty::Scalar { name, signed: object.get("isSigned").and_then(Value::as_bool).unwrap_or(false) },
            "PredefinedIntegerType" => Ty::Predef { name },
            "PackedArrayType" => {
                let (left, right) = parse_range(text(object, "range")).unwrap_or((0, 0));
                let elem = Box::new(self.ty(object.get("elementType").unwrap_or(&Value::Null)));
                Ty::Packed { elem, left, right }
            }
            "FixedSizeUnpackedArrayType" => {
                let (left, right) = parse_range(text(object, "range")).unwrap_or((0, 0));
                let elem = Box::new(self.ty(object.get("elementType").unwrap_or(&Value::Null)));
                Ty::Unpacked { elem, dim: Dim::Fixed(left, right) }
            }
            "DynamicArrayType" | "DPIOpenArrayType" => {
                let elem = Box::new(self.ty(object.get("elementType").unwrap_or(&Value::Null)));
                Ty::Unpacked { elem, dim: Dim::Dynamic }
            }
            "AssociativeArrayType" => {
                let elem = Box::new(self.ty(object.get("elementType").unwrap_or(&Value::Null)));
                let index = object.get("indexType").map(|index| Box::new(self.ty(index)));
                Ty::Unpacked { elem, dim: Dim::Assoc(index) }
            }
            "QueueType" => {
                let elem = Box::new(self.ty(object.get("elementType").unwrap_or(&Value::Null)));
                let bound = object.get("maxBound").and_then(Value::as_u64).unwrap_or(0);
                Ty::Unpacked { elem, dim: Dim::Queue(bound) }
            }
            "EnumType" => {
                let base = Box::new(self.ty(object.get("baseType").unwrap_or(&Value::Null)));
                let mut values = Vec::new();
                if let Some(Value::Array(members)) = object.get("members") {
                    for member in members.iter().filter(|member| text(member, "kind") == "EnumValue") {
                        values.push((text(member, "name").to_string(), text(member, "value").to_string()));
                    }
                }
                Ty::Enum { base, values }
            }
            kind @ ("PackedStructType" | "UnpackedStructType" | "PackedUnionType" | "UnpackedUnionType") => {
                let mut fields = Vec::new();
                if let Some(Value::Array(members)) = object.get("members") {
                    for member in members.iter().filter(|member| text(member, "kind") == "Field") {
                        let ty = self.ty(member.get("type").unwrap_or(&Value::Null));
                        fields.push(Field { name: text(member, "name").to_string(), ty, loc: self.loc(member) });
                    }
                }
                Ty::Struct {
                    packed: kind.starts_with("Packed"),
                    union: kind.ends_with("UnionType"),
                    tagged: object.get("isTagged").and_then(Value::as_bool).unwrap_or(false),
                    fields,
                }
            }
            "TypeAlias" => Ty::Alias { name, target: Box::new(self.ty(object.get("target").unwrap_or(&Value::Null))) },
            "ErrorType" => Ty::Error,
            _ => Ty::Plain(name),
        };
        if matches!(ty, Ty::Enum { .. } | Ty::Struct { .. } | Ty::Alias { .. }) {
            if let Some(addr) = object.get("addr").and_then(Value::as_u64) {
                self.hier.types.insert(addr, ty.clone());
            }
        }
        ty
    }
}

/// The value of the loop variable in one iteration of a generate loop: the parameter of that name the
/// iteration's block declares.
fn loop_value(block: &Value, variable: &str) -> Option<i64> {
    let Some(Value::Array(members)) = block.get("members") else {
        return None;
    };
    members
        .iter()
        .find(|member| text(member, "kind") == "Parameter" && (variable.is_empty() || text(member, "name") == variable))
        .and_then(|member| text(member, "value").parse().ok())
}

// ---- Spelling types as `getTypeString` does ----

fn default_signed(name: &str) -> bool {
    name != "time"
}

fn last_segment(name: &str) -> &str {
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

/// The signedness `unsigned` or `signed` written just before a declared name, which the dump leaves out
/// of a predefined integer type.
fn written_sign(prefix: &str) -> Option<bool> {
    let word = prefix.trim_end().rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next()?;
    match word {
        "unsigned" => Some(false),
        "signed" => Some(true),
        _ => None,
    }
}

impl Hier {
    fn resolve<'a>(&'a self, ty: &'a Ty) -> (&'a Ty, Option<&'a str>) {
        match ty {
            Ty::Link(addr, name) => (self.types.get(addr).unwrap_or(ty), Some(name)),
            _ => (ty, None),
        }
    }

    /// The type under typedefs and links.
    fn canon<'a>(&'a self, ty: &'a Ty) -> &'a Ty {
        let mut current = ty;
        for _ in 0..32 {
            current = self.resolve(current).0;
            match current {
                Ty::Alias { target, .. } => current = target,
                _ => break,
            }
        }
        current
    }

    fn type_name<'a>(&'a self, ty: &'a Ty) -> &'a str {
        match self.resolve(ty) {
            (Ty::Alias { name, .. } | Ty::Scalar { name, .. } | Ty::Predef { name } | Ty::Plain(name), _) => name,
            (Ty::Link(_, name), _) => last_segment(name),
            _ => "",
        }
    }

    fn is_array(&self, ty: &Ty) -> bool {
        matches!(self.canon(ty), Ty::Packed { .. } | Ty::Unpacked { .. })
    }

    fn width(&self, ty: &Ty) -> i64 {
        match self.canon(ty) {
            Ty::Scalar { .. } => 1,
            Ty::Predef { name } => match name.as_str() {
                "byte" => 8,
                "shortint" => 16,
                "int" | "integer" => 32,
                "longint" | "time" => 64,
                _ => 0,
            },
            Ty::Packed { elem, left, right } => ((left - right).abs() + 1) * self.width(elem),
            Ty::Enum { base, .. } => self.width(base),
            Ty::Struct { packed: true, union: false, fields, .. } => fields.iter().map(|field| self.width(&field.ty)).sum(),
            Ty::Struct { packed: true, union: true, fields, .. } => fields.iter().map(|field| self.width(&field.ty)).max().unwrap_or(0),
            _ => 0,
        }
    }

    fn four_state(&self, ty: &Ty) -> bool {
        match self.canon(ty) {
            Ty::Scalar { name, .. } => name != "bit",
            Ty::Predef { name } => matches!(name.as_str(), "integer" | "time"),
            Ty::Packed { elem, .. } => self.four_state(elem),
            Ty::Enum { base, .. } => self.four_state(base),
            Ty::Struct { fields, .. } => fields.iter().any(|field| self.four_state(&field.ty)),
            _ => false,
        }
    }

    fn signed(&self, ty: &Ty) -> bool {
        match self.canon(ty) {
            Ty::Scalar { signed, .. } => *signed,
            Ty::Predef { name } => default_signed(name),
            Ty::Packed { elem, .. } => match &**elem {
                Ty::Scalar { signed, .. } => *signed,
                Ty::Packed { .. } => self.signed(elem),
                _ => false,
            },
            Ty::Enum { base, .. } => self.signed(base),
            _ => false,
        }
    }

    /// The sign written before the name at `loc`, if the declaration has one.
    fn sign_at(&self, loc: Option<Loc>) -> Option<bool> {
        let loc = loc?;
        let line = self.sources.line(loc.file, loc.pos.line)?;
        let end = (loc.pos.col as usize).saturating_sub(1).min(line.len());
        written_sign(line.get(..end)?)
    }

    /// `getTypeString(type)`: the type as slang-server's hierarchy writes it (without a port's direction).
    fn type_string(&self, ty: &Ty, loc: Option<Loc>) -> String {
        let canon = self.canon(ty);
        let sign = self.sign_at(loc);
        if matches!(canon, Ty::Error) {
            return "Incomplete type".to_string();
        }
        let body = self.append(ty, sign);
        let kind = match canon {
            Ty::Enum { .. } => "Enum",
            Ty::Struct { packed: true, union: false, .. } => "PackedStruct",
            Ty::Struct { packed: false, union: false, .. } => "UnpackedStruct",
            Ty::Struct { packed: true, union: true, .. } => "PackedUnion",
            Ty::Struct { packed: false, union: true, .. } => "UnpackedUnion",
            _ => return body,
        };
        format!("{kind} {body}")
    }

    /// `TypePrinter::append` with `printAKA`, `elideScopeNames`, `skipTypeDefs` and `printIntegralRange`.
    fn append(&self, ty: &Ty, sign: Option<bool>) -> String {
        let (resolved, link_name) = self.resolve(ty);
        let mut out = String::new();
        match resolved {
            Ty::Alias { name, .. } => {
                out.push_str(name);
                out.push_str(&self.aka(resolved));
            }
            other => out.push_str(&self.visit(other, link_name.unwrap_or(""), sign)),
        }
        let canon = self.canon(resolved);
        if matches!(canon, Ty::Enum { .. } | Ty::Struct { packed: true, .. }) {
            let word = if self.four_state(canon) { "logic" } else { "bit" };
            out.push_str(&format!(" ({word}[{}:0])", self.width(canon) - 1));
        }
        out
    }

    /// ` (aka X)` when a typedef goes through other named types to a different name.
    fn aka(&self, alias: &Ty) -> String {
        let mut target = alias;
        let mut moved = false;
        for _ in 0..32 {
            let Ty::Alias { target: next, .. } = self.resolve(target).0 else {
                break;
            };
            let next = self.resolve(next).0;
            if self.type_name(next).is_empty() && !self.is_array(next) {
                break;
            }
            target = next;
            moved = true;
        }
        if moved && self.type_name(target) != self.type_name(alias) {
            format!(" (aka {})", self.visit(self.resolve(target).0, "", None))
        } else {
            String::new()
        }
    }

    /// `TypePrinter::visit`: the type itself, a typedef by its name. `hint` is the name a link gave the
    /// type, which spells an anonymous enum's `e$N`.
    fn visit(&self, ty: &Ty, hint: &str, sign: Option<bool>) -> String {
        match ty {
            Ty::Scalar { name, signed } => format!("{name}{}", if *signed { " signed" } else { "" }),
            Ty::Predef { name } => match sign {
                Some(signed) if signed != default_signed(name) => format!("{name} {}", if signed { "signed" } else { "unsigned" }),
                _ => name.clone(),
            },
            Ty::Plain(name) => name.clone(),
            Ty::Alias { name, .. } => name.clone(),
            Ty::Link(_, name) => last_segment(name).to_string(),
            Ty::Error => "<error>".to_string(),
            Ty::Packed { .. } => {
                let mut dims = String::new();
                let mut current = ty;
                while let Ty::Packed { elem, left, right } = current {
                    dims.push_str(&format!("[{left}:{right}]"));
                    match self.canon(elem) {
                        inner @ Ty::Packed { .. } => current = inner,
                        _ => return format!("{}{dims}", self.visit_elem(elem, sign)),
                    }
                }
                dims
            }
            Ty::Enum { base, values } => {
                let (width, signed) = (self.width(base), self.signed(base));
                let items: Vec<String> =
                    values.iter().map(|(name, value)| format!("{name}={}", decimal_with_base(value, width, signed))).collect();
                let id = last_segment(hint);
                format!("enum{{{}}}{}", items.join(","), if id.starts_with("e$") { id } else { "e$0" })
            }
            Ty::Struct { packed, union, tagged, fields } => {
                let mut out = String::from(if *union { "union" } else { "struct" });
                if *packed {
                    out.push_str(" packed");
                }
                if *tagged {
                    out.push_str(" tagged");
                }
                out.push('{');
                for field in fields {
                    out.push_str(&self.append(&field.ty, self.sign_at(field.loc)));
                    out.push_str(&format!(" {};", field.name));
                }
                out.push('}');
                out
            }
            Ty::Unpacked { .. } => {
                let mut element = ty;
                while let Ty::Unpacked { elem, .. } = self.canon(element) {
                    element = elem;
                    if !matches!(self.canon(element), Ty::Unpacked { .. }) {
                        break;
                    }
                }
                let mut out = self.visit_elem(element, sign);
                out.push('$');
                let mut current = self.canon(ty);
                while let Ty::Unpacked { elem, dim } = current {
                    match dim {
                        Dim::Fixed(left, right) => out.push_str(&format!("[{left}:{right}]")),
                        Dim::Dynamic => out.push_str("[]"),
                        Dim::Assoc(None) => out.push_str("[*]"),
                        Dim::Assoc(Some(index)) => out.push_str(&format!("[{}]", self.visit_elem(index, None))),
                        Dim::Queue(0) => out.push_str("[$]"),
                        Dim::Queue(bound) => out.push_str(&format!("[$:{bound}]")),
                    }
                    current = self.canon(elem);
                }
                out
            }
        }
    }

    /// `elementType.visit(...)`: an element that is a typedef or a link is spelled by its name.
    fn visit_elem(&self, ty: &Ty, sign: Option<bool>) -> String {
        let (resolved, link_name) = self.resolve(ty);
        self.visit(resolved, link_name.unwrap_or(""), sign)
    }
}

/// An enum member's value as `SVInt::toString(Decimal, includeBase)` writes it, from the dump's
/// `ConstantValue` text (`3'b100`, `32'd5`, `7`).
fn decimal_with_base(value: &str, width: i64, signed: bool) -> String {
    let (negative, digits) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let (width, signed, number) = match digits.split_once('\'') {
        Some((size, rest)) => {
            let rest_signed = rest.starts_with(['s', 'S']);
            let rest = rest.trim_start_matches(['s', 'S']);
            let radix = match rest.chars().next() {
                Some('b' | 'B') => 2,
                Some('o' | 'O') => 8,
                Some('d' | 'D') => 10,
                Some('h' | 'H') => 16,
                _ => return value.to_string(),
            };
            (size.parse().unwrap_or(width), rest_signed, u128::from_str_radix(&rest[1..], radix))
        }
        None => (width, signed, digits.parse::<u128>()),
    };
    match number {
        Ok(number) => format!("{}{width}'{}d{number}", if negative { "-" } else { "" }, if signed { "s" } else { "" }),
        Err(_) => value.to_string(),
    }
}

/// `formatConstantValue`: a known one-bit integer is `0` or `1`, anything else as the dump has it.
fn constant_text(value: &str) -> String {
    let unsigned = value.strip_prefix("1'");
    if let Some(rest) = unsigned.map(|rest| rest.trim_start_matches(['s', 'S'])) {
        let mut chars = rest.chars();
        if let (Some('b' | 'd' | 'h' | 'o'), Some(digit @ ('0' | '1')), None) = (chars.next(), chars.next(), chars.next()) {
            return digit.to_string();
        }
    }
    value.to_string()
}

// ---- The answers ----

impl Hier {
    fn lsp_location(&self, loc: Option<Loc>, name: &str, keywords: &[&str]) -> Value {
        let Some(loc) = loc else {
            let zero = json!({ "line": 0, "character": 0 });
            return json!({ "uri": "", "range": { "start": zero, "end": zero } });
        };
        let (line, character) = self.sources.position(loc.file, loc.pos);
        let end = character + name.encode_utf16().count() as u32;
        let (start_line, start_character) = self.keyword_start(loc, keywords).unwrap_or((line, character));
        json!({
            "uri": self.sources.uri(loc.file),
            "range": {
                "start": { "line": start_line, "character": start_character },
                "end": { "line": line, "character": end },
            },
        })
    }

    /// Where the `module` (`interface`, `package`...) keyword before the name at `loc` starts, if it is
    /// on the same line.
    fn keyword_start(&self, loc: Loc, keywords: &[&str]) -> Option<(u32, u32)> {
        if keywords.is_empty() {
            return None;
        }
        let line = self.sources.line(loc.file, loc.pos.line)?;
        let mut rest = line.get(..(loc.pos.col as usize).saturating_sub(1))?.trim_end();
        for lifetime in ["automatic", "static"] {
            if let Some(before) = strip_word(rest, lifetime) {
                rest = before.trim_end();
                break;
            }
        }
        let before = keywords.iter().find_map(|keyword| strip_word(rest, keyword))?;
        Some(self.sources.position(loc.file, Pos { line: loc.pos.line, col: before.len() as u32 + 1 }))
    }

    fn node_location(&self, id: NodeId) -> Value {
        let node = &self.nodes[id];
        self.lsp_location(node.loc, &node.name, &[])
    }

    fn is_scope(&self, id: NodeId) -> bool {
        matches!(
            self.nodes[id].kind,
            Kind::Instance { .. } | Kind::InstanceArray { .. } | Kind::InterfacePort { .. } | Kind::GenBlock { .. } | Kind::GenArray | Kind::Package
        )
    }

    /// `scopeHasVisibleChildren`.
    fn has_visible_children(&self, id: NodeId) -> bool {
        self.nodes[id].children.iter().any(|&child| {
            matches!(
                self.nodes[child].kind,
                Kind::Instance { .. }
                    | Kind::InstanceArray { .. }
                    | Kind::Param { .. }
                    | Kind::InterfacePort { .. }
                    | Kind::GenBlock { .. }
                    | Kind::GenArray
                    | Kind::Value { .. }
            )
        })
    }

    fn definition(&self, id: NodeId) -> Option<&Definition> {
        match self.nodes[id].kind {
            Kind::Instance { def: Some(def) } => self.definitions.get(&def),
            _ => None,
        }
    }

    fn definition_location(&self, definition: &Definition) -> Value {
        let keywords: &[&str] = match definition.kind {
            "Interface" => &["interface"],
            "Program" => &["program"],
            _ => &["module", "macromodule"],
        };
        self.lsp_location(definition.loc, &definition.name, keywords)
    }

    /// `toInstance`.
    fn instance_item(&self, id: NodeId, filled: bool) -> Value {
        let node = &self.nodes[id];
        let (decl_name, decl_loc, decl_kind) = match self.definition(id) {
            Some(definition) => (definition.name.clone(), self.definition_location(definition), definition.kind),
            None => (node.name.clone(), self.node_location(id), "Module"),
        };
        let children = if filled { self.scope_children(id) } else { Vec::new() };
        let has_children = if filled { !children.is_empty() } else { self.has_visible_children(id) };
        json!({
            "kind": "Instance",
            "instName": node.label,
            "instLoc": self.node_location(id),
            "declName": decl_name,
            "declLoc": decl_loc,
            "declKind": decl_kind,
            "hasChildren": has_children,
            "children": children,
        })
    }

    /// The elements of an instance array that are instances, in the order the view lists them:
    /// descending ranges from the left bound down.
    fn array_elements(&self, id: NodeId) -> Vec<Value> {
        let node = &self.nodes[id];
        let descending = matches!(node.kind, Kind::InstanceArray { left, right, .. } if left > right);
        let mut elements: Vec<&NodeId> = node.children.iter().filter(|&&child| matches!(self.nodes[child].kind, Kind::Instance { .. })).collect();
        if descending {
            elements.reverse();
        }
        elements.into_iter().map(|&child| self.instance_item(child, false)).collect()
    }

    /// `handleInstanceArray`.
    fn array_item(&self, id: NodeId, out: &mut Vec<Value>) {
        let elements = self.array_elements(id);
        let Some(first) = elements.first() else {
            return;
        };
        let Kind::InstanceArray { text, .. } = &self.nodes[id].kind else {
            return;
        };
        let decl_name = format!("{}{text}", first["declName"].as_str().unwrap_or(""));
        let (decl_loc, decl_kind) = (first["declLoc"].clone(), first["declKind"].clone());
        out.push(json!({
            "kind": "InstanceArray",
            "instName": self.nodes[id].name,
            "instLoc": self.node_location(id),
            "declName": decl_name,
            "declLoc": decl_loc,
            "declKind": decl_kind,
            "hasChildren": true,
            "children": elements,
        }));
    }

    /// `handleBlockScope`: a block is left out when it has nothing to show.
    fn block_item(&self, id: NodeId, check_instantiated: bool, out: &mut Vec<Value>) {
        if check_instantiated && matches!(self.nodes[id].kind, Kind::GenBlock { uninstantiated: true }) {
            return;
        }
        let children = self.scope_children(id);
        if children.is_empty() {
            return;
        }
        out.push(json!({
            "kind": "Scope",
            "instName": self.nodes[id].label,
            "instLoc": self.node_location(id),
            "children": children,
        }));
    }

    fn block_array_entries(&self, id: NodeId) -> Vec<Value> {
        let mut entries = Vec::new();
        for &child in &self.nodes[id].children {
            if matches!(self.nodes[child].kind, Kind::GenBlock { .. }) {
                self.block_item(child, false, &mut entries);
            }
        }
        entries
    }

    /// `getInterfacePortChildren` for a port that is not an array: the members of the connected
    /// interface's modport, or of the interface itself.
    fn interface_children(&self, id: NodeId) -> Vec<Value> {
        let Kind::InterfacePort { modport, conn, .. } = &self.nodes[id].kind else {
            return Vec::new();
        };
        let (instance, connected_modport) = conn.unwrap_or((None, None));
        let instance = instance.and_then(|addr| self.by_addr.get(&addr)).copied();
        let scope = connected_modport.and_then(|addr| self.by_addr.get(&addr)).copied().or_else(|| match instance {
            Some(instance) if !modport.is_empty() => self.nodes[instance]
                .children
                .iter()
                .copied()
                .find(|&child| matches!(self.nodes[child].kind, Kind::Modport) && self.nodes[child].name == *modport),
            other => other,
        });
        scope.map_or_else(Vec::new, |scope| self.scope_children(scope))
    }

    /// `getScopeChildren`: what a scope shows, in member order.
    fn scope_children(&self, id: NodeId) -> Vec<Value> {
        let mut out = Vec::new();
        for &child in &self.nodes[id].children {
            let node = &self.nodes[child];
            match &node.kind {
                Kind::Instance { .. } => out.push(self.instance_item(child, false)),
                Kind::InstanceArray { .. } => self.array_item(child, &mut out),
                Kind::Param { ty, value } => {
                    let mut item = json!({
                        "kind": "Param",
                        "instName": node.label,
                        "instLoc": self.node_location(child),
                        "type": self.type_string(ty, node.loc),
                    });
                    if let Some(value) = value {
                        item["value"] = json!(constant_text(value));
                    }
                    out.push(item);
                }
                Kind::TypeParam { value } => {
                    let mut item = json!({
                        "kind": "Param",
                        "instName": node.label,
                        "instLoc": self.node_location(child),
                        "type": "type",
                    });
                    if let Some(value) = value.as_ref().filter(|ty| !matches!(self.canon(ty), Ty::Error)) {
                        item["value"] = json!(self.type_string(value, None));
                    }
                    out.push(item);
                }
                Kind::Value { ty, addr } => {
                    let port = self.ports.get(addr);
                    let spelled = self.type_string(ty, node.loc);
                    out.push(json!({
                        "kind": if port.is_some() { "Port" } else { "Logic" },
                        "instName": node.label,
                        "instLoc": self.node_location(child),
                        "type": match port {
                            Some(dir) => format!("{} {spelled}", dir.word()),
                            None => spelled,
                        },
                    }));
                }
                Kind::ModportPort { ty, dir } => out.push(json!({
                    "kind": "Logic",
                    "instName": node.label,
                    "instLoc": self.node_location(child),
                    "type": format!("{} {}", dir.word(), self.type_string(ty, node.loc)),
                })),
                Kind::InterfacePort { interface, modport, .. } => {
                    let type_name = match (interface.is_empty(), modport.is_empty()) {
                        (_, true) => interface.clone(),
                        (true, false) => modport.clone(),
                        (false, false) => format!("{interface}.{modport}"),
                    };
                    out.push(json!({
                        "kind": "InterfacePort",
                        "instName": node.label,
                        "instLoc": self.node_location(child),
                        "type": type_name,
                        "children": self.interface_children(child),
                    }));
                }
                Kind::GenBlock { .. } => self.block_item(child, true, &mut out),
                Kind::GenArray => {
                    let entries = self.block_array_entries(child);
                    if !entries.is_empty() {
                        out.push(json!({
                            "kind": "ScopeArray",
                            "instName": node.label,
                            "instLoc": self.node_location(child),
                            "children": entries,
                        }));
                    }
                }
                Kind::Package | Kind::Modport => {}
            }
        }
        out
    }

    /// `getChildrenForScopeSymbol`.
    fn children_for(&self, id: NodeId) -> Vec<Value> {
        match self.nodes[id].kind {
            Kind::InstanceArray { .. } => self.array_elements(id),
            Kind::GenArray => self.block_array_entries(id),
            Kind::InterfacePort { .. } => self.interface_children(id),
            Kind::Instance { .. } | Kind::GenBlock { .. } | Kind::Package => self.scope_children(id),
            _ => Vec::new(),
        }
    }

    /// The roots: the top instances with their children, then the packages, empty.
    fn roots(&self) -> Vec<Value> {
        let mut out: Vec<Value> = self.tops.iter().map(|&top| self.instance_item(top, true)).collect();
        for &package in &self.packages {
            let node = &self.nodes[package];
            let location = self.lsp_location(node.loc, &node.name, &["package"]);
            out.push(json!({
                "kind": "Package",
                "instName": node.name,
                "instLoc": location,
                "declName": node.name,
                "declLoc": location,
                "declKind": "Package",
                "hasChildren": false,
                "children": [],
            }));
        }
        out
    }

    /// The node a path names, or failing that the longest prefix of it that is one (a path that goes on
    /// to something the view does not know still has scopes to open on the way).
    fn find(&self, path: &str) -> Option<NodeId> {
        // (The paths here are joined from plain names, as the view joins them; slang's own spelling of
        // an escaped name, which the trace hands over, is understood as well.)
        let path = &*crate::graph::plain_path(path);
        let mut end = path.len();
        loop {
            if let Some(&id) = self.by_path.get(&path[..end]) {
                return Some(id);
            }
            end = path[..end].rfind(['.', ':', '['])?;
            // (`::` is cut whole.)
            if path[..end].ends_with(':') {
                end -= 1;
            }
        }
    }

    /// `slang.getScope`: the children of the scope at `path`; none when there is no such scope.
    pub fn get_scope(&self, path: &str) -> Vec<Value> {
        if path.is_empty() {
            return self.roots();
        }
        match self.by_path.get(&*crate::graph::plain_path(path)) {
            Some(&id) if self.is_scope(id) => self.children_for(id),
            _ => Vec::new(),
        }
    }

    /// `slang.getScopes`: the roots, then the scopes on the way down to `path`, each with its children
    /// and its own path.
    pub fn get_scopes(&self, path: &str) -> Vec<Value> {
        let mut steps = vec![json!({ "path": "", "children": self.roots() })];
        if path.is_empty() {
            return steps;
        }
        let Some(found) = self.find(path) else {
            return steps;
        };
        let mut chain = Vec::new();
        let mut at = Some(found);
        while let Some(id) = at {
            chain.push(id);
            at = self.parents[id];
        }
        chain.reverse();
        for id in chain {
            // A package is a step only for a path that goes through it with `::`; the lookup does not
            // find the bare name.
            let reached_through = matches!(self.nodes[id].kind, Kind::Package) && id == found;
            if self.is_scope(id) && !reached_through {
                steps.push(json!({ "path": self.nodes[id].path, "children": self.children_for(id) }));
            }
        }
        steps
    }

}

fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let before = text.strip_suffix(word)?;
    match before.chars().next_back() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => None,
        _ => Some(before),
    }
}

/// Answers `slang.getScope` and `slang.getScopes`, given the path as the command's one argument.
pub fn run(hier: &Hier, command: &str, arguments: &[Value]) -> Result<Value, String> {
    let path = arguments.first().and_then(Value::as_str).ok_or_else(|| format!("{command} needs a path"))?;
    match command {
        GET_SCOPE => Ok(Value::Array(hier.get_scope(path))),
        GET_SCOPES => Ok(Value::Array(hier.get_scopes(path))),
        _ => Err(format!("unknown command {command}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bit_constants_are_written_as_a_digit() {
        assert_eq!(constant_text("1'b1"), "1");
        assert_eq!(constant_text("1'b0"), "0");
        assert_eq!(constant_text("1'sb1"), "1");
        assert_eq!(constant_text("1'bx"), "1'bx");
        assert_eq!(constant_text("2'b10"), "2'b10");
        assert_eq!(constant_text("42"), "42");
    }

    #[test]
    fn enum_values_are_decimal_with_a_base() {
        assert_eq!(decimal_with_base("3'b100", 3, false), "3'd4");
        assert_eq!(decimal_with_base("0", 2, false), "2'd0");
        assert_eq!(decimal_with_base("5", 32, true), "32'sd5");
        assert_eq!(decimal_with_base("-1", 32, true), "-32'sd1");
        assert_eq!(decimal_with_base("2'bx", 2, false), "2'bx");
    }

    #[test]
    fn a_written_sign_is_read_from_the_text_before_a_name() {
        assert_eq!(written_sign("    parameter int unsigned "), Some(false));
        assert_eq!(written_sign("  int signed "), Some(true));
        assert_eq!(written_sign("  input logic "), None);
        assert_eq!(written_sign("  int designed "), None);
    }
}

//! The elaborated design as slang's `--ast-json` writes it, read into the few shapes svtrace needs:
//! symbols with their declarations, the expression trees of assignments and procedural blocks, and
//! the port connections of instances. The JSON `Value` is dropped once this is built.

use std::collections::HashMap;

use serde_json::Value;

/// The address slang prints for a symbol. References in the dump are `"<addr> <name>"`.
pub type Addr = u64;

/// A place in a file, 1-based as in the dump (the column counts bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// A range of source text; `end` is one past the last character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Range {
    pub file: u32,
    pub start: Pos,
    pub end: Pos,
}

/// Where a symbol is declared: the start of its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: u32,
    pub pos: Pos,
}

/// How many bits a type has. Named types are links into the type table until the width is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Width {
    Bits(u32),
    /// A typedef or an enum, by address.
    Link(Addr),
    /// A packed struct: its fields side by side.
    Sum(Vec<FieldTy>),
    /// A packed union: its widest member.
    Max(Vec<FieldTy>),
    /// A packed array `[left:right]` of the element.
    Times(i64, i64, Box<Width>),
    /// An unpacked array `[left:right]` of the element. It has no packed bits (its width is 0, like any
    /// type that isn't packed): only a path into it is checked against it.
    Unpacked(i64, i64, Box<Width>),
    /// An unpacked struct or union, likewise: its fields by name.
    Record(Vec<FieldTy>),
}

/// A field of a packed struct or union: where its bits start (counting from the least significant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldTy {
    pub name: String,
    pub offset: u32,
    pub ty: Width,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Variable,
    Net,
    Parameter,
    EnumValue,
    Specparam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    InOut,
    Ref,
}

pub struct Symbol {
    pub addr: Addr,
    pub name: String,
    pub loc: Option<Loc>,
    pub item: Item,
}

pub enum Item {
    /// A variable, net, parameter or enum value.
    /// `init` is the declaration's initializer as an assignment to the symbol (`wire w = a;`, `logic v = b;`).
    Value { kind: ValueKind, value: Option<String>, ty: Width, init: Option<Node> },
    /// A name in a modport, standing for the interface's own signal.
    ModportPort { internal: Option<Addr> },
    /// A package, an enum type, a block, a modport, a compilation unit: anything that holds members.
    Scope { kind: ScopeKind, members: Vec<Symbol> },
    Instance { members: Vec<Symbol>, connections: Vec<Connection> },
    Assign(Node),
    Procedural { combinational: bool, body: Node },
    /// Everything else (genvars, ports, subroutines, ...): not traced.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Package,
    /// An array of instances; its elements are numbered from `lower`.
    InstanceArray { lower: i64 },
    GenerateBlockArray,
    /// One block of a generate construct. `index` is the loop variable's value in a loop iteration.
    GenerateBlock { index: Option<i64> },
    /// Holds the symbols declared in a compilation unit or the root: nothing is added to a path.
    Unit,
    Other,
}

pub struct Connection {
    pub port: Port,
    pub expr: Option<Node>,
}

pub enum Port {
    Port { direction: Direction, internal: Option<Addr> },
    Interface { name: String, instance: Option<Addr>, modport: Option<Addr> },
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A named or hierarchical reference to a symbol.
    Value { addr: Addr, width: Width },
    /// `left = right` is `children[0..2]`; a compound one (`+=`) has its own read of the target on the right.
    Assign { compound: bool, width: Width },
    /// A bit, part or element select: the value, then the index expressions.
    Select(Sel),
    /// A struct field access: the value, then the field.
    Member { field: Addr },
    Concat,
    /// An assignment pattern `'{...}`: one expression per field named in `members` (the fields' addresses),
    /// or, with no `members`, one per field in order; `default` says the last child is `default: x`.
    Pattern { members: Option<Vec<Addr>>, default: bool },
    /// `{n{x}}`: the count, then the concatenation.
    Replicate,
    Convert { implicit: bool },
    Literal,
    Call,
    /// `if`: the conditions, the then branch, and the else branch when there is one.
    /// `taken` is the branch a constant condition always takes.
    If { conditions: usize, taken: Option<bool> },
    /// `case`: the selector, one `CaseItem` per item, and the default statement when there is one.
    Case,
    CaseItem { expressions: usize },
    /// A statement under an event control (`@(posedge clk)`): the event list, then the statement.
    /// The signals in the event list are clocks of what the statement assigns, unless the list has no
    /// edge (`level`, as in `always @(a or b)`): then it only says when to work the statement out again.
    Timed { level: bool },
    /// A loop: `children[..init]` run once first (a `for`'s initializers), the next `conds` are the
    /// condition (`while`, `for`'s stop test, `repeat`'s count) and the rest (steps, body) run under it.
    Loop { init: usize, conds: usize },
    /// A statement that is not one of the above (a block, a call, an assertion, a declaration): what it
    /// reads is not read by the statements after it.
    Stmt,
    /// The right side the dump gives a connection or argument that is written to (an output port's
    /// `x = <the port>`, a task's output argument): nothing in the source.
    Empty,
    /// Anything else, read through its children in source order.
    Other,
}

/// What a select picks, when its indices are constants (the dump's `constant` of each index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sel {
    /// An index that isn't a constant, or a constant that couldn't be read.
    Whole,
    Index(i64),
    /// `[left:right]`.
    Range(i64, i64),
    /// `[base +: width]`.
    Up(i64, i64),
    /// `[base -: width]`.
    Down(i64, i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub range: Option<Range>,
    pub kind: NodeKind,
    pub children: Vec<Node>,
    /// The type of the expression (`Bits(0)` for a statement or an unpacked type).
    pub width: Width,
    /// For a `Value`: the bits of the signal this use touches, when a constant select or field
    /// access around it narrows them; None is the whole signal.
    pub span: Option<crate::bits::Span>,
}

pub struct Design {
    pub files: Vec<String>,
    pub root: Vec<Symbol>,
    /// Typedefs and enums by address, for the width of a named type.
    pub types: HashMap<Addr, Width>,
    /// Where each packed struct field starts, by the field symbol's address.
    pub fields: HashMap<Addr, u32>,
}

/// Reads `{"design": ..., "definitions": [...]}`.
pub fn read(json: &Value) -> Result<Design, String> {
    crate::deep(|| {
        let root = json.get("design").ok_or("no \"design\" in the dump")?;
        let mut reader = Reader::default();
        let root = reader.members(root);
        Ok(Design { files: reader.files, root, types: reader.types, fields: reader.fields })
    })
}

#[derive(Default)]
struct Reader {
    files: Vec<String>,
    file_ids: HashMap<String, u32>,
    types: HashMap<Addr, Width>,
    fields: HashMap<Addr, u32>,
    /// The targets of the compound assignments being read, innermost last: what `LValueReference` stands for.
    targets: Vec<Node>,
    /// The type parameters of the instance bodies being read, as the dump names them in a type
    /// (`sub.T`), with the type each stands for in this instance.
    type_params: HashMap<String, Width>,
}

impl Reader {
    fn file(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.file_ids.get(name) {
            return id;
        }
        let id = self.files.len() as u32;
        self.files.push(name.to_string());
        self.file_ids.insert(name.to_string(), id);
        id
    }

    fn members(&mut self, scope: &Value) -> Vec<Symbol> {
        let Some(Value::Array(members)) = scope.get("members") else {
            return Vec::new();
        };
        members.iter().filter_map(|member| self.symbol(member)).collect()
    }

    fn loc(&mut self, v: &Value) -> Option<Loc> {
        let file = self.file(v.get("source_file")?.as_str()?);
        let line = v.get("source_line")?.as_u64()? as u32;
        let col = v.get("source_column")?.as_u64()? as u32;
        Some(Loc { file, pos: Pos { line, col } })
    }

    fn symbol(&mut self, v: &Value) -> Option<Symbol> {
        let kind = text(v, "kind");
        let name = text(v, "name").to_string();
        let addr = v.get("addr").and_then(Value::as_u64).unwrap_or(0);
        let loc = self.loc(v);
        let item = match kind {
            "Variable" | "Net" | "Parameter" | "EnumValue" | "Specparam" => {
                let kind = match kind {
                    "Variable" => ValueKind::Variable,
                    "Net" => ValueKind::Net,
                    "Parameter" => ValueKind::Parameter,
                    "EnumValue" => ValueKind::EnumValue,
                    _ => ValueKind::Specparam,
                };
                let ty = v.get("type").map(|ty| self.width(ty)).unwrap_or(Width::Bits(0));
                let init = match (kind, v.get("initializer")) {
                    (ValueKind::Net | ValueKind::Variable, Some(init)) => Some(self.initializer(addr, &name, loc, &ty, init)),
                    _ => None,
                };
                Item::Value { kind, value: v.get("value").and_then(Value::as_str).map(str::to_string), ty, init }
            }
            "ModportPort" => Item::ModportPort { internal: v.get("internalSymbol").and_then(link) },
            "Instance" => {
                let members = v.get("body").map(|body| self.body(body)).unwrap_or_default();
                let connections = match v.get("connections") {
                    Some(Value::Array(list)) => list.iter().map(|c| self.connection(c)).collect(),
                    _ => Vec::new(),
                };
                Item::Instance { members, connections }
            }
            "ContinuousAssign" => Item::Assign(self.node(v.get("assignment")?)),
            "ProceduralBlock" => {
                let body = v.get("body")?;
                Item::Procedural { combinational: is_combinational(v), body: self.node(body) }
            }
            "TypeAlias" => {
                if let Some(target) = v.get("target") {
                    let width = self.width(target);
                    self.types.insert(addr, width);
                }
                Item::Other
            }
            "EnumType" => {
                if let Some(base) = v.get("baseType") {
                    let width = self.width(base);
                    self.types.insert(addr, width);
                }
                Item::Scope { kind: ScopeKind::Other, members: self.members(v) }
            }
            "Package" => Item::Scope { kind: ScopeKind::Package, members: self.members(v) },
            "Root" | "CompilationUnit" => {
                Item::Scope { kind: ScopeKind::Unit, members: self.members(v) }
            }
            "InstanceArray" => {
                let lower = v.get("range").and_then(Value::as_str).and_then(lower_bound).unwrap_or(0);
                Item::Scope { kind: ScopeKind::InstanceArray { lower }, members: self.members(v) }
            }
            "GenerateBlockArray" => {
                Item::Scope { kind: ScopeKind::GenerateBlockArray, members: self.members(v) }
            }
            "GenerateBlock" => {
                let members = self.members(v);
                // A loop iteration declares the loop variable as its first member, a parameter
                // holding the iteration's value: that is the number in the path.
                let index = (text(v, "branchKind") == "LoopIteration").then(|| {
                    members.iter().find_map(|member| match &member.item {
                        Item::Value { kind: ValueKind::Parameter, value, .. } => value.as_deref()?.parse().ok(),
                        _ => None,
                    })
                });
                Item::Scope { kind: ScopeKind::GenerateBlock { index: index.flatten() }, members }
            }
            // A subroutine's own variables (its return value, its locals) are not reachable by path.
            "Subroutine" => Item::Other,
            // Block-like symbols whose members matter for paths: modports, named statement blocks.
            _ if v.get("members").is_some() => {
                Item::Scope { kind: ScopeKind::Other, members: self.members(v) }
            }
            _ => Item::Other,
        };
        Some(Symbol { addr, name, loc, item })
    }

    /// The members of an instance's body. A port or signal of a type parameter's type has the type
    /// `"<addr> sub.T"`, where the address is of nothing the dump writes out; the parameter itself
    /// (`T`, a member of the body) says which type it is in this instance.
    fn body(&mut self, body: &Value) -> Vec<Symbol> {
        let mut shadowed = Vec::new();
        if let Some(Value::Array(members)) = body.get("members") {
            for parameter in members.iter().filter(|member| text(member, "kind") == "TypeParameter") {
                let Some(ty) = parameter.get("type") else { continue };
                let name = format!("{}.{}", text(body, "name"), text(parameter, "name"));
                let width = self.width(ty);
                shadowed.push((name.clone(), self.type_params.insert(name, width)));
            }
        }
        let members = self.members(body);
        for (name, outer) in shadowed.into_iter().rev() {
            match outer {
                Some(width) => self.type_params.insert(name, width),
                None => self.type_params.remove(&name),
            };
        }
        members
    }

    /// A declaration's initializer as the assignment it is, to the symbol at its declaration.
    fn initializer(&mut self, addr: Addr, name: &str, loc: Option<Loc>, ty: &Width, init: &Value) -> Node {
        let right = self.node(init);
        let range = loc.map(|loc| Range {
            file: loc.file,
            start: loc.pos,
            end: Pos { line: loc.pos.line, col: loc.pos.col + name.len() as u32 },
        });
        let left = Node { range, kind: NodeKind::Value { addr, width: ty.clone() }, children: Vec::new(), width: ty.clone(), span: None };
        Node { range: right.range, kind: NodeKind::Assign { compound: false, width: ty.clone() }, children: vec![left, right], width: ty.clone(), span: None }
    }

    /// `x++`, `--x`: a compound assignment to the operand, which it reads as well. Anything else: None.
    fn increment(&mut self, v: &Value) -> Option<Node> {
        if text(v, "kind") != "UnaryOp" || !matches!(text(v, "op"), "Preincrement" | "Predecrement" | "Postincrement" | "Postdecrement") {
            return None;
        }
        let target = self.node(v.get("operand")?);
        let width = v.get("type").map(|ty| self.width(ty)).unwrap_or(Width::Bits(0));
        let read = target.clone();
        let kind = NodeKind::Assign { compound: true, width: width.clone() };
        Some(Node { range: self.range(v), kind, children: vec![target, read], width, span: None })
    }

    /// An expression that stands as a statement (or a loop's step): `i++` is an assignment there.
    fn statement_expr(&mut self, v: &Value) -> Node {
        match self.increment(v) {
            Some(node) => node,
            None => self.node(v),
        }
    }

    fn connection(&mut self, v: &Value) -> Connection {
        let mut expr = v.get("expr").map(|expr| self.node(expr));
        let port = match v.get("port") {
            Some(port) => match text(port, "kind") {
                "Port" => Port::Port {
                    direction: match text(port, "direction") {
                        "In" => Direction::In,
                        "Out" => Direction::Out,
                        "InOut" => Direction::InOut,
                        _ => Direction::Ref,
                    },
                    internal: port.get("internalSymbol").and_then(link),
                },
                "InterfacePort" => Port::Interface {
                    name: text(port, "name").to_string(),
                    instance: v.get("ifaceInstance").and_then(link),
                    modport: v.get("modport").and_then(link),
                },
                _ => Port::Other,
            },
            None => Port::Other,
        };
        // An `inout` connection comes as an output's does, `x = <the port>`; a `ref` one as the bare
        // expression. Both are written through the port as well as read, so both get the output's shape.
        if let (Port::Port { direction: Direction::InOut | Direction::Ref, .. }, Some(outside)) = (&port, &expr) {
            if !matches!(outside.kind, NodeKind::Assign { .. }) {
                let outside = expr.take().expect("there is an expression");
                let empty = Node { range: None, kind: NodeKind::Empty, children: Vec::new(), width: Width::Bits(0), span: None };
                let (range, width) = (outside.range, outside.width.clone());
                let kind = NodeKind::Assign { compound: false, width: width.clone() };
                expr = Some(Node { range, kind, children: vec![outside, empty], width, span: None });
            }
        }
        Connection { port, expr }
    }

    /// A call's output arguments come as `x = <nothing>`: what the subroutine does isn't read, so the
    /// call's other arguments stand for what it writes there (and with none, the call itself does, a
    /// value no signal feeds).
    fn output_arguments(&mut self, range: Option<Range>, arguments: &mut [Node]) {
        let is_output = |node: &Node| {
            matches!(node.kind, NodeKind::Assign { .. }) && node.children.get(1).is_some_and(|right| right.kind == NodeKind::Empty)
        };
        if !arguments.iter().any(is_output) {
            return;
        }
        let inputs: Vec<Node> = arguments.iter().filter(|argument| !is_output(argument)).cloned().collect();
        for argument in arguments.iter_mut().filter(|argument| is_output(argument)) {
            argument.children[1] = Node { range, kind: NodeKind::Call, children: inputs.clone(), width: Width::Bits(0), span: None };
        }
    }

    fn range(&mut self, v: &Value) -> Option<Range> {
        let file = self.file(v.get("source_file_start")?.as_str()?);
        let number = |key: &str| v.get(key).and_then(Value::as_u64).map(|n| n as u32);
        Some(Range {
            file,
            start: Pos { line: number("source_line_start")?, col: number("source_column_start")? },
            end: Pos { line: number("source_line_end")?, col: number("source_column_end")? },
        })
    }

    fn node(&mut self, v: &Value) -> Node {
        let kind = text(v, "kind");
        if kind == "LValueReference" {
            if let Some(target) = self.targets.last() {
                return target.clone();
            }
        }
        let range = self.range(v);
        let child = |reader: &mut Self, key: &str| -> Vec<Node> {
            v.get(key).map(|child| vec![reader.node(child)]).unwrap_or_default()
        };
        let width = v.get("type").map(|ty| self.width(ty)).unwrap_or(Width::Bits(0));
        let (kind, children) = match kind {
            "NamedValue" | "HierarchicalValue" => {
                let addr = v.get("symbol").and_then(link).unwrap_or(0);
                let width = v.get("type").map(|ty| self.width(ty)).unwrap_or(Width::Bits(0));
                (NodeKind::Value { addr, width }, Vec::new())
            }
            "Assignment" => {
                let width = v.get("type").map(|ty| self.width(ty)).unwrap_or(Width::Bits(0));
                let mut children = child(self, "left");
                let compound = v.get("op").is_some();
                // The right side of `a += b` is `<a> + b`, the target standing as `LValueReference`: the
                // use of the target is a read of it.
                if let (true, Some(target)) = (compound, children.first()) {
                    self.targets.push(target.clone());
                }
                children.extend(child(self, "right"));
                if compound {
                    self.targets.pop();
                }
                (NodeKind::Assign { compound, width }, children)
            }
            "ElementSelect" | "RangeSelect" => {
                let mut children = child(self, "value");
                for key in ["selector", "left", "right"] {
                    children.extend(child(self, key));
                }
                let constant = |key: &str| v.get(key).and_then(|index| constant(index.get("constant")?.as_str()?));
                let sel = if text(v, "kind") == "ElementSelect" {
                    constant("selector").map_or(Sel::Whole, Sel::Index)
                } else {
                    match (constant("left"), constant("right"), text(v, "selectionKind")) {
                        (Some(l), Some(r), "Simple") => Sel::Range(l, r),
                        (Some(l), Some(r), "IndexedUp") => Sel::Up(l, r),
                        (Some(l), Some(r), "IndexedDown") => Sel::Down(l, r),
                        _ => Sel::Whole,
                    }
                };
                (NodeKind::Select(sel), children)
            }
            "MemberAccess" => {
                let field = v.get("member").and_then(link).unwrap_or(0);
                (NodeKind::Member { field }, child(self, "value"))
            }
            "Concatenation" => (NodeKind::Concat, self.list(v.get("operands"))),
            "SimpleAssignmentPattern" => {
                (NodeKind::Pattern { members: None, default: false }, self.list(v.get("elements")))
            }
            "StructuredAssignmentPattern" if is_empty(v.get("indexSetters")) && is_empty(v.get("typeSetters")) => {
                let mut members = Vec::new();
                let mut children = Vec::new();
                if let Some(Value::Array(setters)) = v.get("memberSetters") {
                    for setter in setters {
                        if let Some(expr) = setter.get("expr") {
                            members.push(setter.get("member").and_then(link).unwrap_or(0));
                            children.push(self.node(expr));
                        }
                    }
                }
                let default = v.get("defaultSetter").map(|setter| self.node(setter));
                let has_default = default.is_some();
                children.extend(default);
                (NodeKind::Pattern { members: Some(members), default: has_default }, children)
            }
            "Replication" => {
                let mut children = child(self, "count");
                children.extend(child(self, "concat"));
                (NodeKind::Replicate, children)
            }
            "Conversion" => {
                let children = child(self, "operand");
                // The dump doesn't say whether a conversion was written; slang gives an implicit
                // one the range of its operand, and a cast or a streaming concatenation a wider one.
                let implicit = match (range, children.first().and_then(|operand| operand.range)) {
                    (Some(outer), Some(inner)) => outer == inner,
                    _ => true,
                };
                (NodeKind::Convert { implicit }, children)
            }
            "IntegerLiteral" | "UnbasedUnsizedIntegerLiteral" => (NodeKind::Literal, Vec::new()),
            "Call" => {
                let mut children = self.generic_children(v);
                self.output_arguments(range, &mut children);
                (NodeKind::Call, children)
            }
            "EmptyArgument" => (NodeKind::Empty, Vec::new()),
            "ExpressionStatement" => (NodeKind::Stmt, v.get("expr").map(|expr| vec![self.statement_expr(expr)]).unwrap_or_default()),
            "ImmediateAssertion" => {
                // The actions run depending on the assertion's condition.
                let mut children = child(self, "cond");
                let conditions = children.len();
                children.extend(child(self, "ifTrue"));
                children.extend(child(self, "ifFalse"));
                (NodeKind::If { conditions, taken: None }, children)
            }
            "Wait" => {
                let mut children = child(self, "cond");
                let conditions = children.len();
                children.extend(child(self, "stmt"));
                (NodeKind::If { conditions, taken: None }, children)
            }
            "ForLoop" => {
                let mut children: Vec<Node> = match v.get("initializers") {
                    Some(Value::Array(list)) => list.iter().map(|item| self.statement_expr(item)).collect(),
                    _ => Vec::new(),
                };
                let init = children.len();
                children.extend(child(self, "stopExpr"));
                let conds = children.len() - init;
                if let Some(Value::Array(steps)) = v.get("steps") {
                    for step in steps {
                        children.push(self.statement_expr(step));
                    }
                }
                children.extend(child(self, "body"));
                (NodeKind::Loop { init, conds }, children)
            }
            "WhileLoop" | "DoWhileLoop" | "RepeatLoop" | "ForeverLoop" | "ForeachLoop" => {
                // (A `foreach`'s array only gives the range of the loop variable.)
                let mut children = match kind {
                    "WhileLoop" | "DoWhileLoop" => child(self, "cond"),
                    "RepeatLoop" => child(self, "count"),
                    _ => Vec::new(),
                };
                let conds = children.len();
                children.extend(child(self, "body"));
                (NodeKind::Loop { init: 0, conds }, children)
            }
            "Block" | "List" | "VariableDeclaration" | "Return" | "Break" | "Continue" | "Disable" | "Empty" | "Invalid"
            | "ProceduralAssign" | "ProceduralDeassign" | "ConcurrentAssertion" | "WaitOrder" | "WaitFork" | "DisableFork"
            | "EventTrigger" | "RandCase" | "RandSequence" | "ProceduralChecker" | "PatternCase" => {
                (NodeKind::Stmt, self.generic_children(v))
            }
            "Conditional" => {
                let mut children = Vec::new();
                let mut taken = None;
                if let Some(Value::Array(conditions)) = v.get("conditions") {
                    for condition in conditions {
                        children.extend(condition.get("expr").map(|expr| self.node(expr)));
                    }
                    if let [only] = conditions.as_slice() {
                        taken = only["expr"].get("constant").and_then(Value::as_str).and_then(truth);
                    }
                }
                let conditions = children.len();
                children.extend(child(self, "ifTrue"));
                children.extend(child(self, "ifFalse"));
                (NodeKind::If { conditions, taken }, children)
            }
            "Timed" => {
                let mut children = child(self, "timing");
                if children.is_empty() {
                    children.push(Node { range: None, kind: NodeKind::Other, children: Vec::new(), width: Width::Bits(0), span: None });
                }
                children.extend(child(self, "stmt"));
                (NodeKind::Timed { level: v.get("timing").is_some_and(is_level_list) }, children)
            }
            "Case" => {
                let mut children = child(self, "expr");
                if let Some(Value::Array(items)) = v.get("items") {
                    for item in items {
                        let expressions = self.list(item.get("expressions"));
                        let count = expressions.len();
                        let mut item_children = expressions;
                        item_children.extend(item.get("stmt").map(|stmt| self.node(stmt)));
                        children.push(Node {
                            range: None,
                            kind: NodeKind::CaseItem { expressions: count },
                            children: item_children,
                            width: Width::Bits(0),
                            span: None,
                        });
                    }
                }
                children.extend(child(self, "defaultCase"));
                (NodeKind::Case, children)
            }
            _ => (NodeKind::Other, self.generic_children(v)),
        };
        Node { range, kind, children, width, span: None }
    }

    fn list(&mut self, v: Option<&Value>) -> Vec<Node> {
        match v {
            Some(Value::Array(list)) => list.iter().map(|item| self.node(item)).collect(),
            _ => Vec::new(),
        }
    }

    /// The nodes found under a node's fields, in source order. A field holding a type is skipped.
    fn generic_children(&mut self, v: &Value) -> Vec<Node> {
        let mut found = Vec::new();
        if let Value::Object(fields) = v {
            for (key, field) in fields {
                if key != "type" {
                    self.collect_nodes(field, &mut found);
                }
            }
        }
        // (The dump's fields come out alphabetically: "stmt" before "timing".)
        found.sort_by_key(|node: &Node| node.range.map(|r| (r.file, r.start)));
        found
    }

    fn collect_nodes(&mut self, v: &Value, found: &mut Vec<Node>) {
        match v {
            Value::Object(fields) if fields.contains_key("kind") => found.push(self.node(v)),
            Value::Object(fields) => {
                for field in fields.values() {
                    self.collect_nodes(field, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.collect_nodes(item, found);
                }
            }
            _ => {}
        }
    }

    fn width(&mut self, ty: &Value) -> Width {
        match ty {
            Value::String(s) => {
                let (addr, name) = s.split_once(' ').unwrap_or((s, ""));
                let Ok(addr) = addr.parse::<Addr>() else { return Width::Bits(0) };
                // (A type parameter's type, which the dump only names: see `body`.)
                if let Some(bound) = self.type_params.get(name) {
                    self.types.entry(addr).or_insert_with(|| bound.clone());
                }
                Width::Link(addr)
            }
            Value::Object(_) => match text(ty, "kind") {
                "ScalarType" => Width::Bits(1),
                "PredefinedIntegerType" => Width::Bits(match text(ty, "name") {
                    "byte" => 8,
                    "shortint" => 16,
                    "int" | "integer" => 32,
                    "longint" | "time" => 64,
                    _ => 0,
                }),
                "PackedArrayType" => {
                    let element = ty.get("elementType").map(|e| self.width(e)).unwrap_or(Width::Bits(0));
                    match ty.get("range").and_then(Value::as_str).and_then(bounds) {
                        Some((left, right)) => Width::Times(left, right, Box::new(element)),
                        None => Width::Bits(0),
                    }
                }
                "PackedStructType" | "PackedUnionType" => {
                    let mut fields = Vec::new();
                    if let Some(Value::Array(members)) = ty.get("members") {
                        for field in members {
                            let Some(field_ty) = field.get("type") else { continue };
                            let offset = field.get("bitOffset").and_then(Value::as_u64).unwrap_or(0) as u32;
                            if let Some(addr) = field.get("addr").and_then(Value::as_u64) {
                                self.fields.insert(addr, offset);
                            }
                            let ty = self.width(field_ty);
                            fields.push(FieldTy { name: text(field, "name").to_string(), offset, ty });
                        }
                    }
                    if text(ty, "kind") == "PackedStructType" { Width::Sum(fields) } else { Width::Max(fields) }
                }
                "FixedSizeUnpackedArrayType" => {
                    let element = ty.get("elementType").map(|e| self.width(e)).unwrap_or(Width::Bits(0));
                    match ty.get("range").and_then(Value::as_str).and_then(bounds) {
                        Some((left, right)) => Width::Unpacked(left, right, Box::new(element)),
                        None => Width::Bits(0),
                    }
                }
                "UnpackedStructType" | "UnpackedUnionType" => {
                    let mut fields = Vec::new();
                    if let Some(Value::Array(members)) = ty.get("members") {
                        for field in members {
                            let Some(field_ty) = field.get("type") else { continue };
                            let ty = self.width(field_ty);
                            fields.push(FieldTy { name: text(field, "name").to_string(), offset: 0, ty });
                        }
                    }
                    Width::Record(fields)
                }
                "EnumType" => ty.get("baseType").map(|base| self.width(base)).unwrap_or(Width::Bits(0)),
                "TypeAlias" => ty.get("target").map(|target| self.width(target)).unwrap_or(Width::Bits(0)),
                // Unpacked types, floating point, strings, void: not bits that a copy keeps.
                _ => Width::Bits(0),
            },
            _ => Width::Bits(0),
        }
    }
}

fn is_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Array(list)) => list.is_empty(),
        Some(_) => false,
    }
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The address in a reference such as `"3218014243240 a"`.
fn link(v: &Value) -> Option<Addr> {
    v.as_str()?.split(' ').next()?.parse().ok()
}

/// Whether a constant such as `1'b0`, `32'd5` or `0` is true; None when it has unknown bits.
fn truth(constant: &str) -> Option<bool> {
    let digits = match constant.split_once('\'') {
        Some((_, based)) => based.trim_start_matches(['s', 'S']).get(1..)?,
        None => constant,
    };
    if digits.contains(['x', 'X', 'z', 'Z', '?']) {
        return None;
    }
    Some(digits.chars().any(|digit| digit != '0' && digit != '_'))
}

/// A block is combinational when it is `always_comb`, an `always @*`, or an `always` on a list of
/// signals without edges (`always @(a or b)`, the way of writing it before `@*`; a list that leaves
/// out a signal the block reads is still combinational logic to synthesis, so it isn't checked).
fn is_combinational(block: &Value) -> bool {
    match text(block, "procedureKind") {
        "AlwaysComb" => true,
        "Always" => {
            let body = &block["body"];
            text(body, "kind") == "Timed" && (text(&body["timing"], "kind") == "ImplicitEvent" || is_level_list(&body["timing"]))
        }
        _ => false,
    }
}

/// An event control that names signals and no edge: `@(a or b)`, `@(a)`. With an edge anywhere in
/// it (`@(posedge clk or rst)`) the whole list counts as clocks, as before.
fn is_level_list(timing: &Value) -> bool {
    match text(timing, "kind") {
        "SignalEvent" => text(timing, "edge") == "None" && timing.get("iff").is_none(),
        "EventList" => match timing.get("events") {
            Some(Value::Array(events)) => !events.is_empty() && events.iter().all(is_level_list),
            _ => false,
        },
        _ => false,
    }
}

/// Splits `"[3:0]"` into its bounds.
fn bounds(range: &str) -> Option<(i64, i64)> {
    let (left, right) = range.strip_prefix('[')?.strip_suffix(']')?.split_once(':')?;
    Some((left.trim().parse().ok()?, right.trim().parse().ok()?))
}

fn lower_bound(range: &str) -> Option<i64> {
    bounds(range).map(|(left, right)| left.min(right))
}

/// The value of a constant such as `3`, `-1` or `4'hF`.
fn constant(text: &str) -> Option<i64> {
    match text.split_once('\'') {
        None => text.trim().parse().ok(),
        Some((_, based)) => {
            let based = based.trim_start_matches(['s', 'S']);
            let radix = match based.chars().next()? {
                'b' | 'B' => 2,
                'o' | 'O' => 8,
                'd' | 'D' => 10,
                'h' | 'H' => 16,
                _ => return None,
            };
            i64::from_str_radix(&based[1..].replace('_', ""), radix).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_of_ranges() {
        assert_eq!(lower_bound("[3:0]"), Some(0));
        assert_eq!(lower_bound("[2:4]"), Some(2));
        assert_eq!(lower_bound("[-1:1]"), Some(-1));
    }

    #[test]
    fn constants_are_true_or_false() {
        assert_eq!(truth("1'b0"), Some(false));
        assert_eq!(truth("1'b1"), Some(true));
        assert_eq!(truth("0"), Some(false));
        assert_eq!(truth("32'sd5"), Some(true));
        assert_eq!(truth("4'b00x0"), None);
    }

    #[test]
    fn reference_addresses() {
        assert_eq!(link(&Value::from("3218014243240 a")), Some(3218014243240));
        assert_eq!(link(&Value::from("3218014244320 ")), Some(3218014244320));
        assert_eq!(link(&Value::from("logic")), None);
    }
}

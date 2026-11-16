//! Bits and fields: which bits of a signal a select or a field access touches, how an assignment or
//! a port connection pairs the bits it writes with the bits it reads, and the selected paths
//! (`top.u.cfg.mode`, `top.x[7:4]`) that name such bits.
//!
//! A [`Span`] counts bits of the signal's packed vector from the least significant one. `None`
//! (`Bits`) is the whole signal; it is also what a signal of unknown width (an unpacked array) has.

use std::collections::HashMap;

use crate::ast::{Addr, Node, NodeKind, Sel, Width};
use crate::graph::{Graph, SymId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
}

pub type Bits = Option<Span>;

impl Span {
    pub fn intersect(self, other: Span) -> Option<Span> {
        let (lo, hi) = (self.lo.max(other.lo), self.hi.min(other.hi));
        (lo <= hi).then_some(Span { lo, hi })
    }
}

/// The smallest span holding both; the whole signal if either is.
pub fn hull(a: Bits, b: Bits) -> Bits {
    match (a, b) {
        (Some(a), Some(b)) => Some(Span { lo: a.lo.min(b.lo), hi: a.hi.max(b.hi) }),
        _ => None,
    }
}

/// Whether two uses share a bit; the whole signal shares one with any.
pub fn overlaps(a: Bits, b: Bits) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.intersect(b).is_some(),
        _ => true,
    }
}

pub fn ptr(node: &Node) -> usize {
    node as *const Node as usize
}

/// A use of a signal narrowed by constant selects and field accesses.
pub struct Access<'a> {
    pub root: &'a Node,
    pub sym: SymId,
    /// Within the whole signal.
    pub span: Span,
    /// The signal's width.
    pub total: u32,
}

impl Access<'_> {
    pub fn bits(&self) -> Bits {
        (self.span.lo != 0 || self.span.hi + 1 != self.total).then_some(self.span)
    }
}

impl Graph {
    /// Follows a typedef or an enum to the type itself.
    fn resolve<'a>(&'a self, mut ty: &'a Width) -> &'a Width {
        for _ in 0..32 {
            match ty {
                Width::Link(addr) => match self.type_of(*addr) {
                    Some(target) => ty = target,
                    None => break,
                },
                _ => break,
            }
        }
        ty
    }

    /// The index range `[left:right]` and the element width of a type that can be selected from.
    fn dims(&self, ty: &Width) -> Option<(i64, i64, u32)> {
        let width = self.width(ty);
        if width == 0 {
            return None;
        }
        match self.resolve(ty) {
            Width::Times(left, right, element) => Some((*left, *right, self.width(element))),
            _ => Some((i64::from(width) - 1, 0, 1)),
        }
    }

    /// The signal and bits a chain of constant selects and field accesses on a signal stands for.
    pub fn access<'a>(&self, node: &'a Node) -> Option<Access<'a>> {
        match &node.kind {
            NodeKind::Value { addr, width } => {
                let total = self.width(width);
                let sym = self.symbol(*addr)?;
                (total > 0).then(|| Access { root: node, sym, span: Span { lo: 0, hi: total - 1 }, total })
            }
            NodeKind::Member { field } => {
                let base = self.access(node.children.first()?)?;
                let width = self.width(&node.width);
                let lo = base.span.lo + self.field_offset(*field)?;
                (width > 0 && lo + width - 1 <= base.span.hi).then(|| Access { span: Span { lo, hi: lo + width - 1 }, ..base })
            }
            NodeKind::Select(sel) => {
                let value = node.children.first()?;
                let base = self.access(value)?;
                let (left, right, element) = self.dims(&value.width)?;
                let (first, last) = match *sel {
                    Sel::Whole => return None,
                    Sel::Index(i) => (i, i),
                    Sel::Range(a, b) => (a.min(b), a.max(b)),
                    Sel::Up(base, width) => (base, base + width - 1),
                    Sel::Down(base, width) => (base - width + 1, base),
                };
                let (low, high) = (left.min(right), left.max(right));
                if first < low || last > high || first > last {
                    return None;
                }
                let offsets = [(first - right).unsigned_abs() as u32, (last - right).unsigned_abs() as u32];
                let (near, far) = (offsets[0].min(offsets[1]), offsets[0].max(offsets[1]));
                let lo = base.span.lo + near * element;
                let hi = base.span.lo + (far + 1) * element - 1;
                (hi <= base.span.hi).then_some(Access { span: Span { lo, hi }, ..base })
            }
            _ => None,
        }
    }

    /// The bits as a signal's own `Bits`: the whole signal when they cover it.
    pub fn normalize(&self, sym: SymId, span: Span) -> Bits {
        let total = self.width(&self.symbols[sym].ty);
        (span.lo != 0 || span.hi + 1 != total).then_some(span)
    }

    /// Records on each use of a signal which of its bits the selects around it pick.
    pub(crate) fn annotate(&self, node: &mut Node) {
        if matches!(node.kind, NodeKind::Select(_) | NodeKind::Member { .. }) {
            if let Some(bits) = self.access(node).map(|access| access.bits()) {
                self.annotate_chain(node, bits);
                return;
            }
        }
        for child in &mut node.children {
            self.annotate(child);
        }
    }

    fn annotate_chain(&self, node: &mut Node, bits: Bits) {
        if let NodeKind::Value { .. } = node.kind {
            node.span = bits;
            return;
        }
        if let Some((first, rest)) = node.children.split_first_mut() {
            for index in rest {
                self.annotate(index);
            }
            self.annotate_chain(first, bits);
        }
    }

    // ---- selected paths ----

    /// A path that is a signal followed by fields and constant selects: the signal and its bits.
    pub fn lookup_selected(&self, path: &str) -> Option<(SymId, Bits)> {
        if let Some(crate::graph::Target::Value(id)) = self.lookup(path) {
            return Some((id, None));
        }
        for (cut, c) in path.char_indices().rev() {
            if c != '.' && c != '[' {
                continue;
            }
            if let Some(crate::graph::Target::Value(id)) = self.lookup(&path[..cut]) {
                return self.select_bits(id, &path[cut..]);
            }
        }
        None
    }

    fn select_bits(&self, id: SymId, suffix: &str) -> Option<(SymId, Bits)> {
        let ty = self.symbols[id].ty.clone();
        if self.width(&ty) == 0 {
            // Not packed (an unpacked array or struct): a part of it is the whole signal here, when
            // the signal has such a part.
            return self.has_part(ty, suffix).then_some((id, None));
        }
        let (base, len) = self.packed_part(ty, suffix)?;
        Some((id, self.normalize(id, Span { lo: base, hi: base + len - 1 })))
    }

    /// Whether fields and selects name a part of an unpacked type: an element in the array's range,
    /// a field the struct has, and from a packed element on, bits it has. A type the dump isn't read
    /// for (a dynamic array, a queue, a class) can't be checked, and passes.
    fn has_part(&self, ty: Width, mut suffix: &str) -> bool {
        let mut current = ty;
        while !suffix.is_empty() {
            if self.width(&current) > 0 {
                return self.packed_part(current, suffix).is_some();
            }
            match self.resolve(&current).clone() {
                Width::Unpacked(left, right, element) => {
                    let Some((first, last, rest)) = select(suffix) else { return false };
                    if first < left.min(right) || last > left.max(right) {
                        return false;
                    }
                    if first != last {
                        // (A slice of the array: what is selected from it is numbered anew, and not followed.)
                        return true;
                    }
                    (current, suffix) = (*element, rest);
                }
                Width::Record(fields) => {
                    let Some(rest) = suffix.strip_prefix('.') else { return false };
                    let end = rest.find(['.', '[']).unwrap_or(rest.len());
                    let Some(field) = fields.iter().find(|field| field.name == rest[..end]) else { return false };
                    (current, suffix) = (field.ty.clone(), &rest[end..]);
                }
                _ => return true,
            }
        }
        true
    }

    /// The bits of a packed type that fields and selects name: where they start and how many.
    fn packed_part(&self, ty: Width, mut suffix: &str) -> Option<(u32, u32)> {
        let mut current = ty;
        let mut base = 0u32;
        let mut len = self.width(&current);
        while !suffix.is_empty() {
            if let Some(rest) = suffix.strip_prefix('.') {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let name = &rest[..end];
                let (Width::Sum(fields) | Width::Max(fields)) = self.resolve(&current) else {
                    return None;
                };
                let field = fields.iter().find(|field| field.name == name)?;
                base += field.offset;
                current = field.ty.clone();
                len = self.width(&current);
                suffix = &rest[end..];
            } else {
                let (first, last, rest) = select(suffix)?;
                let (left, right, element) = self.dims(&current)?;
                if first < left.min(right) || last > left.max(right) {
                    return None;
                }
                let (o1, o2) = ((first - right).unsigned_abs() as u32, (last - right).unsigned_abs() as u32);
                let (near, far) = (o1.min(o2), o1.max(o2));
                base += near * element;
                len = (far - near + 1) * element;
                let element_ty = match self.resolve(&current) {
                    Width::Times(_, _, element) => (**element).clone(),
                    _ => Width::Bits(1),
                };
                current = if near == far {
                    element_ty
                } else {
                    // (A part of an array is numbered from its own end.)
                    let count = i64::from(far - near);
                    let (new_left, new_right) = if left >= right { (count, 0) } else { (0, count) };
                    Width::Times(new_left, new_right, Box::new(element_ty))
                };
                suffix = rest;
            }
        }
        (len > 0).then_some((base, len))
    }

    /// The selected path of some bits of a signal: a field's name when they are exactly a field,
    /// else the select `[hi:lo]` in the declared numbering, else the signal's path.
    pub fn selected_path(&self, id: SymId, bits: Bits) -> String {
        let mut path = self.symbols[id].path.clone();
        if let Some(span) = bits {
            self.suffix(&self.symbols[id].ty, span, &mut path);
        }
        path
    }

    fn suffix(&self, ty: &Width, span: Span, out: &mut String) {
        let total = self.width(ty);
        if total == 0 || (span.lo == 0 && span.hi + 1 >= total) {
            return;
        }
        if let Width::Sum(fields) | Width::Max(fields) = self.resolve(ty) {
            for field in fields {
                let width = self.width(&field.ty);
                if span.lo >= field.offset && span.hi < field.offset + width {
                    out.push('.');
                    out.push_str(&field.name);
                    let inner = Span { lo: span.lo - field.offset, hi: span.hi - field.offset };
                    return self.suffix(&field.ty, inner, out);
                }
            }
        }
        let Some((left, right, element)) = self.dims(ty) else { return };
        let (near, far) = (span.lo / element, span.hi / element);
        let index = |offset: u32| if left >= right { right + i64::from(offset) } else { right - i64::from(offset) };
        out.push_str(&if near == far { format!("[{}]", index(near)) } else { format!("[{}:{}]", index(far), index(near)) });
        if near == far {
            let element_ty = match self.resolve(ty) {
                Width::Times(_, _, element) => (**element).clone(),
                _ => Width::Bits(1),
            };
            self.suffix(&element_ty, Span { lo: span.lo - near * element, hi: span.hi - near * element }, out);
        }
    }
}

/// A select at the start of a path's rest, `[3]` or `[7:4]`: its lowest and highest index, and what follows it.
fn select(suffix: &str) -> Option<(i64, i64, &str)> {
    let rest = suffix.strip_prefix('[')?;
    let end = rest.find(']')?;
    let inner = rest[..end].trim();
    let (a, b) = match inner.split_once(':') {
        Some((a, b)) => (a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?),
        None => {
            let i = inner.parse::<i64>().ok()?;
            (i, i)
        }
    };
    Some((a.min(b), a.max(b), &rest[end + 1..]))
}

// ---- pairing the bits of the two sides of a connection ----

/// A stretch of one side of an assignment or a port connection.
pub enum Src<'a> {
    /// A stretch of one signal, bit for bit: its least significant bit is bit `lo` of the signal.
    /// `node` is the use in the source (None for a port's own signal).
    Bits { sym: SymId, lo: u32, node: Option<&'a Node> },
    /// The result of an expression: every bit depends on every bit of each of these uses.
    Any { values: Vec<&'a Node>, call: bool },
    /// A literal.
    Const,
}

pub struct Seg<'a> {
    pub width: u32,
    pub src: Src<'a>,
}

pub struct Placed<'a> {
    /// The position of the stretch's least significant bit in the value that is assigned.
    pub lo: u32,
    pub seg: Seg<'a>,
}

pub struct Link<'a> {
    pub lhs: Vec<Placed<'a>>,
    pub rhs: Vec<Placed<'a>>,
}

fn place(segs: Vec<Seg<'_>>) -> Vec<Placed<'_>> {
    let mut at: u32 = segs.iter().map(|seg| seg.width).sum();
    segs.into_iter()
        .map(|seg| {
            at -= seg.width;
            Placed { lo: at, seg }
        })
        .collect()
}

/// Pairs the two sides by position; None when their widths aren't known or don't agree, which the
/// callers take as every bit linked to every bit.
pub fn link<'a>(lhs: Vec<Seg<'a>>, rhs: Vec<Seg<'a>>) -> Option<Link<'a>> {
    let total = |segs: &[Seg]| segs.iter().map(|seg| seg.width).sum::<u32>();
    let known = |segs: &[Seg]| !segs.is_empty() && segs.iter().all(|seg| seg.width > 0);
    if !known(&lhs) || !known(&rhs) || total(&lhs) != total(&rhs) {
        return None;
    }
    Some(Link { lhs: place(lhs), rhs: place(rhs) })
}

pub fn collect_values<'a>(node: &'a Node, lhs: bool, out: &mut Vec<&'a Node>) {
    match node.kind {
        NodeKind::Value { .. } => out.push(node),
        // An index on the left only picks which bits are written.
        NodeKind::Select(_) if lhs => {
            if let Some(value) = node.children.first() {
                collect_values(value, lhs, out);
            }
        }
        _ => {
            for child in &node.children {
                collect_values(child, lhs, out);
            }
        }
    }
}

fn has_call(node: &Node) -> bool {
    node.kind == NodeKind::Call || node.children.iter().any(has_call)
}

impl Graph {
    fn any_seg<'a>(&self, node: &'a Node, lhs: bool) -> Seg<'a> {
        let mut values = Vec::new();
        collect_values(node, lhs, &mut values);
        Seg { width: self.width(&node.width), src: Src::Any { values, call: has_call(node) } }
    }

    /// An expression as stretches, most significant first.
    pub fn segs<'a>(&self, node: &'a Node, lhs: bool) -> Vec<Seg<'a>> {
        let width = self.width(&node.width);
        match &node.kind {
            NodeKind::Value { .. } | NodeKind::Select(_) | NodeKind::Member { .. } => match self.access(node) {
                Some(access) => {
                    // (A use that a select narrows is recorded on the signal's own node.)
                    vec![Seg {
                        width: access.span.hi - access.span.lo + 1,
                        src: Src::Bits { sym: access.sym, lo: access.span.lo, node: Some(access.root) },
                    }]
                }
                None => vec![self.any_seg(node, lhs)],
            },
            NodeKind::Concat => node.children.iter().flat_map(|part| self.segs(part, lhs)).collect(),
            NodeKind::Replicate => {
                let Some(inner) = node.children.last() else { return vec![self.any_seg(node, lhs)] };
                let parts = self.segs(inner, lhs);
                let inner_width: u32 = parts.iter().map(|part| part.width).sum();
                if inner_width == 0 || !width.is_multiple_of(inner_width) || parts.iter().any(|part| part.width == 0) {
                    return vec![self.any_seg(node, lhs)];
                }
                let mut all = Vec::new();
                for _ in 0..width / inner_width {
                    all.extend(self.segs(inner, lhs));
                }
                all
            }
            NodeKind::Convert { .. } => {
                let Some(operand) = node.children.first() else { return vec![self.any_seg(node, lhs)] };
                let parts = self.segs(operand, lhs);
                let inner: u32 = parts.iter().map(|part| part.width).sum();
                if width == 0 || inner == 0 || parts.iter().any(|part| part.width == 0) {
                    return vec![self.any_seg(node, lhs)];
                }
                if width == inner {
                    parts
                } else if width < inner {
                    low_bits(parts, width)
                } else {
                    // An extension: the new upper bits are zeros or copies of the top bit, so they
                    // are taken to depend on everything the operand does.
                    let extension = self.any_seg(operand, lhs);
                    let mut all = vec![Seg { width: width - inner, ..extension }];
                    all.extend(parts);
                    all
                }
            }
            NodeKind::Pattern { members, default } => self.pattern_segs(node, members.as_deref(), *default, lhs),
            NodeKind::Literal => vec![Seg { width, src: Src::Const }],
            _ => vec![self.any_seg(node, lhs)],
        }
    }

    /// An assignment pattern as stretches: the field values side by side, the first field on top.
    fn pattern_segs<'a>(&self, node: &'a Node, members: Option<&[Addr]>, default: bool, lhs: bool) -> Vec<Seg<'a>> {
        let Width::Sum(fields) = self.resolve(&node.width) else { return vec![self.any_seg(node, lhs)] };
        let setters = node.children.len() - usize::from(default);
        let default_expr = default.then(|| &node.children[setters]);
        let mut order: Vec<_> = fields.iter().collect();
        order.sort_by_key(|field| std::cmp::Reverse(field.offset));
        let mut all = Vec::new();
        for (at, field) in order.iter().enumerate() {
            let width = self.width(&field.ty);
            let expr = match members {
                // (Positionally, the elements are the fields in declaration order.)
                None => node.children.get(at).filter(|_| setters == fields.len()),
                Some(members) => members
                    .iter()
                    .position(|&member| self.field_offset(member) == Some(field.offset))
                    .and_then(|position| node.children.get(position)),
            };
            let expr = expr.or(default_expr);
            let Some(expr) = expr else {
                all.push(Seg { width, src: Src::Const });
                continue;
            };
            let parts = self.segs(expr, lhs);
            if parts.iter().map(|part| part.width).sum::<u32>() == width && parts.iter().all(|part| part.width > 0) {
                all.extend(parts);
            } else {
                let mut values = Vec::new();
                collect_values(expr, lhs, &mut values);
                all.push(Seg { width, src: Src::Any { values, call: has_call(expr) } });
            }
        }
        all
    }

    /// A signal's own bits as a stretch (a port's internal signal).
    pub fn whole_seg<'a>(&self, sym: SymId) -> Seg<'a> {
        Seg { width: self.width(&self.symbols[sym].ty), src: Src::Bits { sym, lo: 0, node: None } }
    }
}

fn low_bits(parts: Vec<Seg<'_>>, width: u32) -> Vec<Seg<'_>> {
    let mut kept = Vec::new();
    let mut left = width;
    for part in parts.into_iter().rev() {
        if left == 0 {
            break;
        }
        let take = part.width.min(left);
        left -= take;
        kept.push(Seg { width: take, src: part.src });
    }
    kept.reverse();
    kept
}

/// Positions in the assigned value, as inclusive intervals.
pub type Positions = Vec<(u32, u32)>;

/// What the stretches at some positions are made of.
#[derive(Default)]
pub struct Fed {
    /// The uses of signals, by node, with the bits each supplies.
    pub nodes: HashMap<usize, Bits>,
    /// Port signals (no node) with their bits.
    pub synthetic: Vec<(SymId, Bits)>,
    /// Some stretch there is made of a signal that is not a constant.
    pub signal: bool,
    /// Some stretch there is the result of a call.
    pub call: bool,
}

impl Fed {
    fn add(&mut self, key: usize, bits: Bits) {
        self.nodes.entry(key).and_modify(|old| *old = hull(*old, bits)).or_insert(bits);
    }

    /// Every use in these nodes, whole, as when the two sides can't be paired.
    pub fn everything(graph: &Graph, values: &[&Node], call: bool) -> Fed {
        let mut fed = Fed { call, ..Fed::default() };
        for value in values {
            fed.add(ptr(value), value.span);
            fed.signal |= !is_constant_use(graph, value);
        }
        fed
    }
}

fn is_constant_use(graph: &Graph, value: &Node) -> bool {
    match value.kind {
        NodeKind::Value { addr, .. } => graph.symbol(addr).is_some_and(|id| graph.is_constant(id)),
        _ => false,
    }
}

/// The positions at which a signal's bits `asked` are on this side.
pub fn wanted(placed: &[Placed], root: SymId, asked: Span, graph: &Graph) -> Positions {
    let mut positions = Vec::new();
    for p in placed {
        let end = p.lo + p.seg.width - 1;
        match &p.seg.src {
            Src::Bits { sym, lo, .. } if *sym == root => {
                let stretch = Span { lo: *lo, hi: lo + p.seg.width - 1 };
                if let Some(common) = stretch.intersect(asked) {
                    positions.push((p.lo + common.lo - lo, p.lo + common.hi - lo));
                }
            }
            Src::Any { values, .. } => {
                let touched = values.iter().any(|value| {
                    matches!(value.kind, NodeKind::Value { addr, .. } if graph.symbol(addr) == Some(root))
                        && overlaps(value.span, Some(asked))
                });
                if touched {
                    positions.push((p.lo, end));
                }
            }
            _ => {}
        }
    }
    positions
}

/// The positions at which one use of a signal is on this side.
pub fn wanted_at(placed: &[Placed], node: usize, root: SymId, asked: Span) -> Positions {
    let mut positions = Vec::new();
    for p in placed {
        match &p.seg.src {
            Src::Bits { sym, lo, node: Some(n) } if *sym == root && ptr(n) == node => {
                let stretch = Span { lo: *lo, hi: lo + p.seg.width - 1 };
                if let Some(common) = stretch.intersect(asked) {
                    positions.push((p.lo + common.lo - lo, p.lo + common.hi - lo));
                }
            }
            Src::Any { values, .. } if values.iter().any(|value| ptr(value) == node) => {
                positions.push((p.lo, p.lo + p.seg.width - 1));
            }
            _ => {}
        }
    }
    positions
}

/// What the stretches at `positions` are made of.
pub fn fed(placed: &[Placed], positions: &Positions, graph: &Graph) -> Fed {
    let mut fed = Fed::default();
    for p in placed {
        let (start, end) = (p.lo, p.lo + p.seg.width - 1);
        let hits: Vec<(u32, u32)> = positions
            .iter()
            .filter(|(a, b)| *a <= end && *b >= start)
            .map(|(a, b)| ((*a).max(start), (*b).min(end)))
            .collect();
        if hits.is_empty() {
            continue;
        }
        match &p.seg.src {
            Src::Bits { sym, lo, node } => {
                let first = hits.iter().map(|h| h.0).min().unwrap_or(start);
                let last = hits.iter().map(|h| h.1).max().unwrap_or(end);
                let bits = graph.normalize(*sym, Span { lo: lo + first - start, hi: lo + last - start });
                match node {
                    Some(node) => fed.add(ptr(node), bits),
                    None => fed.synthetic.push((*sym, bits)),
                }
                fed.signal |= !graph.is_constant(*sym);
            }
            Src::Any { values, call } => {
                for value in values {
                    fed.add(ptr(value), value.span);
                    fed.signal |= !is_constant_use(graph, value);
                }
                fed.call |= *call;
            }
            Src::Const => {}
        }
    }
    fed
}

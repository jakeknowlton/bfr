//! The optimizer's intermediate representation.
//!
//! A [`Block`] is a canonical sequence of run / loop / scan nodes. Within a
//! [`Run`], effect offsets are relative to the pointer *at run entry* and the
//! pointer moves exactly once, at the end.

pub mod build;
pub mod lower;
pub mod print;

use crate::config::Dialect;
use crate::error::Span;

/// A cell value, masked to the configured width by whatever produces it.
pub type Cell = u32;

/// A signed change to a cell value
pub type CellDelta = i32;

/// Stable identity for a node, for side tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Carried by synthesized nodes until [`Program::renumber`] runs.
    pub const UNASSIGNED: NodeId = NodeId(u32::MAX);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub body: Block,
}

impl Program {
    pub fn new(body: Block) -> Self {
        Program { body }
    }

    /// Assign sequential ids in traversal order, replacing [`NodeId::UNASSIGNED`].
    pub fn renumber(&mut self) {
        todo!("Program::renumber")
    }

    /// Total effects plus control nodes.
    pub fn op_count(&self) -> usize {
        todo!("Program::op_count")
    }

    /// Check every IR invariant, returning the first violation.
    pub fn validate(&self, dialect: &Dialect) -> Result<(), String> {
        todo!("Program::validate")
    }
}

/// A canonical node sequence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    nodes: Vec<Node>,
}

impl Block {
    pub fn new() -> Block {
        Block { nodes: Vec::new() }
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Mutable access to node payloads.
    /// Cannot change the node sequence, so canonical adjacency is preserved.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.nodes.iter_mut()
    }

    /// Replace the nodes in `range` with `replacement`.
    pub fn splice(&mut self, range: core::ops::Range<usize>, replacement: Vec<Node>) {
        todo!("Block::splice")
    }

    /// Offer every loop in this block (not nested ones) to `f`; where it
    /// returns a replacement, splice it in.
    pub fn try_replace_loops(&mut self, f: impl FnMut(&Loop, Span) -> Option<Vec<Node>>) -> bool {
        todo!("Block::try_replace_loops")
    }

    /// Restore canonical and locally reduced form: merge adjacent runs,
    /// compose same-cell effect pairs, drop `+= 0` adds and empty runs.
    /// Returns whether anything changed.
    pub fn normalize(&mut self, dialect: &Dialect) -> bool {
        todo!("Block::normalize")
    }

    /// Visit every block in this subtree, innermost first, `self` last.
    pub fn for_each_block_mut(&mut self, f: &mut impl FnMut(&mut Block)) {
        todo!("Block::for_each_block_mut")
    }

    /// Net pointer movement, or `None` when not statically known.
    pub fn net_shift(&self) -> Option<isize> {
        todo!("Block::net_shift")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    /// Merged span of every source character that contributed.
    pub span: Span,
    pub kind: NodeKind,
}

impl Node {
    /// Construct a synthesized node with [`NodeId::UNASSIGNED`].
    pub fn new(kind: NodeKind, span: Span) -> Self {
        Node {
            id: NodeId::UNASSIGNED,
            span,
            kind,
        }
    }

    /// Net pointer movement, or `None` if not statically known.
    pub fn net_shift(&self) -> Option<isize> {
        todo!("Node::net_shift")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Run(Run),
    Loop(Loop),
    Scan(Scan),
}

impl NodeKind {
    /// The child block, if this kind has one.
    pub fn body(&self) -> Option<&Block> {
        match self {
            NodeKind::Loop(l) => Some(&l.body),
            NodeKind::Run(_) | NodeKind::Scan(_) => None,
        }
    }

    pub fn body_mut(&mut self) -> Option<&mut Block> {
        match self {
            NodeKind::Loop(l) => Some(&mut l.body),
            NodeKind::Run(_) | NodeKind::Scan(_) => None,
        }
    }
}

/// A straight-line run. Effects addressed relative to the pointer at run
/// entry, then one net shift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub effects: Vec<Eff>,
    pub shift: isize,
}

impl Run {
    /// A run with no effects and only a shift.
    pub fn from_shift(shift: isize) -> Run {
        Run {
            effects: Vec::new(),
            shift,
        }
    }

    /// Sequence two runs into one.
    pub fn concat(self, next: Run) -> Run {
        todo!("Run::concat")
    }

    /// No reads, no writes.
    pub fn is_pure(&self) -> bool {
        self.effects.iter().all(|e| !e.kind.is_observable())
    }
}

/// `while tape[ptr] != 0 { body }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loop {
    pub body: Block,
}

impl Loop {
    /// Classify this loop's body. Computed on demand.
    pub fn shape(&self) -> LoopShape {
        todo!("Loop::shape")
    }
}

/// What a loop's body means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopShape {
    /// The control cell steps by `step` toward zero; every other touched
    /// cell accumulates a constant per iteration.
    Drain {
        step: CellDelta,
        /// `(offset, per-iteration delta)` for the cells receiving the
        /// drained value. Empty for `[-]`; one entry for `[->+<]`; two or
        /// more for copy/fan-out loops like `[->+>+<<]`.
        targets: Vec<(isize, CellDelta)>,
    },
    /// Single-run body with no effects and nonzero shift: `[>]`, `[<<]`.
    Scan { stride: isize },
    /// Anything else.
    Other,
}

/// Advance the pointer by `stride` until `tape[ptr] == 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Should never be 0, by convention.
    pub stride: isize,
}

/// One step of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eff {
    /// Merged span of every source character that contributed.
    pub span: Span,
    pub kind: EffKind,
}

impl Eff {
    pub fn new(kind: EffKind, span: Span) -> Eff {
        Eff { span, kind }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffKind {
    /// `tape[ptr + at] += delta`. `delta` is the true integer sum of fused deltas.
    Add { at: isize, delta: CellDelta },

    /// `tape[ptr + at] = value`, masked.
    Set { at: isize, value: Cell },

    /// `tape[ptr + at] += tape[ptr + from] * factor`.
    AddScaled {
        at: isize,
        from: isize,
        factor: CellDelta,
    },

    /// `tape[ptr + at] =` the next input byte, subject to
    /// [`crate::config::EofBehavior`].
    Read { at: isize },

    /// Emit the low 8 bits of `tape[ptr + at]` as output.
    Write { at: isize },
}

impl EffKind {
    /// The offset this effect writes, if any. `Read` writes its cell.
    pub fn writes(&self) -> Option<isize> {
        match self {
            EffKind::Add { at, .. }
            | EffKind::Set { at, .. }
            | EffKind::AddScaled { at, .. }
            | EffKind::Read { at } => Some(*at),
            EffKind::Write { .. } => None,
        }
    }

    /// Every offset this effect reads. `Read` is conservatively a reader.
    pub fn reads(&self, mut sink: impl FnMut(isize)) {
        match self {
            EffKind::Add { at, .. } => sink(*at),
            EffKind::Set { .. } => {}
            EffKind::AddScaled { at, from, .. } => {
                sink(*at);
                sink(*from);
            }
            EffKind::Read { at } => sink(*at),
            EffKind::Write { at } => sink(*at),
        }
    }

    /// Observable outside the tape (I/O).
    pub fn is_observable(&self) -> bool {
        matches!(self, EffKind::Read { .. } | EffKind::Write { .. })
    }

    /// Compose two same-destination effects with no intervening effects for
    /// that cell. Declines (`None`) when the destinations differ or the
    /// combination has no single-effect equivalent in the IR.
    pub fn compose(&self, later: &EffKind, dialect: &Dialect) -> Option<EffKind> {
        todo!("EffKind::compose")
    }
}

//! The optimizer's intermediate representation.
//!
//! A [`Block`] is a canonical sequence of run / loop / scan nodes. Within a
//! [`Run`], effect offsets are relative to the pointer *at run entry* and the
//! pointer moves exactly once, at the end.
//!
//! Vocabulary used throughout the crate:
//!
//! - A run's *shift* is the single pointer move at its end.
//! - A loop's *control cell* is the cell under the pointer at each test.
//! - A *trip* is one pass through a loop body.
//! - A *shell* is a loop whose body zeroes the control cell, so it runs at
//!   most once and acts like an `if`.
//! - A *clear* is a store of zero, `[0] = 0`.
//! - A block is *canonical* when no two runs are adjacent and no run does
//!   nothing.
//! - A function *declines* when it returns `None` because its input does
//!   not fit the pattern it handles.

pub(crate) mod arith;
pub mod build;
pub mod lower;
pub mod print;
#[cfg(test)]
pub mod test_support;

use std::collections::{HashMap, HashSet};

use crate::config::Dialect;
use crate::error::Span;

/// A cell value, masked to the configured width by whatever produces it.
pub type Cell = u32;

/// A signed change to a cell value.
pub type CellDelta = i32;

/// Identifies a node so side tables can refer to it. Stable until the tree
/// changes, see [`Program::renumber`].
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

    /// Visit every block, innermost first.
    pub fn for_each_block_mut(&mut self, f: &mut impl FnMut(&mut Block, BlockSite)) {
        self.body.walk_mut(BlockSite::Program, f);
    }

    /// Assign every node a sequential id in pre-order traversal order.
    /// This is deterministic and idempotent, so side tables keyed by [`NodeId`]
    /// stay valid as long as the tree is unchanged.
    pub fn renumber(&mut self) {
        let mut next = 0u32;
        let mut stack: Vec<&mut [Node]> = vec![&mut self.body.nodes];
        while let Some(nodes) = stack.pop() {
            let Some((node, rest)) = nodes.split_first_mut() else {
                continue;
            };
            node.id = NodeId(next);
            next += 1;
            stack.push(rest);
            if let Some(body) = node.kind.body_mut() {
                stack.push(&mut body.nodes);
            }
        }
    }

    /// Every effect in the program, plus one for each loop and scan.
    pub fn op_count(&self) -> usize {
        self.body.op_count()
    }

    /// Check every IR invariant, returning the first violation.
    ///
    /// The invariants are that every block is canonical, every scan has a
    /// nonzero stride, and every store holds a value that fits the cell
    /// width. The message names the offending node by its index path, so
    /// `node 2.0` is the first node inside the third top-level node.
    pub fn validate(&self, dialect: &Dialect) -> Result<(), String> {
        let mut path = Vec::new();
        self.body.validate(dialect, &mut path)
    }
}

/// A canonical node sequence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    nodes: Vec<Node>,
}

/// Where a block sits in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSite {
    /// Entered once with a zeroed tape.
    Program,
    /// Entered after the loop's test read a nonzero cell.
    LoopBody,
}

impl Block {
    pub fn new() -> Block {
        Block { nodes: Vec::new() }
    }

    pub fn from_nodes(nodes: Vec<Node>) -> Block {
        let mut block = Block { nodes };
        block.canonicalize();
        block
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

    /// Replace each node by the nodes `f` returns for it, then restore
    /// canonical form.
    pub fn map_nodes(&mut self, f: impl FnMut(Node) -> Vec<Node>) {
        let nodes = core::mem::take(&mut self.nodes);
        self.nodes = nodes.into_iter().flat_map(f).collect();
        self.canonicalize();
    }

    /// Replace the nodes in `range` with `replacement`.
    pub fn splice(&mut self, range: core::ops::Range<usize>, replacement: Vec<Node>) {
        self.nodes.splice(range, replacement);
        self.canonicalize();
    }

    /// Call `f` on each loop directly in this block, not in nested blocks.
    /// Where it returns a replacement, splice that in.
    pub fn try_replace_loops(
        &mut self,
        mut f: impl FnMut(&Loop, Span) -> Option<Vec<Node>>,
    ) -> bool {
        let mut changed = false;
        let mut out: Vec<Node> = Vec::with_capacity(self.nodes.len());
        for node in core::mem::take(&mut self.nodes) {
            let replacement = match &node.kind {
                NodeKind::Loop(l) => f(l, node.span),
                NodeKind::Run(_) | NodeKind::Scan(_) => None,
            };
            match replacement {
                Some(nodes) => {
                    changed = true;
                    out.extend(nodes);
                }
                None => out.push(node),
            }
        }
        self.nodes = out;
        if changed {
            self.canonicalize();
        }
        changed
    }

    /// Restore canonical form by merging adjacent runs and dropping
    /// runs that do nothing. Returns whether anything changed.
    fn canonicalize(&mut self) -> bool {
        let mut changed = false;

        let mut merged: Vec<Node> = Vec::with_capacity(self.nodes.len());
        for node in core::mem::take(&mut self.nodes) {
            let Node { id, span, kind } = node;
            let next = match kind {
                NodeKind::Run(run) => run,
                kind => {
                    merged.push(Node { id, span, kind });
                    continue;
                }
            };
            if let Some(Node {
                id: prev_id,
                span: prev_span,
                kind: NodeKind::Run(prev),
            }) = merged.last_mut()
            {
                prev.absorb(next);
                *prev_span = prev_span.merge(span);
                *prev_id = NodeId::UNASSIGNED;
                changed = true;
                continue;
            }
            merged.push(Node {
                id,
                span,
                kind: NodeKind::Run(next),
            });
        }
        self.nodes = merged;

        let before = self.nodes.len();
        self.nodes.retain(|node| match &node.kind {
            NodeKind::Run(run) => !run.is_nop(),
            NodeKind::Loop(_) | NodeKind::Scan(_) => true,
        });
        changed |= self.nodes.len() != before;

        changed
    }

    /// Restore canonical form and fold within each run: merge adjacent
    /// runs, compose effects on the same cell, and drop effects and runs
    /// that do nothing. Returns whether anything changed.
    pub fn normalize(&mut self, dialect: &Dialect) -> bool {
        let mut changed = self.canonicalize();

        for node in &mut self.nodes {
            if let NodeKind::Run(run) = &mut node.kind {
                changed |= normalize_effects(&mut run.effects, dialect);
            }
        }

        // Folding may have emptied a run, so canonicalize again
        changed | self.canonicalize()
    }

    /// Every effect in this block and its nested blocks, plus one for each
    /// loop and scan.
    pub fn op_count(&self) -> usize {
        let mut count = 0;
        let mut stack = vec![self.nodes()];
        while let Some(nodes) = stack.pop() {
            for node in nodes {
                match &node.kind {
                    NodeKind::Run(run) => count += run.effects.len(),
                    NodeKind::Loop(l) => {
                        count += 1;
                        stack.push(l.body.nodes());
                    }
                    NodeKind::Scan(_) => count += 1,
                }
            }
        }
        count
    }

    /// Check the invariants of this block and every block nested in it.
    /// `path` is the index path to this block, used in the message.
    fn validate(&self, dialect: &Dialect, path: &mut Vec<usize>) -> Result<(), String> {
        fn describe(path: &[usize]) -> String {
            let indexes: Vec<String> = path.iter().map(usize::to_string).collect();
            format!("node {}", indexes.join("."))
        }

        let mut after_run = false;
        for (i, node) in self.nodes.iter().enumerate() {
            path.push(i);
            match &node.kind {
                NodeKind::Run(run) => {
                    if after_run {
                        return Err(format!("{} is a run next to a run", describe(path)));
                    }
                    if run.is_nop() {
                        return Err(format!("{} is a run that does nothing", describe(path)));
                    }
                    for eff in &run.effects {
                        if let EffKind::Store { value, .. } = eff.kind
                            && value & dialect.cell_width.mask() != value
                        {
                            return Err(format!(
                                "{} stores {value}, which does not fit the cell width",
                                describe(path)
                            ));
                        }
                    }
                    after_run = true;
                }
                NodeKind::Loop(l) => {
                    l.body.validate(dialect, path)?;
                    after_run = false;
                }
                NodeKind::Scan(scan) => {
                    if scan.stride == 0 {
                        return Err(format!("{} is a scan with a stride of 0", describe(path)));
                    }
                    after_run = false;
                }
            }
            path.pop();
        }
        Ok(())
    }

    /// Visit every block in this subtree, innermost first, `self` last.
    fn walk_mut(&mut self, site: BlockSite, f: &mut impl FnMut(&mut Block, BlockSite)) {
        for node in &mut self.nodes {
            if let Some(body) = node.kind.body_mut() {
                body.walk_mut(BlockSite::LoopBody, f);
            }
        }
        f(self, site);
    }

    /// Net pointer movement, or `None` when not known at compile time.
    pub fn net_shift(&self) -> Option<isize> {
        self.nodes
            .iter()
            .try_fold(0, |sum, node| Some(sum + node.net_shift()?))
    }

    /// Add every cell this block touches, starting from `cursor`, to
    /// `footprint`. Returns the cursor at exit, or `None` when the exit
    /// cursor is not known at compile time.
    fn footprint_from(&self, mut cursor: isize, footprint: &mut Footprint) -> Option<isize> {
        for node in &self.nodes {
            match &node.kind {
                NodeKind::Run(run) => {
                    for eff in &run.effects {
                        eff.kind.reads(|at| {
                            footprint.reads.insert(cursor + at);
                        });
                        if let Some(at) = eff.kind.writes() {
                            footprint.writes.insert(cursor + at);
                        }
                    }
                    cursor += run.shift;
                }
                NodeKind::Loop(l) => {
                    footprint.reads.insert(cursor);
                    if l.body.footprint_from(cursor, footprint)? != cursor {
                        return None;
                    }
                }
                NodeKind::Scan(_) => return None,
            }
        }
        Some(cursor)
    }
}

/// Fold each effect into the most recent write to the same cell, where
/// nothing in between observes that cell, and drop effects that do nothing.
/// Returns whether anything changed.
fn normalize_effects(effects: &mut Vec<Eff>, dialect: &Dialect) -> bool {
    struct LastWrite {
        /// Index into `out`.
        at: usize,
        /// Whether anything has read the cell since.
        observed: bool,
    }

    /// The index in `out` of the write that `kind` can fold into, if any.
    fn fold_target(kind: &EffKind, state: &HashMap<isize, LastWrite>) -> Option<usize> {
        let prev = state.get(&kind.writes()?)?;
        if prev.observed {
            return None;
        }
        // Folding moves this effect, including its read of `from`, back to
        // `prev.at`. Decline if a write to `from` sits in between, since
        // the moved read would no longer see that write.
        if let EffKind::AddScaled { from, .. } = kind
            && state.get(from).is_some_and(|w| w.at > prev.at)
        {
            return None;
        }
        Some(prev.at)
    }

    let before = effects.len();
    let mut out: Vec<Eff> = Vec::with_capacity(before);
    let mut state: HashMap<isize, LastWrite> = HashMap::new();

    for eff in core::mem::take(effects) {
        // Fold into the last write to the same cell when nothing in
        // between has read that cell
        if let Some(i) = fold_target(&eff.kind, &state)
            && let Some(kind) = out[i].kind.compose(&eff.kind, dialect)
        {
            out[i] = Eff::new(kind, out[i].span.merge(eff.span));
            continue;
        }

        // This effect stays, so mark every cell it reads as observed
        eff.kind.reads(|cell| {
            if let Some(prev) = state.get_mut(&cell) {
                prev.observed = true;
            }
        });
        if let Some(dest) = eff.kind.writes() {
            state.insert(
                dest,
                LastWrite {
                    at: out.len(),
                    observed: false,
                },
            );
        }
        out.push(eff);
    }

    out.retain(|eff| !eff.kind.is_nop());

    // Only folds and drops change `out`, and both shorten it, so a length
    // change is the same as any change.
    *effects = out;
    effects.len() != before
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

    /// Net pointer movement, or `None` when not known at compile time.
    pub fn net_shift(&self) -> Option<isize> {
        match &self.kind {
            NodeKind::Run(run) => Some(run.shift),
            NodeKind::Loop(l) => (l.body.net_shift()? == 0).then_some(0),
            NodeKind::Scan(_) => None,
        }
    }

    /// Whether the cell under the pointer is known to be zero after this
    /// node runs.
    pub fn exits_on_zero(&self) -> bool {
        match &self.kind {
            NodeKind::Loop(_) | NodeKind::Scan(_) => true,
            NodeKind::Run(run) => {
                let exit = run.shift;
                run.effects
                    .iter()
                    .rev()
                    .find(|eff| eff.kind.writes() == Some(exit))
                    .is_some_and(|eff| eff.kind == EffKind::Store { at: exit, value: 0 })
            }
        }
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

/// A straight-line run: effects addressed relative to the pointer at run
/// entry, followed by one net shift.
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

    /// Append `next` to this run. Its effects were addressed from after
    /// this run's shift, so they are rebased to this run's entry.
    pub fn absorb(&mut self, next: Run) {
        self.effects.extend(
            next.effects
                .into_iter()
                .map(|eff| Eff::new(build::rebase(eff.kind, self.shift), eff.span)),
        );
        self.shift += next.shift;
    }

    /// Whether the run contains an input or output effect.
    pub fn has_io(&self) -> bool {
        self.effects.iter().any(|e| e.kind.is_io())
    }

    /// Neither touches a cell nor moves the pointer, so it can be dropped.
    pub fn is_nop(&self) -> bool {
        self.effects.is_empty() && self.shift == 0
    }
}

/// `while tape[ptr] != 0 { body }`. The cell under the pointer at each
/// test is the loop's *control cell*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loop {
    pub body: Block,
}

impl Loop {
    /// Classify this loop's body.
    pub fn shape(&self) -> LoopShape {
        let [node] = self.body.nodes() else {
            return LoopShape::Other;
        };
        let NodeKind::Run(run) = &node.kind else {
            return LoopShape::Other;
        };
        if run.effects.is_empty() && run.shift != 0 {
            return LoopShape::Scan { stride: run.shift };
        }
        if run.shift != 0 {
            return LoopShape::Other;
        }

        // Sum each cell's deltas exactly. A sum too big for a CellDelta
        // makes the shape `Other`, since there is no dialect here to
        // reduce it against.
        let mut by: i64 = 0;
        let mut sums: Vec<(isize, i64)> = Vec::new();
        for eff in &run.effects {
            let EffKind::Add { at, delta } = eff.kind else {
                return LoopShape::Other;
            };
            if at == 0 {
                by += i64::from(delta);
            } else {
                match sums.iter_mut().find(|(offset, _)| *offset == at) {
                    Some((_, sum)) => *sum += i64::from(delta),
                    None => sums.push((at, i64::from(delta))),
                }
            }
        }

        // Only an odd change is guaranteed to reach zero, see
        // `arith::drain_factor`
        if by % 2 == 0 {
            return LoopShape::Other;
        }
        let Ok(by) = CellDelta::try_from(by) else {
            return LoopShape::Other;
        };
        let mut targets = Vec::with_capacity(sums.len());
        for (at, sum) in sums {
            let Ok(delta) = CellDelta::try_from(sum) else {
                return LoopShape::Other;
            };
            targets.push((at, delta));
        }
        LoopShape::Drain { by, targets }
    }

    /// Every cell the loop can touch, relative to its entry, including its
    /// own test. `None` unless each trip is known to leave the pointer where
    /// it started.
    pub fn footprint(&self) -> Option<Footprint> {
        let mut footprint = Footprint::default();
        footprint.reads.insert(0);
        (self.body.footprint_from(0, &mut footprint)? == 0).then_some(footprint)
    }
}

/// The cells a loop can touch, relative to its entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    pub reads: HashSet<isize>,
    pub writes: HashSet<isize>,
}

/// What a loop's body does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopShape {
    /// Each trip changes the control cell by `by` and adds a constant to
    /// every other touched cell.
    Drain {
        by: CellDelta,
        /// `(offset, per-trip delta)` for the cells receiving the
        /// drained value. Empty for `[-]`; one entry for `[->+<]`; two or
        /// more for copy/fan-out loops like `[->+>+<<]`.
        targets: Vec<(isize, CellDelta)>,
    },
    /// A body that only moves the pointer: `[>]`, `[<<]`.
    Scan { stride: isize },
    /// Anything else.
    Other,
}

/// Advance the pointer by `stride` until `tape[ptr] == 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Never 0. [`build::Builder::scan`] rejects a zero stride.
    pub stride: isize,
}

/// One effect in a run.
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
    /// `tape[ptr + at] += delta`. `delta` is the exact sum of the fused `+`
    /// and `-` commands, reduced only when it would overflow (see
    /// [`arith::fuse_deltas`]).
    Add { at: isize, delta: CellDelta },

    /// `tape[ptr + at] = value`, masked.
    Store { at: isize, value: Cell },

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
            | EffKind::Store { at, .. }
            | EffKind::AddScaled { at, .. }
            | EffKind::Read { at } => Some(*at),
            EffKind::Write { .. } => None,
        }
    }

    /// Every offset this effect reads. `Read` also counts as reading its
    /// cell, since under [`crate::config::EofBehavior::Unchanged`] it can
    /// leave the old value in place.
    pub fn reads(&self, mut sink: impl FnMut(isize)) {
        match self {
            EffKind::Add { at, .. } => sink(*at),
            EffKind::Store { .. } => {}
            EffKind::AddScaled { at, from, .. } => {
                sink(*at);
                sink(*from);
            }
            EffKind::Read { at } => sink(*at),
            EffKind::Write { at } => sink(*at),
        }
    }

    /// Visible outside the tape (I/O).
    pub fn is_io(&self) -> bool {
        matches!(self, EffKind::Read { .. } | EffKind::Write { .. })
    }

    /// Leaves the tape as it found it, so it can be dropped.
    pub fn is_nop(&self) -> bool {
        matches!(
            self,
            EffKind::Add { delta: 0, .. } | EffKind::AddScaled { factor: 0, .. }
        )
    }

    /// Combine this effect with `later`, assuming both write the same cell
    /// and nothing in between touches it. Returns `None` when the
    /// destinations differ or the IR has no single effect for the result.
    pub fn compose(&self, later: &EffKind, dialect: &Dialect) -> Option<EffKind> {
        // Both effects must write the same cell
        if self.writes()? != later.writes()? {
            return None;
        }
        match (self, later) {
            (EffKind::Add { at, delta: a }, EffKind::Add { delta: b, .. }) => Some(EffKind::Add {
                at: *at,
                delta: arith::fuse_deltas(*a, *b, dialect),
            }),
            (EffKind::Store { at, value }, EffKind::Add { delta, .. }) => Some(EffKind::Store {
                at: *at,
                value: arith::apply_delta(*value, *delta, dialect),
            }),
            (
                EffKind::Add { .. } | EffKind::Store { .. } | EffKind::AddScaled { .. },
                EffKind::Store { at, value },
            ) => Some(EffKind::Store {
                at: *at,
                value: *value,
            }),
            (
                EffKind::AddScaled {
                    at,
                    from,
                    factor: a,
                },
                EffKind::AddScaled {
                    from: later_from,
                    factor: b,
                    ..
                },
            ) if from == later_from && from != at => Some(EffKind::AddScaled {
                at: *at,
                from: *from,
                factor: arith::fuse_deltas(*a, *b, dialect),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CellWidth;
    use crate::ir::test_support::*;

    fn u8_dialect() -> Dialect {
        Dialect::default()
    }

    mod compose {
        use super::*;

        #[test]
        fn adds_sum() {
            let d = u8_dialect();
            assert_eq!(add(0, 2).compose(&add(0, 3), &d), Some(add(0, 5)));
        }

        #[test]
        fn add_folds_into_set_at_the_width() {
            let d = u8_dialect();
            assert_eq!(store(0, 255).compose(&add(0, 1), &d), Some(store(0, 0)));
            let d = Dialect {
                cell_width: CellWidth::U16,
                ..Dialect::default()
            };
            assert_eq!(store(0, 255).compose(&add(0, 1), &d), Some(store(0, 256)));
        }

        #[test]
        fn a_later_store_wins() {
            let d = u8_dialect();
            assert_eq!(add(0, 5).compose(&store(0, 7), &d), Some(store(0, 7)));
            assert_eq!(store(0, 1).compose(&store(0, 2), &d), Some(store(0, 2)));
            assert_eq!(scaled(0, 1, 2).compose(&store(0, 9), &d), Some(store(0, 9)));
        }

        #[test]
        fn scaled_adds_with_one_source_sum_factors() {
            let d = u8_dialect();
            assert_eq!(
                scaled(0, 1, 2).compose(&scaled(0, 1, 3), &d),
                Some(scaled(0, 1, 5))
            );
        }

        #[test]
        fn declines_different_destinations() {
            let d = u8_dialect();
            assert_eq!(add(0, 1).compose(&add(1, 1), &d), None);
        }

        #[test]
        fn declines_a_constant_and_a_scaled_term() {
            let d = u8_dialect();
            assert_eq!(add(0, 1).compose(&scaled(0, 1, 2), &d), None);
            assert_eq!(scaled(0, 1, 2).compose(&add(0, 1), &d), None);
            assert_eq!(store(0, 1).compose(&scaled(0, 1, 2), &d), None);
        }

        #[test]
        fn declines_scaled_adds_with_different_or_self_reading_sources() {
            let d = u8_dialect();
            assert_eq!(scaled(0, 1, 2).compose(&scaled(0, 2, 3), &d), None);
            assert_eq!(scaled(0, 0, 2).compose(&scaled(0, 0, 3), &d), None);
        }

        #[test]
        fn declines_io() {
            let d = u8_dialect();
            assert_eq!(EffKind::Read { at: 0 }.compose(&store(0, 1), &d), None);
            assert_eq!(add(0, 1).compose(&EffKind::Read { at: 0 }, &d), None);
            assert_eq!(
                EffKind::Write { at: 0 }.compose(&EffKind::Write { at: 0 }, &d),
                None
            );
        }
    }

    mod absorb {
        use super::*;

        #[test]
        fn rebases_the_second_run_across_the_shift() {
            let mut a = Run {
                effects: vec![Eff::new(add(0, 1), Span::new(0, 1))],
                shift: 2,
            };
            let b = Run {
                effects: vec![Eff::new(scaled(1, 0, 2), Span::new(3, 4))],
                shift: -1,
            };
            a.absorb(b);
            assert_eq!(a.effects.len(), 2);
            assert_eq!(a.effects[0].kind, add(0, 1));
            assert_eq!(a.effects[1].kind, scaled(3, 2, 2));
            assert_eq!(a.effects[1].span, Span::new(3, 4));
            assert_eq!(a.shift, 1);
        }
    }

    mod net_shift {
        use super::*;

        #[test]
        fn sums_the_runs_in_a_block() {
            let block = Block::from_nodes(vec![
                run_node(vec![add(0, 1)], 2),
                loop_node(vec![run_node(vec![add(0, -1)], 0)]),
                run_node(vec![], -1),
            ]);
            assert_eq!(block.net_shift(), Some(1));
        }

        #[test]
        fn a_loop_is_zero_only_when_its_body_is() {
            assert_eq!(
                loop_node(vec![run_node(vec![add(0, -1)], 0)]).net_shift(),
                Some(0)
            );
            let balanced = loop_node(vec![
                run_node(vec![add(0, 1)], 1),
                loop_node(vec![run_node(vec![add(0, -1)], 0)]),
                run_node(vec![], -1),
            ]);
            assert_eq!(balanced.net_shift(), Some(0));
            assert_eq!(loop_node(vec![run_node(vec![], 1)]).net_shift(), None);
        }

        #[test]
        fn a_scan_is_unknown() {
            assert_eq!(scan_node(1).net_shift(), None);
            assert_eq!(
                Block::from_nodes(vec![run_node(vec![], 1), scan_node(1)]).net_shift(),
                None
            );
        }
    }

    mod footprint {
        use super::*;

        fn footprint_of(node: &Node) -> Option<Footprint> {
            match &node.kind {
                NodeKind::Loop(l) => l.footprint(),
                other => panic!("expected a loop, got {other:?}"),
            }
        }

        #[test]
        fn collects_reads_and_writes_across_shifts() {
            // `[+1] += [0]; shift +1`, then `loop { [0] += -1 }`, then
            // `write [+2]; shift -1`, all relative to the loop's entry.
            let l = loop_node(vec![
                run_node(vec![scaled(1, 0, 1)], 1),
                loop_node(vec![run_node(vec![add(0, -1)], 0)]),
                run_node(vec![EffKind::Write { at: 2 }], -1),
            ]);
            let footprint = footprint_of(&l).expect("balanced");
            assert_eq!(footprint.reads, HashSet::from([0, 1, 3]));
            assert_eq!(footprint.writes, HashSet::from([1]));
        }

        #[test]
        fn an_empty_body_reads_only_its_test() {
            let footprint = footprint_of(&loop_node(vec![])).expect("balanced");
            assert_eq!(footprint.reads, HashSet::from([0]));
            assert!(footprint.writes.is_empty());
        }

        #[test]
        fn declines_a_body_that_moves_the_pointer() {
            assert_eq!(
                footprint_of(&loop_node(vec![run_node(vec![add(0, 1)], 1)])),
                None
            );
            assert_eq!(footprint_of(&loop_node(vec![scan_node(1)])), None);
            let nested = loop_node(vec![
                run_node(vec![add(0, 1)], 0),
                loop_node(vec![run_node(vec![], 1)]),
            ]);
            assert_eq!(footprint_of(&nested), None);
        }
    }

    mod normalize {
        use super::*;

        #[test]
        fn merges_adjacent_runs() {
            let mut block = Block {
                nodes: vec![run_node(vec![add(0, 1)], 1), run_node(vec![add(0, 2)], 0)],
            };
            assert!(block.normalize(&u8_dialect()));
            assert_eq!(block.len(), 1);
            assert_eq!(effect_kinds(&block), vec![&add(0, 1), &add(1, 2)]);
            assert_eq!(block.nodes()[0].id, NodeId::UNASSIGNED);
        }

        #[test]
        fn composes_across_the_merge_seam() {
            // `[0] += 1; shift +1` then `[-1] += 2; shift -1` is `[0] += 3`.
            let mut block = Block {
                nodes: vec![run_node(vec![add(0, 1)], 1), run_node(vec![add(-1, 2)], -1)],
            };
            assert!(block.normalize(&u8_dialect()));
            assert_eq!(effect_kinds(&block), vec![&add(0, 3)]);
            match &block.nodes()[0].kind {
                NodeKind::Run(run) => assert_eq!(run.shift, 0),
                other => panic!("expected a run, got {other:?}"),
            }
        }

        #[test]
        fn drops_zero_adds_and_the_run_they_empty() {
            let mut block = Block {
                nodes: vec![run_node(vec![add(0, 1), add(0, -1)], 0)],
            };
            assert!(block.normalize(&u8_dialect()));
            assert!(block.is_empty());
        }

        #[test]
        fn drops_scaled_adds_that_cancel_to_a_zero_factor() {
            // `[0] += [+1] * 1` then `[0] += [+1] * -1` adds nothing.
            let mut block = Block {
                nodes: vec![run_node(vec![scaled(0, 1, 1), scaled(0, 1, -1)], 0)],
            };
            assert!(block.normalize(&u8_dialect()));
            assert!(block.is_empty());
        }

        #[test]
        fn keeps_a_shift_only_run() {
            let mut block = Block {
                nodes: vec![run_node(vec![add(0, 1), add(0, -1)], 2)],
            };
            assert!(block.normalize(&u8_dialect()));
            assert_eq!(block.len(), 1);
            assert!(effect_kinds(&block).is_empty());
        }

        #[test]
        fn composes_past_effects_on_other_cells() {
            let mut block = Block {
                nodes: vec![run_node(vec![add(0, 1), add(1, 5), add(0, 2)], 0)],
            };
            assert!(block.normalize(&u8_dialect()));
            assert_eq!(effect_kinds(&block), vec![&add(0, 3), &add(1, 5)]);
        }

        #[test]
        fn declines_composition_past_a_read_of_the_cell() {
            let mut block = Block {
                nodes: vec![run_node(
                    vec![add(0, 1), EffKind::Write { at: 0 }, add(0, 2)],
                    0,
                )],
            };
            assert!(!block.normalize(&u8_dialect()));
            assert_eq!(
                effect_kinds(&block),
                vec![&add(0, 1), &EffKind::Write { at: 0 }, &add(0, 2)]
            );
        }

        #[test]
        fn declines_composition_past_a_write_to_the_scaled_source() {
            // `[+1] += [+2]` ... `[+2] += 1` ... `[+1] += [+2]`:
            // summing the factors would read the updated source twice.
            let mut block = Block {
                nodes: vec![run_node(
                    vec![scaled(1, 2, 1), add(2, 1), scaled(1, 2, 1)],
                    0,
                )],
            };
            assert!(!block.normalize(&u8_dialect()));
            assert_eq!(
                effect_kinds(&block),
                vec![&scaled(1, 2, 1), &add(2, 1), &scaled(1, 2, 1)]
            );
        }

        #[test]
        fn a_canonical_block_reports_no_change() {
            let mut block = Block {
                nodes: vec![
                    run_node(vec![add(0, 3)], 0),
                    Node::new(
                        NodeKind::Loop(Loop {
                            body: Block {
                                nodes: vec![run_node(vec![add(0, -1)], 0)],
                            },
                        }),
                        Span::SYNTHETIC,
                    ),
                    run_node(vec![], 1),
                ],
            };
            assert!(!block.normalize(&u8_dialect()));
        }
    }

    mod exits_on_zero {
        use super::*;

        #[test]
        fn loops_and_scans_do() {
            let l = Node::new(
                NodeKind::Loop(Loop {
                    body: Block {
                        nodes: vec![run_node(vec![add(0, 1)], 0)],
                    },
                }),
                Span::SYNTHETIC,
            );
            assert!(l.exits_on_zero());
            let s = Node::new(NodeKind::Scan(Scan { stride: 1 }), Span::SYNTHETIC);
            assert!(s.exits_on_zero());
        }

        #[test]
        fn a_run_whose_last_write_to_the_cell_is_a_clear_does() {
            assert!(run_node(vec![store(0, 0)], 0).exits_on_zero());
            assert!(run_node(vec![store(0, 0), add(1, 1)], 0).exits_on_zero());
            assert!(run_node(vec![store(0, 0), EffKind::Write { at: 0 }], 0).exits_on_zero());
        }

        #[test]
        fn a_later_write_to_the_cell_hides_the_clear() {
            assert!(!run_node(vec![store(0, 0), add(0, 1)], 0).exits_on_zero());
            assert!(
                !run_node(vec![store(0, 0), EffKind::Write { at: 0 }, add(0, 3)], 0).exits_on_zero()
            );
            assert!(!run_node(vec![store(0, 0), EffKind::Read { at: 0 }], 0).exits_on_zero());
        }

        #[test]
        fn the_exit_cell_is_the_one_under_the_shifted_pointer() {
            // `>[-]` canonicalizes to `[+1] = 0; shift +1`.
            assert!(run_node(vec![store(1, 0)], 1).exits_on_zero());
            assert!(run_node(vec![store(0, 5), store(-2, 0)], -2).exits_on_zero());
            assert!(!run_node(vec![store(0, 0)], 1).exits_on_zero());
            assert!(!run_node(vec![store(1, 0), add(1, 1)], 1).exits_on_zero());
        }

        #[test]
        fn other_stores_do_not() {
            assert!(!run_node(vec![store(0, 1)], 0).exits_on_zero());
            assert!(!run_node(vec![add(0, -1)], 0).exits_on_zero());
            assert!(!run_node(vec![], 0).exits_on_zero());
        }
    }

    mod validate {
        use super::*;

        fn program(nodes: Vec<Node>) -> Program {
            Program::new(Block { nodes })
        }

        #[test]
        fn a_canonical_program_passes() {
            let p = program(vec![
                run_node(vec![add(0, 1)], 0),
                loop_node(vec![run_node(vec![add(0, -1)], 0), scan_node(1)]),
                run_node(vec![store(0, 255)], 1),
            ]);
            assert_eq!(p.validate(&u8_dialect()), Ok(()));
        }

        #[test]
        fn an_empty_program_passes() {
            assert_eq!(program(vec![]).validate(&u8_dialect()), Ok(()));
        }

        #[test]
        fn adjacent_runs_fail() {
            let p = program(vec![run_node(vec![add(0, 1)], 0), run_node(vec![], 1)]);
            assert_eq!(
                p.validate(&u8_dialect()),
                Err("node 1 is a run next to a run".to_string())
            );
        }

        #[test]
        fn a_run_that_does_nothing_fails() {
            let p = program(vec![run_node(vec![], 0)]);
            assert_eq!(
                p.validate(&u8_dialect()),
                Err("node 0 is a run that does nothing".to_string())
            );
        }

        #[test]
        fn a_zero_stride_scan_fails() {
            let p = program(vec![loop_node(vec![scan_node(0)])]);
            assert_eq!(
                p.validate(&u8_dialect()),
                Err("node 0.0 is a scan with a stride of 0".to_string())
            );
        }

        #[test]
        fn an_oversized_store_fails_only_at_a_narrow_width() {
            let p = program(vec![run_node(vec![store(0, 256)], 0)]);
            assert_eq!(
                p.validate(&u8_dialect()),
                Err("node 0 stores 256, which does not fit the cell width".to_string())
            );
            let u16 = Dialect {
                cell_width: CellWidth::U16,
                ..Dialect::default()
            };
            assert_eq!(p.validate(&u16), Ok(()));
        }

        #[test]
        fn the_path_reaches_into_nested_bodies() {
            // Built by hand, since `Block::from_nodes` would merge the runs.
            let inner = Node::new(
                NodeKind::Loop(Loop {
                    body: Block {
                        nodes: vec![run_node(vec![add(0, 1)], 0), run_node(vec![add(0, 1)], 0)],
                    },
                }),
                Span::SYNTHETIC,
            );
            let p = program(vec![run_node(vec![add(0, 1)], 0), loop_node(vec![inner])]);
            assert_eq!(
                p.validate(&u8_dialect()),
                Err("node 1.0.1 is a run next to a run".to_string())
            );
        }
    }

    #[test]
    fn for_each_block_mut_visits_innermost_first_and_self_last() {
        // Distinct lengths: inner body 1 node, outer body 2, top level 3.
        let inner = Node::new(
            NodeKind::Loop(Loop {
                body: Block {
                    nodes: vec![run_node(vec![add(0, -1)], 0)],
                },
            }),
            Span::SYNTHETIC,
        );
        let outer = Node::new(
            NodeKind::Loop(Loop {
                body: Block {
                    nodes: vec![run_node(vec![add(0, 1)], 0), inner],
                },
            }),
            Span::SYNTHETIC,
        );
        let mut program = Program::new(Block {
            nodes: vec![run_node(vec![add(0, 1)], 0), outer, run_node(vec![], 1)],
        });

        let mut visited = Vec::new();
        program.for_each_block_mut(&mut |b, site| visited.push((b.len(), site)));
        assert_eq!(
            visited,
            vec![
                (1, BlockSite::LoopBody),
                (2, BlockSite::LoopBody),
                (3, BlockSite::Program),
            ]
        );
    }
}

//! The optimizer's intermediate representation.
//!
//! A [`Block`] is a canonical sequence of run / loop / scan nodes. Within a
//! [`Run`], effect offsets are relative to the pointer *at run entry* and the
//! pointer moves exactly once, at the end.

pub mod arith;
pub mod build;
pub mod lower;
pub mod print;

use std::collections::HashMap;

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

    /// Total effects plus control nodes.
    pub fn op_count(&self) -> usize {
        let mut count = 0;
        let mut stack = vec![self.body.nodes()];
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
        self.nodes.splice(range, replacement);
        self.canonicalize();
    }

    /// Offer every loop in this block (not nested ones) to `f`; where it
    /// returns a replacement, splice it in.
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

    /// Restore canonical and locally reduced form by merging adjacent runs,
    /// composing same-cell effect pairs, dropping `+= 0` adds and empty runs.
    /// Returns whether anything changed.
    pub fn normalize(&mut self, dialect: &Dialect) -> bool {
        let mut changed = self.canonicalize();

        for node in &mut self.nodes {
            if let NodeKind::Run(run) = &mut node.kind {
                changed |= normalize_effects(&mut run.effects, dialect);
            }
        }

        // Canonicalize again in case there are empty runs now
        changed | self.canonicalize()
    }

    /// Visit every block in this subtree, innermost first, `self` last.
    pub fn for_each_block_mut(&mut self, f: &mut impl FnMut(&mut Block)) {
        for node in &mut self.nodes {
            if let Some(body) = node.kind.body_mut() {
                body.for_each_block_mut(f);
            }
        }
        f(self);
    }

    /// Net pointer movement, or `None` when not statically known.
    pub fn net_shift(&self) -> Option<isize> {
        todo!("Block::net_shift")
    }
}

/// Fold each effect into the most recent write to the same cell, where
/// nothing in between observes that cell, and drop effects that do nothing.
/// Returns whether anything changed.
fn normalize_effects(effects: &mut Vec<Eff>, dialect: &Dialect) -> bool {
    struct LastWrite {
        /// Index in `out` vec.
        at: usize,
        /// Whether anything has read the cell since.
        observed: bool,
    }

    /// Index in `out` of the write `kind` can fold into, if any.
    fn fold_target(kind: &EffKind, state: &HashMap<isize, LastWrite>) -> Option<usize> {
        let prev = state.get(&kind.writes()?)?;
        if prev.observed {
            return None;
        }
        // Folding moves this effect, and its read of `from`, back to
        // `prev.at`, so decline when a write to `from` sits in between
        // and the read would no longer see it.
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
        // Fold all contiguous foldable effects
        if let Some(i) = fold_target(&eff.kind, &state)
            && let Some(kind) = out[i].kind.compose(&eff.kind, dialect)
        {
            out[i] = Eff::new(kind, out[i].span.merge(eff.span));
            continue;
        }

        // This effect is not foldable, so make sure its reads are observed
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

    // Every fold and every drop shortens `out`, and nothing else mutates it.
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

    /// Sequence `next` onto the end of this run. Its effects are rebased
    /// onto this run's entry, since they were addressed from after the shift.
    pub fn absorb(&mut self, next: Run) {
        self.effects.extend(
            next.effects
                .into_iter()
                .map(|eff| Eff::new(build::rebase(eff.kind, self.shift), eff.span)),
        );
        self.shift += next.shift;
    }

    /// No reads, no writes.
    pub fn is_pure(&self) -> bool {
        self.effects.iter().all(|e| !e.kind.is_observable())
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

        // Exact sums per cell; a sum too big for CellDelta declines rather
        // than reduce, since shape has no dialect to reduce against.
        let mut step: i64 = 0;
        let mut sums: Vec<(isize, i64)> = Vec::new();
        for eff in &run.effects {
            let EffKind::Add { at, delta } = eff.kind else {
                return LoopShape::Other;
            };
            if at == 0 {
                step += i64::from(delta);
            } else {
                match sums.iter_mut().find(|(offset, _)| *offset == at) {
                    Some((_, sum)) => *sum += i64::from(delta),
                    None => sums.push((at, i64::from(delta))),
                }
            }
        }

        // Only an odd step is a guaranteed to reach zero
        if step % 2 == 0 {
            return LoopShape::Other;
        }
        let Ok(step) = CellDelta::try_from(step) else {
            return LoopShape::Other;
        };
        let mut targets = Vec::with_capacity(sums.len());
        for (at, sum) in sums {
            let Ok(delta) = CellDelta::try_from(sum) else {
                return LoopShape::Other;
            };
            targets.push((at, delta));
        }
        LoopShape::Drain { step, targets }
    }
}

/// What a loop's body means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopShape {
    /// The control cell steps by `step` toward zero; every other touched
    /// cell accumulates a constant per trip.
    Drain {
        step: CellDelta,
        /// `(offset, per-trip delta)` for the cells receiving the
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

    /// Leaves the tape as it found it, so it can be dropped.
    pub fn is_nop(&self) -> bool {
        matches!(
            self,
            EffKind::Add { delta: 0, .. } | EffKind::AddScaled { factor: 0, .. }
        )
    }

    /// Compose two same-destination effects with no intervening effects for
    /// that cell. Declines (`None`) when the destinations differ or the
    /// combination has no single-effect equivalent in the IR.
    pub fn compose(&self, later: &EffKind, dialect: &Dialect) -> Option<EffKind> {
        // Both effects must write to the same cell to be composeable
        if self.writes()? != later.writes()? {
            return None;
        }
        match (self, later) {
            (EffKind::Add { at, delta: a }, EffKind::Add { delta: b, .. }) => Some(EffKind::Add {
                at: *at,
                delta: arith::fuse_deltas(*a, *b, dialect),
            }),
            (EffKind::Set { at, value }, EffKind::Add { delta, .. }) => Some(EffKind::Set {
                at: *at,
                value: arith::apply_delta(*value, *delta, dialect),
            }),
            (
                EffKind::Add { .. } | EffKind::Set { .. } | EffKind::AddScaled { .. },
                EffKind::Set { at, value },
            ) => Some(EffKind::Set {
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

    fn u8_dialect() -> Dialect {
        Dialect::default()
    }

    fn add(at: isize, delta: CellDelta) -> EffKind {
        EffKind::Add { at, delta }
    }

    fn set(at: isize, value: Cell) -> EffKind {
        EffKind::Set { at, value }
    }

    fn scaled(at: isize, from: isize, factor: CellDelta) -> EffKind {
        EffKind::AddScaled { at, from, factor }
    }

    fn run_node(effects: Vec<EffKind>, shift: isize) -> Node {
        let effects = effects
            .into_iter()
            .map(|kind| Eff::new(kind, Span::SYNTHETIC))
            .collect();
        Node::new(NodeKind::Run(Run { effects, shift }), Span::SYNTHETIC)
    }

    fn effect_kinds(block: &Block) -> Vec<&EffKind> {
        block
            .nodes()
            .iter()
            .flat_map(|node| match &node.kind {
                NodeKind::Run(run) => run.effects.iter().map(|e| &e.kind).collect(),
                _ => Vec::new(),
            })
            .collect()
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
            assert_eq!(set(0, 255).compose(&add(0, 1), &d), Some(set(0, 0)));
            let d = Dialect {
                cell_width: CellWidth::U16,
                ..Dialect::default()
            };
            assert_eq!(set(0, 255).compose(&add(0, 1), &d), Some(set(0, 256)));
        }

        #[test]
        fn a_later_set_supersedes_updates() {
            let d = u8_dialect();
            assert_eq!(add(0, 5).compose(&set(0, 7), &d), Some(set(0, 7)));
            assert_eq!(set(0, 1).compose(&set(0, 2), &d), Some(set(0, 2)));
            assert_eq!(scaled(0, 1, 2).compose(&set(0, 9), &d), Some(set(0, 9)));
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
            assert_eq!(set(0, 1).compose(&scaled(0, 1, 2), &d), None);
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
            assert_eq!(EffKind::Read { at: 0 }.compose(&set(0, 1), &d), None);
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
            // `[p] += 1; p += 1` then `[p-1] += 2; p -= 1` is `[p] += 3`.
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
            // `[p] += [p+1] * 1` then `[p] += [p+1] * -1` adds nothing.
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
            // `[p+1] += [p+2]` ... `[p+2] += 1` ... `[p+1] += [p+2]`:
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
        let mut block = Block {
            nodes: vec![run_node(vec![add(0, 1)], 0), outer, run_node(vec![], 1)],
        };

        let mut visited = Vec::new();
        block.for_each_block_mut(&mut |b| visited.push(b.len()));
        assert_eq!(visited, vec![1, 2, 3]);
    }
}

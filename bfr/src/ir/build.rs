//! Incremental IR construction with cursor tracking.

use crate::error::Span;
use crate::ir::{Block, Eff, EffKind, Loop, Node, NodeKind, Program, Run, Scan};

pub struct Builder {
    /// One frame per unclosed loop, plus the program body at the bottom.
    frames: Vec<Frame>,
}

/// A block's completed nodes, plus the current run the cursor is walking through.
struct Frame {
    nodes: Vec<Node>,
    /// Span of the `[` that opened this frame.
    open_span: Span,
    /// Effects of the open run, already rebased to run-entry coordinates.
    effects: Vec<Eff>,
    /// Merged span of the open run, shift characters included.
    run_span: Span,
    /// Cursor offset from the open run's entry.
    cursor: isize,
}

impl Frame {
    fn new(open_span: Span) -> Frame {
        Frame {
            nodes: Vec::new(),
            open_span,
            effects: Vec::new(),
            run_span: Span::SYNTHETIC,
            cursor: 0,
        }
    }

    /// Close the open run into a node, unless it does nothing.
    fn seal(&mut self) {
        if self.effects.is_empty() && self.cursor == 0 {
            self.run_span = Span::SYNTHETIC;
            return;
        }
        let run = Run {
            effects: core::mem::take(&mut self.effects),
            shift: self.cursor,
        };
        let span = core::mem::replace(&mut self.run_span, Span::SYNTHETIC);
        self.nodes.push(Node::new(NodeKind::Run(run), span));
        self.cursor = 0;
    }
}

/// Rebase an effect's offsets by `cursor`.
pub fn rebase(kind: EffKind, cursor: isize) -> EffKind {
    match kind {
        EffKind::Add { at, delta } => EffKind::Add {
            at: at + cursor,
            delta,
        },
        EffKind::Set { at, value } => EffKind::Set {
            at: at + cursor,
            value,
        },
        EffKind::AddScaled { at, from, factor } => EffKind::AddScaled {
            at: at + cursor,
            from: from + cursor,
            factor,
        },
        EffKind::Read { at } => EffKind::Read { at: at + cursor },
        EffKind::Write { at } => EffKind::Write { at: at + cursor },
    }
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            frames: vec![Frame::new(Span::SYNTHETIC)],
        }
    }

    fn top(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("the bottom frame is never popped")
    }

    /// `>` / `<`: move the cursor. The span merges into the current run.
    pub fn bump(&mut self, delta: isize, span: Span) {
        let frame = self.top();
        frame.cursor += delta;
        frame.run_span = frame.run_span.merge(span);
    }

    /// Push one effect at cursor-relative offsets.
    pub fn push(&mut self, kind: EffKind, span: Span) {
        let frame = self.top();
        let kind = rebase(kind, frame.cursor);
        frame.run_span = frame.run_span.merge(span);
        frame.effects.push(Eff::new(kind, span));
    }

    /// Seal the current run and emit a [`crate::ir::Scan`] node.
    ///
    /// # Panics
    ///
    /// Panics if `stride` is 0.
    pub fn scan(&mut self, stride: isize, span: Span) {
        assert!(stride != 0, "scan stride must be nonzero");
        let frame = self.top();
        frame.seal();
        frame
            .nodes
            .push(Node::new(NodeKind::Scan(Scan { stride }), span));
    }

    /// `[`: seal the current run and open a loop body.
    pub fn begin_loop(&mut self, span: Span) {
        self.top().seal();
        self.frames.push(Frame::new(span));
    }

    /// `]`: close the innermost loop.
    ///
    /// # Panics
    ///
    /// Panics if no loop is open.
    pub fn end_loop(&mut self, span: Span) {
        assert!(self.frames.len() > 1, "end_loop with no open loop");
        let mut frame = self.frames.pop().unwrap();
        frame.seal();

        let mut node_span = frame.open_span.merge(span);
        for node in &frame.nodes {
            node_span = node_span.merge(node.span);
        }

        let body = Block { nodes: frame.nodes };
        self.top()
            .nodes
            .push(Node::new(NodeKind::Loop(Loop { body }), node_span));
    }

    /// Seal the final run and hand back the program.
    ///
    /// # Panics
    ///
    /// Panics if a loop is still open.
    pub fn finish(mut self) -> Program {
        assert!(self.frames.len() == 1, "finish with an open loop");
        let mut frame = self.frames.pop().unwrap();
        frame.seal();
        Program::new(Block { nodes: frame.nodes })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::NodeId;

    /// The span of one source character at byte `i`.
    fn sp(i: usize) -> Span {
        Span::new(i, i + 1)
    }

    fn expect_run(node: &Node) -> &Run {
        match &node.kind {
            NodeKind::Run(run) => run,
            other => panic!("expected a run, got {other:?}"),
        }
    }

    fn expect_loop(node: &Node) -> &Loop {
        match &node.kind {
            NodeKind::Loop(l) => l,
            other => panic!("expected a loop, got {other:?}"),
        }
    }

    #[test]
    fn movement_folds_into_one_run() {
        // >>+<
        let mut b = Builder::new();
        b.bump(1, sp(0));
        b.bump(1, sp(1));
        b.push(EffKind::Add { at: 0, delta: 1 }, sp(2));
        b.bump(-1, sp(3));
        let program = b.finish();

        let nodes = program.body.nodes();
        assert_eq!(nodes.len(), 1);
        let run = expect_run(&nodes[0]);
        assert_eq!(run.effects.len(), 1);
        assert_eq!(run.effects[0].kind, EffKind::Add { at: 2, delta: 1 });
        assert_eq!(run.shift, 1);
        // Provenance covers all four characters, shift characters included.
        assert_eq!(nodes[0].span, Span::new(0, 4));
        assert_eq!(nodes[0].id, NodeId::UNASSIGNED);
    }

    #[test]
    fn cancelling_movement_produces_nothing() {
        // ><
        let mut b = Builder::new();
        b.bump(1, sp(0));
        b.bump(-1, sp(1));
        let program = b.finish();
        assert!(program.body.is_empty());
    }

    #[test]
    fn negative_offsets_rebase() {
        // <+
        let mut b = Builder::new();
        b.bump(-1, sp(0));
        b.push(EffKind::Add { at: 0, delta: 1 }, sp(1));
        let program = b.finish();

        let run = expect_run(&program.body.nodes()[0]);
        assert_eq!(run.effects[0].kind, EffKind::Add { at: -1, delta: 1 });
        assert_eq!(run.shift, -1);
    }

    #[test]
    fn rebase_applies_to_both_add_scaled_offsets() {
        let mut b = Builder::new();
        b.bump(3, sp(0));
        b.push(
            EffKind::AddScaled {
                at: 1,
                from: 0,
                factor: 2,
            },
            sp(1),
        );
        let program = b.finish();

        let run = expect_run(&program.body.nodes()[0]);
        assert_eq!(
            run.effects[0].kind,
            EffKind::AddScaled {
                at: 4,
                from: 3,
                factor: 2
            }
        );
    }

    #[test]
    fn loops_seal_runs_and_own_their_bodies() {
        // +[-]>
        let mut b = Builder::new();
        b.push(EffKind::Add { at: 0, delta: 1 }, sp(0));
        b.begin_loop(sp(1));
        b.push(EffKind::Add { at: 0, delta: -1 }, sp(2));
        b.end_loop(sp(3));
        b.bump(1, sp(4));
        let program = b.finish();

        let nodes = program.body.nodes();
        assert_eq!(nodes.len(), 3);
        assert_eq!(expect_run(&nodes[0]).shift, 0);
        let body = expect_loop(&nodes[1]).body.nodes();
        assert_eq!(body.len(), 1);
        assert_eq!(
            expect_run(&body[0]).effects[0].kind,
            EffKind::Add { at: 0, delta: -1 }
        );
        assert_eq!(expect_run(&nodes[2]).shift, 1);
    }

    #[test]
    fn loop_spans_cover_brackets_and_body() {
        // [-]
        let mut b = Builder::new();
        b.begin_loop(sp(0));
        b.push(EffKind::Add { at: 0, delta: -1 }, sp(1));
        b.end_loop(sp(2));
        let program = b.finish();

        assert_eq!(program.body.nodes()[0].span, Span::new(0, 3));
    }

    #[test]
    fn cursor_is_per_frame() {
        // >[>+]
        let mut b = Builder::new();
        b.bump(1, sp(0));
        b.begin_loop(sp(1));
        b.bump(1, sp(2));
        b.push(EffKind::Add { at: 0, delta: 1 }, sp(3));
        b.end_loop(sp(4));
        let program = b.finish();

        let nodes = program.body.nodes();
        // The `>` before the loop is its own shift-only run.
        assert_eq!(expect_run(&nodes[0]).shift, 1);
        // The body's offsets are relative to the body's own entry.
        let body = expect_loop(&nodes[1]).body.nodes();
        let run = expect_run(&body[0]);
        assert_eq!(run.effects[0].kind, EffKind::Add { at: 1, delta: 1 });
        assert_eq!(run.shift, 1);
    }

    #[test]
    fn nested_loops() {
        // [[-]]
        let mut b = Builder::new();
        b.begin_loop(sp(0));
        b.begin_loop(sp(1));
        b.push(EffKind::Add { at: 0, delta: -1 }, sp(2));
        b.end_loop(sp(3));
        b.end_loop(sp(4));
        let program = b.finish();

        let outer = expect_loop(&program.body.nodes()[0]);
        let inner = expect_loop(&outer.body.nodes()[0]);
        assert_eq!(inner.body.nodes().len(), 1);
    }

    #[test]
    fn empty_loop_body_is_allowed() {
        // []
        let mut b = Builder::new();
        b.begin_loop(sp(0));
        b.end_loop(sp(1));
        let program = b.finish();

        assert!(expect_loop(&program.body.nodes()[0]).body.is_empty());
    }

    #[test]
    fn scan_seals_the_open_run() {
        let mut b = Builder::new();
        b.push(EffKind::Add { at: 0, delta: 1 }, sp(0));
        b.scan(2, sp(1));
        let program = b.finish();

        let nodes = program.body.nodes();
        assert_eq!(nodes.len(), 2);
        expect_run(&nodes[0]);
        match &nodes[1].kind {
            NodeKind::Scan(scan) => assert_eq!(scan.stride, 2),
            other => panic!("expected a scan, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "no open loop")]
    fn end_loop_without_open_loop_panics() {
        Builder::new().end_loop(sp(0));
    }

    #[test]
    #[should_panic(expected = "open loop")]
    fn finish_with_open_loop_panics() {
        let mut b = Builder::new();
        b.begin_loop(sp(0));
        b.finish();
    }

    #[test]
    #[should_panic(expected = "nonzero")]
    fn zero_stride_scan_panics() {
        Builder::new().scan(0, sp(0));
    }
}

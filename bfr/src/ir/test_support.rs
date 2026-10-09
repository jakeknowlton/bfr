//! Builders and matchers shared by the tests of the IR and the passes.
//!
//! Every node is built with a synthetic span and no id, and every block
//! goes through [`Block::from_nodes`], so it is canonical.

use crate::error::Span;
use crate::ir::{Block, Cell, CellDelta, Eff, EffKind, Loop, Node, NodeKind, Program, Run, Scan};

pub fn add(at: isize, delta: CellDelta) -> EffKind {
    EffKind::Add { at, delta }
}

pub fn store(at: isize, value: Cell) -> EffKind {
    EffKind::Store { at, value }
}

pub fn scaled(at: isize, from: isize, factor: CellDelta) -> EffKind {
    EffKind::AddScaled { at, from, factor }
}

pub fn read(at: isize) -> EffKind {
    EffKind::Read { at }
}

pub fn write(at: isize) -> EffKind {
    EffKind::Write { at }
}

pub fn run_node(effects: Vec<EffKind>, shift: isize) -> Node {
    let effects = effects
        .into_iter()
        .map(|kind| Eff::new(kind, Span::SYNTHETIC))
        .collect();
    Node::new(NodeKind::Run(Run { effects, shift }), Span::SYNTHETIC)
}

pub fn loop_node(body: Vec<Node>) -> Node {
    let body = Block::from_nodes(body);
    Node::new(NodeKind::Loop(Loop { body }), Span::SYNTHETIC)
}

pub fn scan_node(stride: isize) -> Node {
    Node::new(NodeKind::Scan(Scan { stride }), Span::SYNTHETIC)
}

pub fn program_of(nodes: Vec<Node>) -> Program {
    Program::new(Block::from_nodes(nodes))
}

/// The span of one source character at byte `i`.
pub fn sp(i: usize) -> Span {
    Span::new(i, i + 1)
}

/// The kinds of every effect in the block's runs, in order.
pub fn effect_kinds(block: &Block) -> Vec<&EffKind> {
    block
        .nodes()
        .iter()
        .flat_map(|node| match &node.kind {
            NodeKind::Run(run) => run.effects.iter().map(|e| &e.kind).collect(),
            _ => Vec::new(),
        })
        .collect()
}

pub fn expect_run(node: &Node) -> &Run {
    match &node.kind {
        NodeKind::Run(run) => run,
        other => panic!("expected a run, got {other:?}"),
    }
}

pub fn expect_loop(node: &Node) -> &Loop {
    match &node.kind {
        NodeKind::Loop(l) => l,
        other => panic!("expected a loop, got {other:?}"),
    }
}

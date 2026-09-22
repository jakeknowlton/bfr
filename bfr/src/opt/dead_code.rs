//! Deletes loop tests that cannot matter, where the control cell must be
//! zero: at program start, or after a node that [`Node::exits_on_zero`].
//!
//! A loop whose body sets the last cell of the loop to zero always runs
//! at most once (like an `if` statement). When that body is a single
//! loop/scan or a single `[p] = 0` (which touches only the cell the loop
//! test read), the outer loop "shell" can be removed.
//!
//! Examples
//!
//! `while [p] { [p] -= 1 }; [p] += 1` at program start becomes `[p] += 1`.
//! `while [p] { [p] -= 1 }; while [p] { [p+1] += 1 }` becomes `while [p] { [p] -= 1 }` (a loop only exits at zero).
//! `[p] = 0; while [p] { [p+1] += 1 }` becomes `[p] = 0`.
//! `while [p] { while [p] { [p] -= 1 } }` becomes `while [p] { [p] -= 1 }`.
//! `while [p] { [p] = 0 }` becomes `[p] = 0`.

use crate::error::Span;
use crate::ir::{Block, BlockSite, Loop, Node, NodeKind, Program};
use crate::opt::{Changed, Ctx, Pass};

pub struct DeadCode;

impl Pass for DeadCode {
    fn name(&self) -> &'static str {
        "DeadCode"
    }

    fn run(&self, program: &mut Program, _ctx: &Ctx<'_>) -> Changed {
        let mut changed = false;
        program.for_each_block_mut(&mut |block, site| {
            changed |= block.try_replace_loops(unwrap_shell);
            changed |= sweep(block, site == BlockSite::Program);
        });
        changed
    }
}

/// A shell is a loop whose only body node makes the loop's test
/// redundant. Lift that inner node out.
fn unwrap_shell(l: &Loop, span: Span) -> Option<Vec<Node>> {
    let [inner] = l.body.nodes() else {
        return None;
    };
    if !inner.exits_on_zero() {
        return None;
    }
    let self_guarded = match &inner.kind {
        // Tests the same cell itself, so the shell's test is a repeat
        NodeKind::Loop(_) | NodeKind::Scan(_) => true,
        // A single `[p] = 0` touches only the test cell and has no
        // other effects, so the shell can be safely dropped.
        NodeKind::Run(run) => run.shift == 0 && run.effects.len() == 1,
    };
    if !self_guarded {
        return None;
    }
    let mut node = inner.clone();
    node.span = span.merge(node.span);
    Some(vec![node])
}

/// Remove dead loops, rescanning after each removal since splicing merges
/// neighbors. `zero_on_entry` is whether the control cell is provably zero
/// on entry.
fn sweep(block: &mut Block, zero_on_entry: bool) -> bool {
    let mut changed = false;
    while let Some(i) = find_dead_loop(block, zero_on_entry) {
        block.splice(i..i + 1, Vec::new());
        changed = true;
    }
    changed
}

fn find_dead_loop(block: &Block, zero_on_entry: bool) -> Option<usize> {
    let nodes = block.nodes();
    nodes.iter().enumerate().position(|(i, node)| {
        matches!(node.kind, NodeKind::Loop(_))
            && match i.checked_sub(1) {
                None => zero_on_entry,
                Some(prev) => nodes[prev].exits_on_zero(),
            }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Dialect;
    use crate::error::Span;
    use crate::ir::{Eff, EffKind, Loop, Node, Run, Scan, lower, print};
    use crate::parser;

    fn swept(src: &str) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = DeadCode.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    /// Assemble a program through the public block-editing API.
    fn program_of(nodes: Vec<Node>) -> Program {
        Program::new(Block::from_nodes(nodes))
    }

    fn loop_node(body_nodes: Vec<Node>) -> Node {
        let body = Block::from_nodes(body_nodes);
        Node::new(NodeKind::Loop(Loop { body }), Span::SYNTHETIC)
    }

    fn run_node(effects: Vec<EffKind>, shift: isize) -> Node {
        let effects = effects
            .into_iter()
            .map(|kind| Eff::new(kind, Span::SYNTHETIC))
            .collect();
        Node::new(NodeKind::Run(Run { effects, shift }), Span::SYNTHETIC)
    }

    fn add(at: isize, delta: i32) -> EffKind {
        EffKind::Add { at, delta }
    }

    #[test]
    fn comment_loops_at_program_start_die() {
        let (program, changed) = swept("[foo.bar][baz]+");
        assert!(changed);
        assert_eq!(print::print(&program), "[p] += 1\n");
    }

    #[test]
    fn a_loop_after_a_loop_dies() {
        let (program, changed) = swept("+[-][+]");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] += 1\nwhile [p] {\n  [p] -= 1\n}\n"
        );
    }

    #[test]
    fn a_loop_at_a_loop_body_start_survives() {
        let (program, changed) = swept("+[[-]+]");
        assert!(!changed);
        let expected = [
            "[p] += 1\n",
            "while [p] {\n",
            "  while [p] {\n",
            "    [p] -= 1\n",
            "  }\n",
            "  [p] += 1\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_shell_around_a_loop_unwraps() {
        let (program, changed) = swept("+[[-]]");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] += 1\nwhile [p] {\n  [p] -= 1\n}\n"
        );
    }

    #[test]
    fn nested_shells_unwrap_in_one_run() {
        let (program, changed) = swept("+[[[>]]]");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] += 1\nwhile [p] {\n  p += 1\n}\n"
        );
    }

    #[test]
    fn a_shell_around_a_clearing_store_collapses() {
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            loop_node(vec![run_node(vec![EffKind::Set { at: 0, value: 0 }], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(program.body.len(), 1);
        assert_eq!(print::print(&program), "[p] += 1\n[p] = 0\n");
    }

    #[test]
    fn a_shell_survives_when_the_body_touches_another_cell() {
        // Runs at most once, but straight-lining would touch `[p+1]`
        // when the original never does.
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            loop_node(vec![run_node(
                vec![add(1, 1), EffKind::Set { at: 0, value: 0 }],
                0,
            )]),
        ]);
        let dialect = Dialect::default();
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }

    #[test]
    fn the_unwrapped_node_keeps_the_shell_span() {
        let (program, _) = swept("+[[-]]");
        // `[[-]]` spans bytes 1..6; the surviving loop covers all of it.
        assert_eq!(program.body.nodes()[1].span, Span::new(1, 6));
    }

    #[test]
    fn a_loop_after_a_clearing_store_dies() {
        let mut program = program_of(vec![
            run_node(vec![EffKind::Set { at: 0, value: 0 }], 0),
            loop_node(vec![run_node(vec![add(1, 1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(print::print(&program), "[p] = 0\n");
    }

    #[test]
    fn a_shift_off_the_cleared_cell_keeps_the_loop() {
        let mut program = program_of(vec![
            run_node(vec![EffKind::Set { at: 0, value: 0 }], 1),
            loop_node(vec![run_node(vec![add(0, -1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }

    #[test]
    fn a_loop_after_a_clear_of_the_landing_cell_dies() {
        // `>[-]` then a loop: the run is `[p+1] = 0; p += 1`.
        let mut program = program_of(vec![
            run_node(vec![EffKind::Set { at: 1, value: 0 }], 1),
            loop_node(vec![run_node(vec![add(0, -1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(print::print(&program), "[p+1] = 0\np += 1\n");
    }

    #[test]
    fn a_shell_survives_when_the_body_clears_a_different_cell() {
        // Runs at most once, but the body touches `[p+1]` and moves the
        // pointer, which the original does not when `[p]` is 0.
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            loop_node(vec![run_node(vec![EffKind::Set { at: 1, value: 0 }], 1)]),
        ]);
        let dialect = Dialect::default();
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }

    #[test]
    fn a_loop_after_a_scan_dies() {
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            Node::new(NodeKind::Scan(Scan { stride: 1 }), Span::SYNTHETIC),
            loop_node(vec![run_node(vec![add(0, -1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(print::print(&program), "[p] += 1\nscan p += 1\n");
    }

    #[test]
    fn removal_merges_the_neighboring_runs() {
        let mut program = program_of(vec![
            run_node(vec![EffKind::Set { at: 0, value: 0 }], 0),
            loop_node(vec![run_node(vec![add(1, 1)], 0)]),
            run_node(vec![add(0, 1)], 0),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(program.body.len(), 1);
        assert_eq!(print::print(&program), "[p] = 0\n[p] += 1\n");
    }

    #[test]
    fn a_chain_of_dead_loops_collapses() {
        let (program, changed) = swept("[a][b][c]");
        assert!(changed);
        assert_eq!(print::print(&program), "");
    }

    #[test]
    fn is_idempotent_and_honest() {
        let (program, changed) = swept("+[-]");
        assert!(!changed);
        let dialect = Dialect::default();
        let mut program = program;
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }
}

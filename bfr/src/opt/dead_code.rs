//! Deletes loops that can never run, and loop tests that repeat a test
//! already made.
//!
//! A loop can never run where its control cell must be zero: at program
//! start, or right after a node that [`Node::exits_on_zero`].
//!
//! A shell's test is redundant when its body is a single loop or scan,
//! which tests the same cell itself, or a single `[0] = 0`, which does
//! nothing when the cell is already zero. The body is lifted out in the
//! shell's place.
//!
//! Examples
//!
//! `loop { [0] += -1 }; [0] += 1` at program start becomes `[0] += 1`.
//! `loop { [0] += -1 }; loop { [+1] += 1 }` becomes `loop { [0] += -1 }` (a loop only exits at zero).
//! `[0] = 0; loop { [+1] += 1 }` becomes `[0] = 0`.
//! `loop { loop { [0] += -1 } }` becomes `loop { [0] += -1 }`.
//! `loop { [0] = 0 }` becomes `[0] = 0`.

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

/// Lift the body out of a shell whose only body node makes the shell's
/// own test redundant.
fn unwrap_shell(l: &Loop, span: Span) -> Option<Vec<Node>> {
    let [inner] = l.body.nodes() else {
        return None;
    };
    if !inner.exits_on_zero() {
        return None;
    }
    let self_guarded = match &inner.kind {
        // Tests the control cell itself, so the shell's test is a repeat
        NodeKind::Loop(_) | NodeKind::Scan(_) => true,
        // A single `[0] = 0` touches only the control cell. When that
        // cell is already zero the store changes nothing, and the shell's
        // test has already accessed it, so lifting it out is safe.
        NodeKind::Run(run) => run.shift == 0 && run.effects.len() == 1,
    };
    if !self_guarded {
        return None;
    }
    let mut node = inner.clone();
    node.span = span.merge(node.span);
    Some(vec![node])
}

/// Remove loops that can never run, restarting the search after each
/// removal since splicing merges neighboring runs. `zero_on_entry` is
/// whether the cell under the pointer is known to be zero when the block
/// is entered.
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
    use crate::ir::test_support::*;
    use crate::ir::{lower, print};
    use crate::parser;

    fn swept(src: &str) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = DeadCode.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    #[test]
    fn comment_loops_at_program_start_die() {
        let (program, changed) = swept("[foo.bar][baz]+");
        assert!(changed);
        assert_eq!(print::print(&program), "[0] += 1\n");
    }

    #[test]
    fn a_loop_after_a_loop_dies() {
        let (program, changed) = swept("+[-][+]");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[0] += 1\nloop {\n  [0] += -1\n}\n"
        );
    }

    #[test]
    fn a_loop_at_a_loop_body_start_survives() {
        let (program, changed) = swept("+[[-]+]");
        assert!(!changed);
        let expected = [
            "[0] += 1\n",
            "loop {\n",
            "  loop {\n",
            "    [0] += -1\n",
            "  }\n",
            "  [0] += 1\n",
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
            "[0] += 1\nloop {\n  [0] += -1\n}\n"
        );
    }

    #[test]
    fn nested_shells_unwrap_in_one_run() {
        let (program, changed) = swept("+[[[>]]]");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[0] += 1\nloop {\n  shift +1\n}\n"
        );
    }

    #[test]
    fn a_shell_around_a_clearing_store_collapses() {
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            loop_node(vec![run_node(vec![store(0, 0)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(program.body.len(), 1);
        assert_eq!(print::print(&program), "[0] += 1\n[0] = 0\n");
    }

    #[test]
    fn a_shell_survives_when_the_body_touches_another_cell() {
        // Runs at most once, but straight-lining would touch `[+1]`
        // when the original never does.
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            loop_node(vec![run_node(
                vec![add(1, 1), store(0, 0)],
                0,
            )]),
        ]);
        let dialect = Dialect::default();
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }

    #[test]
    fn the_unwrapped_node_keeps_the_shell_span() {
        let (program, _) = swept("+[[-]]");
        // `[[-]]` spans bytes 1..6, and the surviving loop covers all of it.
        assert_eq!(program.body.nodes()[1].span, Span::new(1, 6));
    }

    #[test]
    fn a_loop_after_a_clearing_store_dies() {
        let mut program = program_of(vec![
            run_node(vec![store(0, 0)], 0),
            loop_node(vec![run_node(vec![add(1, 1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(print::print(&program), "[0] = 0\n");
    }

    #[test]
    fn a_shift_off_the_cleared_cell_keeps_the_loop() {
        let mut program = program_of(vec![
            run_node(vec![store(0, 0)], 1),
            loop_node(vec![run_node(vec![add(0, -1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }

    #[test]
    fn a_loop_after_a_clear_of_the_landing_cell_dies() {
        // `>[-]` then a loop: the run is `[+1] = 0; shift +1`.
        let mut program = program_of(vec![
            run_node(vec![store(1, 0)], 1),
            loop_node(vec![run_node(vec![add(0, -1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(print::print(&program), "[+1] = 0\nshift +1\n");
    }

    #[test]
    fn a_shell_survives_when_the_body_clears_a_different_cell() {
        // Runs at most once, but the body touches `[+1]` and moves the
        // pointer, which the original does not when `[0]` is 0.
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            loop_node(vec![run_node(vec![store(1, 0)], 1)]),
        ]);
        let dialect = Dialect::default();
        assert!(!DeadCode.run(&mut program, &Ctx::new(&dialect)));
    }

    #[test]
    fn a_loop_after_a_scan_dies() {
        let mut program = program_of(vec![
            run_node(vec![add(0, 1)], 0),
            scan_node(1),
            loop_node(vec![run_node(vec![add(0, -1)], 0)]),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(print::print(&program), "[0] += 1\nscan +1\n");
    }

    #[test]
    fn removal_merges_the_neighboring_runs() {
        let mut program = program_of(vec![
            run_node(vec![store(0, 0)], 0),
            loop_node(vec![run_node(vec![add(1, 1)], 0)]),
            run_node(vec![add(0, 1)], 0),
        ]);
        let dialect = Dialect::default();
        assert!(DeadCode.run(&mut program, &Ctx::new(&dialect)));
        assert_eq!(program.body.len(), 1);
        assert_eq!(print::print(&program), "[0] = 0\n[0] += 1\n");
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

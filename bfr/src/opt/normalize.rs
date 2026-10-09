//! Merges adjacent runs, and within each run composes effects on the same
//! cell. See [`crate::ir::Block::normalize`] for the exact rules.
//!
//! Examples
//!
//! `+++` becomes `[p] += 3`.
//! `>>>>>` becomes `p += 5`.

use crate::ir::Program;
use crate::opt::{Changed, Ctx, Pass};

pub struct Normalize;

impl Pass for Normalize {
    fn name(&self) -> &'static str {
        "Normalize"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        let mut changed = false;
        program.for_each_block_mut(&mut |block, _| {
            changed |= block.normalize(ctx.dialect);
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Dialect;
    use crate::ir::{lower, print};
    use crate::parser;

    fn normalized(src: &str) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = Normalize.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    #[test]
    fn fuses_a_string_of_increments() {
        let (program, changed) = normalized("+++");
        assert!(changed);
        assert_eq!(print::print(&program), "[p] += 3\n");
    }

    #[test]
    fn signs_fold_together() {
        let (program, _) = normalized("++---");
        assert_eq!(print::print(&program), "[p] -= 1\n");
    }

    #[test]
    fn cancelling_adds_leave_nothing() {
        let (program, changed) = normalized("+-");
        assert!(changed);
        assert_eq!(print::print(&program), "");
    }

    #[test]
    fn composes_past_adds_to_other_cells() {
        let (program, _) = normalized("+>+<+");
        assert_eq!(print::print(&program), "[p] += 2\n[p+1] += 1\n");
    }

    #[test]
    fn declines_composition_across_an_observer() {
        let (program, changed) = normalized("+.+");
        assert!(!changed);
        assert_eq!(print::print(&program), "[p] += 1\nwrite([p])\n[p] += 1\n");
    }

    #[test]
    fn normalizes_loop_bodies() {
        let (program, changed) = normalized("+++[>++++<-]>.");
        assert!(changed);
        let expected = [
            "[p] += 3\n",
            "while [p] {\n",
            "  [p+1] += 4\n",
            "  [p] -= 1\n",
            "}\n",
            "write([p+1])\n",
            "p += 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn reports_a_change_made_only_in_a_nested_block() {
        let (program, changed) = normalized("+[++]");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] += 1\nwhile [p] {\n  [p] += 2\n}\n"
        );
    }

    #[test]
    fn a_canonical_program_reports_no_change() {
        let (_, changed) = normalized("+[>.]");
        assert!(!changed);
    }

    #[test]
    fn is_idempotent() {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let mut program = lower::lower(&parser::parse("+++[>++++<-]>.").expect("parses"));
        assert!(Normalize.run(&mut program, &ctx));
        let settled = program.clone();
        assert!(!Normalize.run(&mut program, &ctx));
        assert_eq!(program, settled);
    }
}

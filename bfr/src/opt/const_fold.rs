//! Tracks which cells hold known values, starting from the zeroed tape,
//! and uses that knowledge in two ways. Arithmetic on a known cell becomes
//! a store. A loop whose control cell is known is settled: a known zero
//! deletes the loop, and a known nonzero replaces it with one trip of its
//! body, when that body is a single run that zeroes the control cell.
//!
//! Examples
//!
//! `[p] += 3` at program start becomes `[p] = 3`.
//! `[p] = 3; while [p] { [p+1] += [p] * 4; [p] = 0 }` becomes `[p] = 3; [p+1] = 12; [p] = 0`.

use std::collections::HashMap;

use crate::config::Dialect;
use crate::ir::{Block, BlockSite, Cell, EffKind, Loop, Node, NodeKind, Program, Run, arith};
use crate::opt::{Changed, Ctx, Pass};

pub struct ConstFold;

impl Pass for ConstFold {
    fn name(&self) -> &'static str {
        "ConstFold"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        let mut changed = false;
        program.for_each_block_mut(&mut |block, site| {
            changed |= fold(block, site, ctx.dialect);
        });
        changed
    }
}

/// What is known about one cell's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Value(Cell),
    Unknown,
}

/// What the tape holds at one point in a block. `cells` is keyed by offset
/// from the pointer at block entry, and `origin` is the pointer's current
/// offset from there, so a shift is one addition. A cell gets an entry once
/// something has accessed it. Every other cell holds `rest`.
struct State {
    cells: HashMap<isize, Known>,
    rest: Known,
    origin: isize,
}

impl State {
    fn on_entry(site: BlockSite) -> State {
        let rest = match site {
            BlockSite::Program => Known::Value(0),
            BlockSite::LoopBody => Known::Unknown,
        };
        // Whatever entered the block read the cell under the pointer.
        State {
            cells: HashMap::from([(0, rest)]),
            rest,
            origin: 0,
        }
    }

    fn get(&self, at: isize) -> Known {
        self.cells
            .get(&(self.origin + at))
            .copied()
            .unwrap_or(self.rest)
    }

    fn set(&mut self, at: isize, value: Known) {
        self.cells.insert(self.origin + at, value);
    }

    /// Record an access that leaves the cell as it was.
    fn touch(&mut self, at: isize) {
        let value = self.get(at);
        self.cells.entry(self.origin + at).or_insert(value);
    }

    /// Whether an access has proven the cell is on the tape.
    fn accessed(&self, at: isize) -> bool {
        self.cells.contains_key(&(self.origin + at))
    }

    fn forget_all(&mut self) {
        self.cells.clear();
        self.rest = Known::Unknown;
    }

    /// Move the pointer.
    fn shift(&mut self, by: isize) {
        self.origin += by;
    }

    /// Apply one effect to the state. When its inputs are known, return
    /// the store or plain add it amounts to.
    fn step(&mut self, kind: &EffKind, dialect: &Dialect) -> Option<EffKind> {
        match *kind {
            EffKind::Add { at, delta } => match self.get(at) {
                Known::Value(v) => {
                    let value = arith::apply_delta(v, delta, dialect);
                    self.set(at, Known::Value(value));
                    Some(EffKind::Store { at, value })
                }
                Known::Unknown => {
                    self.set(at, Known::Unknown);
                    None
                }
            },
            EffKind::Store { at, value } => {
                self.set(at, Known::Value(value));
                None
            }
            EffKind::AddScaled { at, from, factor } => {
                self.touch(from);
                match (self.get(at), self.get(from)) {
                    (Known::Value(a), Known::Value(f)) => {
                        let delta = arith::scaled_delta(f, factor, dialect);
                        let value = arith::apply_delta(a, delta, dialect);
                        self.set(at, Known::Value(value));
                        Some(EffKind::Store { at, value })
                    }
                    (Known::Unknown, Known::Value(f)) => {
                        self.set(at, Known::Unknown);
                        let delta = arith::scaled_delta(f, factor, dialect);
                        Some(EffKind::Add { at, delta })
                    }
                    (_, Known::Unknown) => {
                        self.set(at, Known::Unknown);
                        None
                    }
                }
            }
            EffKind::Read { at } => {
                self.set(at, Known::Unknown);
                None
            }
            EffKind::Write { at } => {
                self.touch(at);
                None
            }
        }
    }
}

/// Fold one block from its entry state.
fn fold(block: &mut Block, site: BlockSite, dialect: &Dialect) -> bool {
    let mut state = State::on_entry(site);
    let mut changed = false;
    // Splicing merges neighboring runs, so replacements wait until the
    // walk is done.
    let mut edits: Vec<(usize, Option<Node>)> = Vec::new();
    for (i, node) in block.iter_mut().enumerate() {
        let span = node.span;
        match &mut node.kind {
            NodeKind::Run(run) => changed |= fold_run(run, &mut state, dialect),
            NodeKind::Scan(_) => {
                if state.get(0) == Known::Value(0) {
                    // The test can only go once an access has proven the
                    // cell is on the tape, since the test itself could fault.
                    if state.accessed(0) {
                        edits.push((i, None));
                    }
                    state.touch(0);
                    continue;
                }
                state.forget_all();
                state.set(0, Known::Value(0));
            }
            NodeKind::Loop(l) => {
                match state.get(0) {
                    Known::Value(0) => {
                        if state.accessed(0) {
                            edits.push((i, None));
                        }
                        state.touch(0);
                        continue;
                    }
                    Known::Value(_) => {
                        if let Some(body) = single_trip(l) {
                            let mut body = body.clone();
                            body.span = span.merge(body.span);
                            if let NodeKind::Run(run) = &mut body.kind {
                                fold_run(run, &mut state, dialect);
                            }
                            edits.push((i, Some(body)));
                            continue;
                        }
                    }
                    Known::Unknown => {}
                }
                // The loop stays. Afterwards every cell it can write is
                // unknown and its control cell is zero.
                match l.footprint() {
                    Some(footprint) => {
                        for at in footprint.writes {
                            state.set(at, Known::Unknown);
                        }
                    }
                    None => state.forget_all(),
                }
                state.set(0, Known::Value(0));
            }
        }
    }
    for (i, replacement) in edits.into_iter().rev() {
        block.splice(i..i + 1, replacement.into_iter().collect());
        changed = true;
    }
    changed
}

/// Fold the run's effects, advancing `state` past the run.
fn fold_run(run: &mut Run, state: &mut State, dialect: &Dialect) -> bool {
    let mut changed = false;
    for eff in &mut run.effects {
        if let Some(kind) = state.step(&eff.kind, dialect) {
            eff.kind = kind;
            changed = true;
        }
    }
    state.shift(run.shift);
    changed
}

/// The body of a shell, which a nonzero control cell runs exactly once.
fn single_trip(l: &Loop) -> Option<&Node> {
    match l.body.nodes() {
        [node] if matches!(node.kind, NodeKind::Run(_)) && node.exits_on_zero() => Some(node),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CellWidth, Config, Dialect, OptLevel};
    use crate::error::Span;
    use crate::ir::{Eff, EffKind, Loop, Node, NodeKind, Run, Scan, print};

    fn program_of(nodes: Vec<Node>) -> Program {
        Program::new(Block::from_nodes(nodes))
    }

    fn loop_node(body_nodes: Vec<Node>) -> Node {
        let body = Block::from_nodes(body_nodes);
        Node::new(NodeKind::Loop(Loop { body }), Span::SYNTHETIC)
    }

    fn scan_node(stride: isize) -> Node {
        Node::new(NodeKind::Scan(Scan { stride }), Span::SYNTHETIC)
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

    fn store(at: isize, value: u32) -> EffKind {
        EffKind::Store { at, value }
    }

    fn scaled(at: isize, from: isize, factor: i32) -> EffKind {
        EffKind::AddScaled { at, from, factor }
    }

    fn read(at: isize) -> EffKind {
        EffKind::Read { at }
    }

    fn write(at: isize) -> EffKind {
        EffKind::Write { at }
    }

    fn folded_under(nodes: Vec<Node>, dialect: &Dialect) -> (Program, Changed) {
        let mut program = program_of(nodes);
        let changed = ConstFold.run(&mut program, &Ctx::new(dialect));
        (program, changed)
    }

    fn folded(nodes: Vec<Node>) -> (Program, Changed) {
        folded_under(nodes, &Dialect::default())
    }

    #[test]
    fn an_add_to_a_known_cell_becomes_a_store() {
        let (program, changed) = folded(vec![run_node(vec![add(0, 3), add(5, -1)], 0)]);
        assert!(changed);
        assert_eq!(print::print(&program), "[p] = 3\n[p+5] = 255\n");
    }

    #[test]
    fn a_store_seeds_later_folds() {
        let (program, _) = folded(vec![run_node(vec![read(0), store(1, 2), add(1, 3)], 0)]);
        assert_eq!(
            print::print(&program),
            "[p] = read()\n[p+1] = 2\n[p+1] = 5\n"
        );
    }

    #[test]
    fn input_makes_a_cell_unknown() {
        let (_, changed) = folded(vec![run_node(vec![read(0), add(0, 1)], 0)]);
        assert!(!changed);
    }

    #[test]
    fn offsets_follow_the_pointer() {
        let (program, _) = folded(vec![
            run_node(vec![store(1, 7)], 1),
            loop_node(vec![run_node(vec![read(1)], 0)]),
            run_node(vec![add(0, 1)], 0),
        ]);
        let expected = [
            "[p+1] = 7\n",
            "p += 1\n",
            "while [p] {\n",
            "  [p+1] = read()\n",
            "}\n",
            "[p] = 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_loop_body_starts_unknown() {
        let (_, changed) = folded(vec![
            run_node(vec![read(0)], 0),
            loop_node(vec![run_node(vec![add(1, 1)], 0)]),
        ]);
        assert!(!changed);
        let (program, changed) = folded(vec![
            run_node(vec![read(0)], 0),
            loop_node(vec![run_node(vec![store(1, 2), add(1, 3)], 0)]),
        ]);
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] = read()\nwhile [p] {\n  [p+1] = 2\n  [p+1] = 5\n}\n"
        );
    }

    #[test]
    fn a_scaled_add_of_known_cells_becomes_a_store() {
        let (program, _) = folded(vec![run_node(vec![store(0, 3), scaled(1, 0, 4)], 0)]);
        assert_eq!(print::print(&program), "[p] = 3\n[p+1] = 12\n");
    }

    #[test]
    fn a_scaled_add_from_a_known_cell_becomes_a_plain_add() {
        let (program, _) = folded(vec![
            run_node(vec![read(0)], 0),
            loop_node(vec![run_node(vec![store(0, 3), scaled(1, 0, 4)], 0)]),
        ]);
        assert_eq!(
            print::print(&program),
            "[p] = read()\nwhile [p] {\n  [p] = 3\n  [p+1] += 12\n}\n"
        );
    }

    #[test]
    fn a_scaled_add_from_an_unknown_cell_stays() {
        let (_, changed) = folded(vec![
            run_node(vec![read(0)], 0),
            loop_node(vec![run_node(vec![store(1, 3), scaled(1, 0, 4)], 0)]),
        ]);
        assert!(!changed);
    }

    #[test]
    fn a_known_nonzero_control_runs_the_body_once() {
        let (program, changed) = folded(vec![
            run_node(vec![store(0, 3)], 0),
            loop_node(vec![run_node(vec![scaled(1, 0, 4), store(0, 0)], 0)]),
        ]);
        assert!(changed);
        assert_eq!(program.body.len(), 1);
        assert_eq!(print::print(&program), "[p] = 3\n[p+1] = 12\n[p] = 0\n");
    }

    #[test]
    fn a_single_trip_may_move_the_pointer() {
        let (program, _) = folded(vec![
            run_node(vec![store(0, 3)], 0),
            loop_node(vec![run_node(vec![store(1, 0), write(0)], 1)]),
            run_node(vec![add(0, 1)], 0),
        ]);
        // The trip and the run after it merge, so the trailing add is
        // addressed from before the shift.
        assert_eq!(
            print::print(&program),
            "[p] = 3\n[p+1] = 0\nwrite([p])\n[p+1] = 1\np += 1\n"
        );
    }

    #[test]
    fn the_single_trip_keeps_the_loop_span() {
        let mut body = run_node(vec![store(0, 0)], 0);
        body.span = Span::new(2, 3);
        let mut l = loop_node(vec![body]);
        l.span = Span::new(1, 4);
        let (program, _) = folded(vec![run_node(vec![store(0, 3)], 0), l]);
        assert_eq!(program.body.nodes()[0].span, Span::new(1, 4));
    }

    #[test]
    fn a_body_that_leaves_its_control_cell_alive_stays() {
        let (program, changed) = folded(vec![
            run_node(vec![store(0, 3)], 0),
            loop_node(vec![run_node(vec![add(1, 1), add(0, -1)], 0)]),
            run_node(vec![add(1, 1), add(0, 1)], 0),
        ]);
        assert!(changed);
        let expected = [
            "[p] = 3\n",
            "while [p] {\n",
            "  [p+1] += 1\n",
            "  [p] -= 1\n",
            "}\n",
            "[p+1] += 1\n",
            "[p] = 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_known_zero_control_deletes_the_loop() {
        let (program, changed) = folded(vec![
            loop_node(vec![run_node(vec![write(0)], 0)]),
            run_node(vec![store(1, 0), add(0, 1)], 1),
            loop_node(vec![run_node(vec![write(0)], 0)]),
        ]);
        assert!(changed);
        assert_eq!(print::print(&program), "[p+1] = 0\n[p] = 1\np += 1\n");
    }

    #[test]
    fn a_zero_cell_nothing_has_touched_keeps_its_loop() {
        // The test may still fault at the tape edge.
        let (_, changed) = folded(vec![
            run_node(vec![], -1),
            loop_node(vec![run_node(vec![write(0)], 0)]),
        ]);
        assert!(!changed);
    }

    #[test]
    fn a_zero_trip_loop_leaves_the_state_alone() {
        // The pointer lands on an untouched cell, so the loop stays but
        // nothing it would write is forgotten.
        let (program, _) = folded(vec![
            run_node(vec![store(1, 4)], 2),
            loop_node(vec![run_node(vec![add(0, 1), add(-1, 1)], 0)]),
            run_node(vec![add(0, 1), add(-1, 1)], 0),
        ]);
        let expected = [
            "[p+1] = 4\n",
            "p += 2\n",
            "while [p] {\n",
            "  [p] += 1\n",
            "  [p-1] += 1\n",
            "}\n",
            "[p] = 1\n",
            "[p-1] = 5\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_zero_trip_scan_is_deleted() {
        let (program, changed) = folded(vec![scan_node(1)]);
        assert!(changed);
        assert!(program.body.is_empty());
        let (_, changed) = folded(vec![run_node(vec![], 1), scan_node(1)]);
        assert!(!changed);
    }

    #[test]
    fn a_scan_forgets_all_but_its_landing_cell() {
        let (program, _) = folded(vec![
            run_node(vec![store(3, 1), read(0)], 0),
            scan_node(1),
            run_node(vec![add(0, 1), add(3, 1)], 0),
        ]);
        let expected = [
            "[p+3] = 1\n",
            "[p] = read()\n",
            "scan p += 1\n",
            "[p] = 1\n",
            "[p+3] += 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_kept_loop_forgets_what_it_may_write() {
        let (program, _) = folded(vec![
            run_node(vec![store(1, 7), store(2, 7), read(0)], 0),
            loop_node(vec![run_node(vec![add(1, 1), add(0, -1)], 0)]),
            run_node(vec![add(0, 1), add(1, 1), add(2, 1)], 0),
        ]);
        let expected = [
            "[p+1] = 7\n",
            "[p+2] = 7\n",
            "[p] = read()\n",
            "while [p] {\n",
            "  [p+1] += 1\n",
            "  [p] -= 1\n",
            "}\n",
            "[p] = 1\n",
            "[p+1] += 1\n",
            "[p+2] = 8\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_loop_that_moves_the_pointer_forgets_everything() {
        let (program, _) = folded(vec![
            run_node(vec![store(2, 7), read(0)], 0),
            loop_node(vec![run_node(vec![], 1)]),
            run_node(vec![add(0, 1), add(2, 1)], 0),
        ]);
        let expected = [
            "[p+2] = 7\n",
            "[p] = read()\n",
            "while [p] {\n",
            "  p += 1\n",
            "}\n",
            "[p] = 1\n",
            "[p+2] += 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn knowledge_rebuilds_after_the_pointer_is_lost() {
        let (program, _) = folded(vec![
            run_node(vec![store(0, 4), read(0)], 0),
            scan_node(1),
            run_node(vec![store(1, 5)], 2),
            loop_node(vec![run_node(vec![read(2)], 0)]),
            run_node(vec![add(0, 1), add(-1, 1), add(-2, 1)], 0),
        ]);
        // `[p-1]` is the 5 stored before the shift, `[p-2]` the cell the
        // scan stopped on.
        let expected = [
            "[p] = 4\n",
            "[p] = read()\n",
            "scan p += 1\n",
            "[p+1] = 5\n",
            "p += 2\n",
            "while [p] {\n",
            "  [p+2] = read()\n",
            "}\n",
            "[p] = 1\n",
            "[p-1] = 6\n",
            "[p-2] = 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn stores_wrap_at_the_cell_width() {
        let nodes = || vec![run_node(vec![store(0, 255), add(0, 1)], 0)];
        let (program, _) = folded(nodes());
        assert_eq!(print::print(&program), "[p] = 255\n[p] = 0\n");
        let u16 = Dialect {
            cell_width: CellWidth::U16,
            ..Dialect::default()
        };
        let (program, _) = folded_under(nodes(), &u16);
        assert_eq!(print::print(&program), "[p] = 255\n[p] = 256\n");
    }

    #[test]
    fn is_idempotent_and_honest() {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let mut program = program_of(vec![
            run_node(vec![add(0, 3)], 0),
            loop_node(vec![run_node(vec![scaled(1, 0, 4), store(0, 0)], 0)]),
            run_node(vec![read(0)], 0),
            loop_node(vec![run_node(vec![scaled(1, 0, 4), store(0, 0)], 0)]),
            run_node(vec![add(0, 1), write(1)], 1),
        ]);
        assert!(ConstFold.run(&mut program, &ctx));
        let settled = program.clone();
        assert!(!ConstFold.run(&mut program, &ctx));
        assert_eq!(program, settled);
    }

    #[test]
    fn the_worked_example_reaches_its_documented_form() {
        let config = Config::new(OptLevel::O2);
        let (program, _) = crate::compile_to_ir("+++[>++++<-]>.", &config).expect("parses");
        assert_eq!(
            print::print(&program),
            "[p] = 0\n[p+1] = 12\nwrite([p+1])\np += 1\n"
        );
    }

    #[test]
    fn output_matches_o0_at_every_width() {
        // Known drains, a transfer of a cleared cell, a fan-out, unknown
        // control from input, and a wrap that only a loop test can see.
        for src in [
            "+++[>++++<-]>.",
            ">[->+<]<.",
            "++[->+>+<<]>.>.",
            ",[->+<]>.",
            "++++++[--->+<]>.",
            ">>++<<[>>+<<-]>>.",
        ] {
            for cell_width in [CellWidth::U8, CellWidth::U16, CellWidth::U32] {
                let dialect = Dialect {
                    cell_width,
                    ..Dialect::default()
                };
                let o0 = Config::new(OptLevel::O0).with_dialect(dialect);
                let o2 = Config::new(OptLevel::O2).with_dialect(dialect);
                assert_eq!(
                    crate::run_to_vec(src, &o2, b"\x05"),
                    crate::run_to_vec(src, &o0, b"\x05"),
                    "mismatch for {src} at {cell_width:?}"
                );
            }
        }
    }

    #[test]
    fn a_wrapping_drain_matches_o0() {
        // 255 steps down by 5, wrapping through 0 to land there after 51 trips.
        let src = "-[>+<-----]>.";
        assert_eq!(
            crate::run_to_vec(src, &Config::new(OptLevel::O2), b""),
            crate::run_to_vec(src, &Config::new(OptLevel::O0), b"")
        );
    }

    #[test]
    fn faults_match_o0() {
        for src in ["<[-]", "<[+.]", "+[<]", "<+"] {
            let o0 = crate::run_to_vec(src, &Config::new(OptLevel::O0), b"").expect_err("faults");
            let o2 = crate::run_to_vec(src, &Config::new(OptLevel::O2), b"").expect_err("faults");
            assert_eq!(o2, o0, "mismatch for {src}");
        }
    }
}

//! Runs the program at compile time, tracking what every cell holds. An
//! effect whose inputs are known is applied to that tracked state. An effect
//! that depends on an unknown value, such as the byte a read produces, is
//! kept and leaves its cell unknown. Evaluation stops only when it can no
//! longer continue, such as at a loop whose control cell is unknown, or when
//! the fuel or output budget runs out.
//!
//! The residual program is built as evaluation goes. Known cell values are
//! written into it only when something kept needs them, or at the end so
//! the final tape matches. Once evaluation stops, the rest of the program
//! is copied through unchanged.
//!
//! Examples
//!
//! `[p] += 3; while [p] { [p+1] += 2; [p] -= 1 }; [p] = read()` becomes `[p] = read(); [p+1] = 6`.
//! `hello.b` becomes `[p] = 72; write([p]); [p] = 101; write([p]); ...`.

use std::collections::HashMap;

use crate::config::Dialect;
use crate::error::Span;
use crate::ir::build::Builder;
use crate::ir::{BlockSite, Cell, Eff, EffKind, Loop, Node, NodeKind, Program, Run, Scan, arith};
use crate::opt::state::{Known, State};
use crate::opt::{Changed, Ctx, Pass};

pub struct PartialEval {
    /// Maximum steps to execute at compile time.
    pub fuel: u64,
    /// Maximum bytes of compile-time output to keep.
    pub max_output: usize,
}

impl Default for PartialEval {
    fn default() -> Self {
        PartialEval {
            fuel: 10_000_000,
            max_output: 1 << 16,
        }
    }
}

impl Pass for PartialEval {
    fn name(&self) -> &'static str {
        "PartialEval"
    }

    fn run(&self, program: &mut Program, ctx: &Ctx<'_>) -> Changed {
        let residual = Eval::new(self, ctx.dialect).run(program);
        if residual.body == program.body {
            return false;
        }
        program.body = residual.body;
        true
    }
}

/// One level of the walk: a node sequence and how far along it is. A trip
/// through a loop body remembers the loop, so evaluation can leave the rest
/// of the loop in the residual if it has to stop partway through.
struct Frame<'a> {
    nodes: &'a [Node],
    next: usize,
    trip_of: Option<&'a Node>,
}

/// The evaluator. `state` is what the real tape holds, and `residual` is
/// what the residual program's tape holds at the same point. Where they
/// differ, the known value has yet to be written into the residual.
struct Eval<'a> {
    dialect: &'a Dialect,
    state: State,
    residual: HashMap<isize, Known>,
    /// For each cell the residual is behind on, the merged span of the
    /// effects that produced the known value.
    pending_spans: HashMap<isize, Span>,
    builder: Builder,
    fuel: u64,
    output_left: usize,
    stack: Vec<Frame<'a>>,
}

/// Why evaluation cannot go on.
enum Stop {
    /// The next step needs something only the running program has.
    Opaque,
    /// The next step would fault, so the program has to fault there too.
    Fault,
}

impl<'a> Eval<'a> {
    fn new(pass: &PartialEval, dialect: &'a Dialect) -> Eval<'a> {
        Eval {
            dialect,
            state: State::on_entry(BlockSite::Program),
            residual: HashMap::new(),
            pending_spans: HashMap::new(),
            builder: Builder::new(),
            fuel: pass.fuel,
            output_left: pass.max_output,
            stack: Vec::new(),
        }
    }

    /// Evaluate `program` and build its residual.
    ///
    /// Whatever stops evaluation has already put itself into the residual,
    /// so [`Eval::finish`] only has to copy what comes after it.
    fn run(mut self, program: &'a Program) -> Program {
        self.stack.push(Frame {
            nodes: program.body.nodes(),
            next: 0,
            trip_of: None,
        });
        loop {
            let frame = self.top();
            if frame.next == frame.nodes.len() {
                if frame.trip_of.is_none() {
                    break;
                }
                // The trip is over, so test the control cell again.
                match self.test() {
                    Ok(Some(true)) => self.top().next = 0,
                    Ok(Some(false)) => {
                        self.stack.pop();
                    }
                    Ok(None) | Err(_) => break,
                }
                continue;
            }
            let node = &frame.nodes[frame.next];
            frame.next += 1;
            let outcome = match &node.kind {
                NodeKind::Run(run) => self.eval_run(run, node.span),
                NodeKind::Loop(l) => self.enter_loop(node, l),
                NodeKind::Scan(scan) => self.eval_scan(node, scan),
            };
            if outcome.is_err() {
                break;
            }
        }
        self.finish()
    }

    /// Flush everything the residual is behind on, copy the rest of the
    /// program through, and hand back the residual.
    fn finish(mut self) -> Program {
        self.flush_all();
        while let Some(frame) = self.stack.pop() {
            for node in &frame.nodes[frame.next..] {
                self.builder.push_node(node);
            }
            if let Some(l) = frame.trip_of {
                self.builder.push_node(l);
            }
        }
        self.builder.finish()
    }

    fn top(&mut self) -> &mut Frame<'a> {
        self.stack
            .last_mut()
            .expect("the program's frame is never popped")
    }

    /// Spend one unit of fuel, or stop if there is none left.
    fn burn(&mut self) -> Result<(), Stop> {
        if self.fuel == 0 {
            return Err(Stop::Opaque);
        }
        self.fuel -= 1;
        Ok(())
    }

    /// Stop at `node`, putting it into the residual whole.
    fn stop_at(&mut self, node: &Node, stop: Stop) -> Stop {
        self.flush_all();
        self.builder.push_node(node);
        stop
    }

    /// The cell at `at`, as an offset from the pointer at program entry.
    fn key(&self, at: isize) -> isize {
        self.state.origin() + at
    }

    /// Check that an access at `at` would not fault. The pointer's position
    /// is always known while evaluation runs, so this is exact.
    fn check(&self, at: isize) -> Result<(), Stop> {
        let index = self.dialect.origin as isize + self.key(at);
        if index >= 0 && (index as usize) < self.dialect.tape_cells {
            Ok(())
        } else {
            Err(Stop::Fault)
        }
    }

    /// Test the cell under the pointer, as a loop or scan does. The result
    /// is whether the cell is nonzero, or `None` when that is unknown.
    /// Testing a known value costs fuel.
    fn test(&mut self) -> Result<Option<bool>, Stop> {
        self.check(0)?;
        match self.state.get(0) {
            Known::Value(v) => {
                self.burn()?;
                Ok(Some(v != 0))
            }
            Known::Unknown => Ok(None),
        }
    }

    /// What the residual's tape holds at the cell `at`.
    fn residual_at(&self, at: isize) -> Known {
        self.residual
            .get(&self.key(at))
            .copied()
            .unwrap_or(Known::Value(0))
    }

    /// Record a known value for the cell at `at`, produced by an effect
    /// with `span`. The residual will need a store for it unless it already
    /// holds that value.
    fn learn(&mut self, at: isize, value: Cell, span: Span) {
        self.state.set(at, Known::Value(value));
        let key = self.key(at);
        if self.residual_at(at) == Known::Value(value) {
            self.pending_spans.remove(&key);
            return;
        }
        let merged = self.pending_spans.get(&key).copied().unwrap_or(Span::SYNTHETIC);
        self.pending_spans.insert(key, merged.merge(span));
    }

    /// Forget the cell at `at`. A kept effect or loop is about to write it,
    /// and the residual does the same.
    fn forget(&mut self, at: isize) {
        self.state.set(at, Known::Unknown);
        let key = self.key(at);
        self.residual.insert(key, Known::Unknown);
        self.pending_spans.remove(&key);
    }

    /// Bring the residual's copy of the cell at `at` up to date.
    fn flush(&mut self, at: isize) {
        let Known::Value(value) = self.state.get(at) else {
            return;
        };
        if self.residual_at(at) == Known::Value(value) {
            return;
        }
        let key = self.key(at);
        let span = self.pending_spans.remove(&key).unwrap_or(Span::SYNTHETIC);
        self.builder.push(EffKind::Store { at, value }, span);
        self.residual.insert(key, Known::Value(value));
    }

    /// Flush each of `cells`, lowest first so the output is stable.
    fn flush_sorted(&mut self, cells: impl IntoIterator<Item = isize>) {
        let mut cells: Vec<isize> = cells.into_iter().collect();
        cells.sort_unstable();
        for at in cells {
            self.flush(at);
        }
    }

    /// Bring the residual's copy of every cell up to date.
    fn flush_all(&mut self) {
        let origin = self.state.origin();
        let cells: Vec<isize> = self.state.known_cells().map(|(key, _)| key - origin).collect();
        self.flush_sorted(cells);
    }

    /// Put a kept effect into the residual. The cells it reads are flushed
    /// first so it sees the same values the real program does.
    fn keep(&mut self, kind: &EffKind, span: Span) {
        kind.reads(|at| self.flush(at));
        self.builder.push(kind.clone(), span);
        if let Some(at) = kind.writes() {
            self.forget(at);
        }
    }

    /// Evaluate the effects of `run` in order, then move the pointer. When
    /// an effect stops evaluation, it and the rest of the run go into the
    /// residual as they are.
    fn eval_run(&mut self, run: &Run, span: Span) -> Result<(), Stop> {
        for (i, eff) in run.effects.iter().enumerate() {
            if let Err(stop) = self.eval_effect(eff) {
                self.flush_all();
                for eff in &run.effects[i..] {
                    self.builder.push(eff.kind.clone(), eff.span);
                }
                self.builder.bump(run.shift, span);
                return Err(stop);
            }
        }
        self.builder.bump(run.shift, span);
        self.state.shift(run.shift);
        Ok(())
    }

    /// Evaluate one effect, or keep it when its inputs are unknown.
    fn eval_effect(&mut self, eff: &Eff) -> Result<(), Stop> {
        let kind = &eff.kind;
        let span = eff.span;
        match *kind {
            EffKind::Add { at, delta } => {
                self.check(at)?;
                match self.state.get(at) {
                    Known::Value(v) => {
                        self.burn()?;
                        self.learn(at, arith::apply_delta(v, delta, self.dialect), span);
                    }
                    Known::Unknown => self.keep(kind, span),
                }
            }
            EffKind::Store { at, value } => {
                self.check(at)?;
                self.burn()?;
                self.learn(at, value, span);
            }
            EffKind::AddScaled { at, from, factor } => {
                self.check(at)?;
                self.check(from)?;
                match (self.state.get(at), self.state.get(from)) {
                    (Known::Value(a), Known::Value(f)) => {
                        self.burn()?;
                        let delta = arith::scaled_delta(f, factor, self.dialect);
                        self.learn(at, arith::apply_delta(a, delta, self.dialect), span);
                    }
                    _ => self.keep(kind, span),
                }
            }
            EffKind::Read { at } => {
                self.check(at)?;
                self.keep(kind, span);
            }
            EffKind::Write { at } => {
                self.check(at)?;
                if let Known::Value(_) = self.state.get(at) {
                    if self.output_left == 0 {
                        return Err(Stop::Opaque);
                    }
                    self.output_left -= 1;
                }
                self.keep(kind, span);
            }
        }
        Ok(())
    }

    /// Test a loop's control cell on the way in. A known nonzero value
    /// starts a trip. A known zero skips the loop. An unknown value keeps
    /// the loop whole.
    fn enter_loop(&mut self, node: &'a Node, l: &'a Loop) -> Result<(), Stop> {
        match self.test() {
            Ok(Some(false)) => Ok(()),
            Ok(Some(true)) => {
                self.stack.push(Frame {
                    nodes: l.body.nodes(),
                    next: 0,
                    trip_of: Some(node),
                });
                Ok(())
            }
            Ok(None) => self.keep_loop(node, l),
            Err(stop) => Err(self.stop_at(node, stop)),
        }
    }

    /// Put a loop into the residual whole. The cells it can read are
    /// flushed first, the cells it can write are forgotten, and its control
    /// cell is zero afterwards on both tapes. A loop that could leave the
    /// pointer anywhere stops evaluation.
    fn keep_loop(&mut self, node: &Node, l: &Loop) -> Result<(), Stop> {
        let Some(footprint) = l.footprint() else {
            return Err(self.stop_at(node, Stop::Opaque));
        };
        self.flush_sorted(footprint.reads.iter().copied());
        self.builder.push_node(node);
        for at in footprint.writes {
            self.forget(at);
        }
        self.state.set(0, Known::Value(0));
        self.residual.insert(self.key(0), Known::Value(0));
        Ok(())
    }

    /// Walk a scan while the cells it lands on are known. On an unknown
    /// cell the scan goes into the residual from where the walk got to.
    fn eval_scan(&mut self, node: &'a Node, scan: &Scan) -> Result<(), Stop> {
        loop {
            match self.test() {
                Ok(Some(false)) => return Ok(()),
                Ok(Some(true)) => {
                    self.builder.bump(scan.stride, node.span);
                    self.state.shift(scan.stride);
                }
                Ok(None) => return Err(self.stop_at(node, Stop::Opaque)),
                Err(stop) => return Err(self.stop_at(node, stop)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CellWidth, Config, OptLevel};
    use crate::error::{Error, FaultCode, RuntimeError};
    use crate::ir::{lower, print};
    use crate::opt::test_support::{assert_faults_match_o0, assert_matches_o0};
    use crate::parser;

    const HELLO: &str = "++++++++[>++++[>++>+++>+++>+<<<<-]>+>+>->>+[<]<-]>>.>---.+++++++..+++.>>.<-.<.+++.------.--------.>>+.>++.";

    fn evaluated_by(src: &str, pass: &PartialEval) -> (Program, Changed) {
        let dialect = Dialect::default();
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        let changed = pass.run(&mut program, &Ctx::new(&dialect));
        (program, changed)
    }

    fn evaluated(src: &str) -> (Program, Changed) {
        evaluated_by(src, &PartialEval::default())
    }

    /// Lowering turns `[>]` into a loop, so run ScanLoop first to get a
    /// scan node for the evaluator to meet.
    fn evaluated_with_scans(src: &str) -> Program {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        crate::opt::scan::ScanLoop.run(&mut program, &ctx);
        PartialEval::default().run(&mut program, &ctx);
        program
    }

    #[test]
    fn the_docs_worked_example() {
        let (program, changed) = evaluated("+++[>++<-],");
        assert!(changed);
        assert_eq!(print::print(&program), "[p] = read()\n[p+1] = 6\n");
    }

    #[test]
    fn known_output_becomes_stores_and_writes() {
        let (program, changed) = evaluated("++.+.");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] = 2\nwrite([p])\n[p] = 3\nwrite([p])\n"
        );
    }

    #[test]
    fn hello_world_has_no_loops_left() {
        let (program, _) = evaluated(HELLO);
        let text = print::print(&program);
        assert!(!text.contains("while"), "{text}");
        assert_eq!(text.matches("write").count(), 13);
    }

    #[test]
    fn a_clean_cell_needs_no_store_before_a_write() {
        // `[p]` is 0 and the residual's tape starts at 0.
        let (program, changed) = evaluated(".");
        assert!(!changed);
        assert_eq!(print::print(&program), "write([p])\n");
    }

    #[test]
    fn a_read_leaves_its_cell_unknown_but_evaluation_continues() {
        let (program, changed) = evaluated(",>++<.");
        assert!(changed);
        assert_eq!(
            print::print(&program),
            "[p] = read()\nwrite([p])\n[p+1] = 2\n"
        );
    }

    #[test]
    fn a_read_flushes_its_cell_first() {
        // With EOF leaving the cell unchanged, the 5 has to be there.
        let (program, _) = evaluated("+++++,.");
        assert_eq!(
            print::print(&program),
            "[p] = 5\n[p] = read()\nwrite([p])\n"
        );
    }

    #[test]
    fn a_loop_with_a_read_inside_runs_its_trips() {
        let (program, _) = evaluated("+++[>,<-]");
        assert_eq!(
            print::print(&program),
            "[p+1] = read()\n[p+1] = read()\n[p+1] = read()\n"
        );
    }

    #[test]
    fn a_loop_with_an_unknown_control_cell_is_kept() {
        let (program, changed) = evaluated(",[-]");
        assert!(!changed);
        assert_eq!(print::print(&program), "[p] = read()\nwhile [p] {\n  [p] -= 1\n}\n");
    }

    #[test]
    fn a_kept_loop_gets_what_it_reads_and_forgets_what_it_writes() {
        let (program, _) = evaluated("+++>,[<->-]<.");
        // The 3 is only written once the loop needs it.
        let expected = [
            "[p+1] = read()\n",
            "[p] = 3\n",
            "p += 1\n",
            "while [p] {\n",
            "  [p-1] -= 1\n",
            "  [p] -= 1\n",
            "}\n",
            "write([p-1])\n",
            "p -= 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_kept_loop_inside_a_trip_lands_in_the_straight_line() {
        let (program, _) = evaluated("++[>,[-]<-]");
        let expected = [
            "[p+1] = read()\n",
            "p += 1\n",
            "while [p] {\n",
            "  [p] -= 1\n",
            "}\n",
            "[p] = read()\n",
            "while [p] {\n",
            "  [p] -= 1\n",
            "}\n",
            "p -= 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn stopping_mid_trip_leaves_the_rest_of_the_loop() {
        // The control cell becomes unknown after the read, so the trips so
        // far are straight-line and the loop continues from there.
        let (program, _) = evaluated("++[.,]");
        let expected = [
            "[p] = 2\n",
            "write([p])\n",
            "[p] = read()\n",
            "while [p] {\n",
            "  write([p])\n",
            "  [p] = read()\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_known_scan_walks_at_compile_time() {
        let program = evaluated_with_scans("+>+>+<<[>]+");
        assert_eq!(
            print::print(&program),
            "[p] = 1\n[p+1] = 1\n[p+2] = 1\n[p+3] = 1\np += 3\n"
        );
    }

    #[test]
    fn a_scan_onto_an_unknown_cell_continues_from_there() {
        let program = evaluated_with_scans("+>+>,<<[>]+");
        let expected = [
            "[p+2] = read()\n",
            "[p] = 1\n",
            "[p+1] = 1\n",
            "p += 2\n",
            "scan p += 1\n",
            "[p] += 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn a_fault_stops_evaluation_at_the_faulting_effect() {
        let (program, changed) = evaluated("+<+");
        assert!(changed);
        assert_eq!(print::print(&program), "[p] = 1\n[p-1] += 1\np -= 1\n");
        let (_, changed) = evaluated("<+");
        assert!(!changed);
    }

    #[test]
    fn a_loop_test_off_the_tape_is_kept_in_place() {
        let (program, _) = evaluated("+<[-]");
        assert_eq!(
            print::print(&program),
            "[p] = 1\np -= 1\nwhile [p] {\n  [p] -= 1\n}\n"
        );
    }

    #[test]
    fn running_out_of_fuel_leaves_the_loop_to_run_time() {
        let pass = PartialEval {
            fuel: 10,
            ..PartialEval::default()
        };
        let (program, _) = evaluated_by("+++++[>+<-]>.", &pass);
        // Five adds, the test, a trip of two effects, the test again, and
        // one more add spend the ten, so the second trip's decrement is
        // where evaluation stops.
        let expected = [
            "[p] = 4\n",
            "[p+1] = 2\n",
            "[p] -= 1\n",
            "while [p] {\n",
            "  [p+1] += 1\n",
            "  [p] -= 1\n",
            "}\n",
            "write([p+1])\n",
            "p += 1\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn an_infinite_loop_stops_at_the_fuel_limit() {
        let pass = PartialEval {
            fuel: 100,
            ..PartialEval::default()
        };
        let (program, _) = evaluated_by("+[]", &pass);
        assert_eq!(print::print(&program), "[p] = 1\nwhile [p] {\n}\n");
    }

    #[test]
    fn the_output_budget_stops_evaluation() {
        let pass = PartialEval {
            max_output: 2,
            ..PartialEval::default()
        };
        let (program, _) = evaluated_by("+++[.-]", &pass);
        let expected = [
            "[p] = 3\n",
            "write([p])\n",
            "[p] = 2\n",
            "write([p])\n",
            "[p] = 1\n",
            "write([p])\n",
            "[p] -= 1\n",
            "while [p] {\n",
            "  write([p])\n",
            "  [p] -= 1\n",
            "}\n",
        ];
        assert_eq!(print::print(&program), expected.concat());
    }

    #[test]
    fn stores_wrap_at_the_cell_width() {
        let src = "-.";
        let (program, _) = evaluated(src);
        assert_eq!(print::print(&program), "[p] = 255\nwrite([p])\n");
        let u16 = Dialect {
            cell_width: CellWidth::U16,
            ..Dialect::default()
        };
        let mut program = lower::lower(&parser::parse(src).expect("parses"));
        PartialEval::default().run(&mut program, &Ctx::new(&u16));
        assert_eq!(print::print(&program), "[p] = 65535\nwrite([p])\n");
    }

    #[test]
    fn the_pointer_starts_at_origin_for_bounds() {
        let dialect = Dialect {
            tape_cells: 4,
            origin: 2,
            ..Dialect::default()
        };
        let mut program = lower::lower(&parser::parse("<+<+<+").expect("parses"));
        PartialEval::default().run(&mut program, &Ctx::new(&dialect));
        // Cells 1 and 0 are on the tape, the third is not, and all three
        // adds share one run so the pointer only moves at the end.
        assert_eq!(
            print::print(&program),
            "[p-2] = 1\n[p-1] = 1\n[p-3] += 1\np -= 3\n"
        );
    }

    #[test]
    fn is_idempotent() {
        let dialect = Dialect::default();
        let ctx = Ctx::new(&dialect);
        let pass = PartialEval::default();
        for src in [HELLO, "+++[>++<-],", "++[.,]", "+>+>,<<[>]+", "+++[.-]"] {
            let mut program = lower::lower(&parser::parse(src).expect("parses"));
            pass.run(&mut program, &ctx);
            let settled = program.clone();
            assert!(!pass.run(&mut program, &ctx), "{src}");
            assert_eq!(program, settled, "{src}");
        }
    }

    fn o3(dialect: Dialect) -> Config {
        Config::new(OptLevel::O3).with_dialect(dialect)
    }

    #[test]
    fn o3_matches_o0_at_every_width() {
        let corpus: &[(&str, &[u8])] = &[
            ("+++[>++++<-]>.", b""),
            (HELLO, b""),
            (",[.,]", b"hello\n\0"),
            ("+++++[->+>+<<]>>[-<<+>>]<<.", b""),
            ("[this is a comment]+++.", b""),
            (">,[>,]<[.<]", b"abc\0"),
            ("++++[>++++<-]>[-]<[-]+.", b""),
            ("++++++[--->+<]>.", b""),
            ("++[--]+.", b""),
            (",.,.", b"x"),
            ("+++[>+++[>+<-]<-]>>.", b""),
            ("++[.,]", b"ab\0"),
            ("+>+>,<<[>]+.", b"\x07"),
            (",>++++[<.>-]", b"q"),
        ];
        for &(src, input) in corpus {
            assert_matches_o0(src, input, o3);
        }
    }

    #[test]
    fn o3_faults_match_o0() {
        for src in ["<+", "<[-]", "+[<]", "<[+.]", "+[>+]", "++[>.<-]<+"] {
            assert_faults_match_o0(src, o3);
        }
    }

    #[test]
    fn o3_still_runs_out_of_fuel_on_an_infinite_loop() {
        let config = Config::new(OptLevel::O3).with_fuel(10_000);
        assert_eq!(
            crate::run_to_vec("+[]", &config, b""),
            Err(Error::Runtime(RuntimeError {
                code: FaultCode::OutOfFuel,
                position: 0,
            }))
        );
    }
}

//! A resumable interpreter over [`crate::ir::Program`], driven by the caller.
//!
//! [`Session::new`] first flattens the program into a list of ops, where one
//! op is exactly one step. The session then executes ops as the caller asks
//! for them.

use std::collections::VecDeque;

use crate::config::{Config, Dialect, EofBehavior};
use crate::error::{FaultCode, RuntimeError};
use crate::ir::{Cell, CellDelta, EffKind, Node, NodeId, NodeKind, Program};
use crate::tape::Tape;

/// Why control returned to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The step budget was spent, but the program is still runnable.
    Yielded,
    /// Stopped before entering a node in the breakpoint set.
    Breakpoint,
    /// Parked at a `read` while the input queue is empty and input has not been closed.
    NeedInput,
    /// The program ran off the end of its last node.
    Done,
    /// Faulted and the session is finished. The tape remains inspectable.
    Fault(RuntimeError),
}

/// The node an op belongs to, and which effect of that node it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub node: NodeId,
    /// Index of the effect within its run. A run's closing shift is
    /// `effects.len()`. Loop tests and scan steps are always 0.
    pub effect: usize,
}

/// One step, in the flat form [`Session::new`] compiles to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add {
        at: isize,
        delta: CellDelta,
    },
    Store {
        at: isize,
        value: Cell,
    },
    AddScaled {
        at: isize,
        from: isize,
        factor: CellDelta,
    },
    Read {
        at: isize,
    },
    Write {
        at: isize,
    },
    /// A run's closing pointer move.
    Shift {
        by: isize,
    },
    /// If the current cell is zero, jump to `exit`.
    EnterLoop {
        exit: usize,
    },
    /// If the current cell is nonzero, jump back to `top`.
    BackEdge {
        top: usize,
    },
    /// Advance by `stride` until the current cell is zero.
    Scan {
        stride: isize,
    },
}

/// The flattened program.
struct Code {
    ops: Vec<Op>,
    source: Vec<Position>,
}

impl Code {
    fn new() -> Self {
        Code {
            ops: Vec::new(),
            source: Vec::new(),
        }
    }

    fn push(&mut self, op: Op, node: NodeId, effect: usize) {
        self.ops.push(op);
        self.source.push(Position { node, effect });
    }
}

/// Flatten the tree. Each loop becomes a test at its head and a test at
/// its tail, so every trip boundary is exactly one step.
fn flatten(program: &Program) -> Code {
    struct Frame<'a> {
        nodes: &'a [Node],
        next: usize,
        /// Index of the owning loop's `EnterLoop`, whose `exit` is patched
        /// when the body closes.
        enter: Option<usize>,
    }

    let mut code = Code::new();
    let mut stack = vec![Frame {
        nodes: program.body.nodes(),
        next: 0,
        enter: None,
    }];

    while let Some(frame) = stack.last_mut() {
        if frame.next == frame.nodes.len() {
            let enter = frame.enter;
            stack.pop();
            // Only the program body has no owning loop
            let Some(e) = enter else { continue };
            let id = code.source[e].node;
            // Jump to just past `EnterLoop`, since this op has already
            // tested the control cell
            code.push(Op::BackEdge { top: e + 1 }, id, 0);
            let after = code.ops.len();
            let Op::EnterLoop { exit } = &mut code.ops[e] else {
                unreachable!("`enter` indexes an EnterLoop");
            };
            // The loop's end is known now, so fill in the exit
            *exit = after;
            continue;
        }

        let nodes = frame.nodes;
        let node = &nodes[frame.next];
        frame.next += 1;
        match &node.kind {
            NodeKind::Run(run) => {
                for (i, eff) in run.effects.iter().enumerate() {
                    let op = match eff.kind {
                        EffKind::Add { at, delta } => Op::Add { at, delta },
                        EffKind::Store { at, value } => Op::Store { at, value },
                        EffKind::AddScaled { at, from, factor } => {
                            Op::AddScaled { at, from, factor }
                        }
                        EffKind::Read { at } => Op::Read { at },
                        EffKind::Write { at } => Op::Write { at },
                    };
                    code.push(op, node.id, i);
                }
                if run.shift != 0 {
                    code.push(Op::Shift { by: run.shift }, node.id, run.effects.len());
                }
            }
            NodeKind::Loop(l) => {
                code.push(Op::EnterLoop { exit: usize::MAX }, node.id, 0);
                let enter = Some(code.ops.len() - 1);
                stack.push(Frame {
                    nodes: l.body.nodes(),
                    next: 0,
                    enter,
                });
            }
            NodeKind::Scan(scan) => {
                code.push(
                    Op::Scan {
                        stride: scan.stride,
                    },
                    node.id,
                    0,
                );
            }
        }
    }
    code
}

/// A program and its execution state.
pub struct Session {
    /// Kept only for dumps and side tables. Execution uses `code`.
    program: Program,
    code: Code,
    pc: usize,
    ptr: isize,
    tape: Tape,
    dialect: Dialect,
    fuel: Option<u64>,
    steps: u64,
    input: VecDeque<u8>,
    input_closed: bool,
    output: Vec<u8>,
    fault: Option<RuntimeError>,
    /// Node of the most recently executed step, so breakpoints fire only
    /// when execution enters a node.
    last_node: Option<NodeId>,
    /// The op a breakpoint last stopped at, so resuming does not stop
    /// there again.
    last_breakpoint: Option<usize>,
}

impl Session {
    /// Initialize a `program` under `config`'s dialect and fuel.
    pub fn new(mut program: Program, config: &Config) -> Session {
        program.renumber();
        let code = flatten(&program);
        Session {
            code,
            pc: 0,
            ptr: config.dialect.origin as isize,
            tape: Tape::new(&config.dialect),
            dialect: config.dialect,
            fuel: config.fuel,
            steps: 0,
            input: VecDeque::new(),
            input_closed: false,
            output: Vec::new(),
            fault: None,
            last_node: None,
            last_breakpoint: None,
            program,
        }
    }

    /// Execute one step. Never returns [`Step::Breakpoint`].
    pub fn step(&mut self) -> Step {
        self.run(1)
    }

    /// Execute up to `budget` steps, returning early on anything that
    /// needs the caller.
    pub fn run(&mut self, budget: u64) -> Step {
        self.run_until(budget, &[])
    }

    /// Like [`Session::run`], but also stops with [`Step::Breakpoint`]
    /// before executing a step whose node is in `breakpoints`.
    pub fn run_until(&mut self, budget: u64, breakpoints: &[NodeId]) -> Step {
        for _ in 0..budget {
            if let Some(fault) = self.fault {
                return Step::Fault(fault);
            }
            if self.pc == self.code.ops.len() {
                return Step::Done;
            }
            let node = self.code.source[self.pc].node;
            if breakpoints.contains(&node)
                && self.last_node != Some(node)
                && self.last_breakpoint != Some(self.pc)
            {
                self.last_breakpoint = Some(self.pc);
                return Step::Breakpoint;
            }
            if let Err(stop) = self.exec_op() {
                return stop;
            }
            self.last_node = Some(node);
            // A step has executed, so the last breakpoint may fire again
            self.last_breakpoint = None;
        }
        // The budget is spent, so report where things stand
        if let Some(fault) = self.fault {
            Step::Fault(fault)
        } else if self.pc == self.code.ops.len() {
            Step::Done
        } else {
            Step::Yielded
        }
    }

    /// Execute the current op. `Err` means the caller is needed and the
    /// step did not execute.
    fn exec_op(&mut self) -> Result<(), Step> {
        if self.fuel == Some(0) {
            return Err(self.fail(FaultCode::OutOfFuel, self.ptr));
        }
        match self.code.ops[self.pc] {
            Op::Add { at, delta } => {
                let v = self.load(at)? as i64 + delta as i64;
                self.store(at, v as Cell);
                self.pc += 1;
            }
            Op::Store { at, value } => {
                let index = self.cell(at)?;
                self.tape.set(index, value);
                self.pc += 1;
            }
            Op::AddScaled { at, from, factor } => {
                // The notation reads `[at] += [from] * factor`
                let src = self.load(from)? as i64;
                let dst = self.load(at)? as i64;
                self.store(at, (dst + src * factor as i64) as Cell);
                self.pc += 1;
            }
            Op::Read { at } => {
                let index = self.cell(at)?;
                if let Some(byte) = self.input.pop_front() {
                    // Low 8 bits in, high bits zeroed.
                    self.tape.set(index, byte.into());
                } else if !self.input_closed {
                    return Err(Step::NeedInput);
                } else {
                    match self.dialect.eof {
                        EofBehavior::Zero => self.tape.set(index, 0),
                        EofBehavior::MinusOne => {
                            self.tape.set(index, self.dialect.cell_width.mask());
                        }
                        EofBehavior::Unchanged => {}
                    }
                }
                self.pc += 1;
            }
            Op::Write { at } => {
                let v = self.load(at)?;
                self.output.push((v & 0xff) as u8);
                self.pc += 1;
            }
            Op::Shift { by } => {
                self.ptr += by;
                self.pc += 1;
            }
            Op::EnterLoop { exit } => {
                self.pc = if self.load(0)? == 0 {
                    exit
                } else {
                    self.pc + 1
                };
            }
            Op::BackEdge { top } => {
                self.pc = if self.load(0)? != 0 { top } else { self.pc + 1 };
            }
            Op::Scan { stride } => {
                if self.load(0)? == 0 {
                    self.pc += 1;
                } else {
                    self.ptr += stride;
                }
            }
        }
        self.steps += 1;
        if let Some(fuel) = &mut self.fuel {
            *fuel -= 1;
        }
        Ok(())
    }

    /// Record the fault, which finishes the session.
    fn fail(&mut self, code: FaultCode, position: isize) -> Step {
        let fault = RuntimeError { code, position };
        self.fault = Some(fault);
        Step::Fault(fault)
    }

    /// Bounds-check `ptr + at`.
    fn cell(&mut self, at: isize) -> Result<usize, Step> {
        let index = self.ptr + at;
        if index < 0 {
            Err(self.fail(FaultCode::TapeUnderflow, index))
        } else if index as usize >= self.tape.len_cells() {
            Err(self.fail(FaultCode::TapeOverflow, index))
        } else {
            Ok(index as usize)
        }
    }

    fn load(&mut self, at: isize) -> Result<Cell, Step> {
        let index = self.cell(at)?;
        Ok(self.tape.get(index).expect("index is bounds-checked"))
    }

    /// Store at a cell that [`Session::cell`] has already checked.
    fn store(&mut self, at: isize, value: Cell) {
        let index = (self.ptr + at) as usize;
        self.tape.set(index, value & self.dialect.cell_width.mask());
    }

    /// Append bytes to the input queue.
    ///
    /// # Panics
    ///
    /// Panics if input has been closed.
    pub fn feed_input(&mut self, bytes: &[u8]) {
        assert!(!self.input_closed, "feed_input after close_input");
        self.input.extend(bytes);
    }

    /// Mark input as permanently exhausted. The pending `read`, if any,
    /// and every later one then follow [`crate::config::EofBehavior`].
    pub fn close_input(&mut self) {
        self.input_closed = true;
    }

    /// Drain the output accumulated since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.output)
    }

    /// The step about to execute. `None` once the session is finished.
    pub fn position(&self) -> Option<Position> {
        if self.fault.is_some() || self.pc == self.code.ops.len() {
            return None;
        }
        Some(self.code.source[self.pc])
    }

    /// The renumbered program this session executes.
    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn tape(&self) -> &Tape {
        &self.tape
    }

    /// The pointer. It may sit outside the tape, since only accesses fault.
    pub fn ptr(&self) -> isize {
        self.ptr
    }

    /// Steps executed so far.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// The fault that finished the session, if any.
    pub fn fault(&self) -> Option<RuntimeError> {
        self.fault
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CellWidth, OptLevel};
    use crate::error::Span;
    use crate::ir::build::Builder;
    use crate::ir::lower;
    use crate::parser;

    fn config() -> Config {
        Config::new(OptLevel::O0)
    }

    fn tiny() -> Config {
        config().with_dialect(Dialect {
            tape_cells: 4,
            ..Dialect::default()
        })
    }

    fn session_with(src: &str, config: &Config) -> Session {
        let ast = parser::parse(src).expect("test program parses");
        Session::new(lower::lower(&ast), config)
    }

    fn session(src: &str) -> Session {
        session_with(src, &config())
    }

    /// Drive to whatever needs the caller, panicking only on `Yielded`.
    fn finish(s: &mut Session) -> Step {
        match s.run(u64::MAX) {
            Step::Yielded => panic!("u64::MAX steps was not enough"),
            step => step,
        }
    }

    fn pos(node: u32, effect: usize) -> Position {
        Position {
            node: NodeId(node),
            effect,
        }
    }

    #[test]
    fn add_and_write() {
        let mut s = session("+++[>++++<-]>.");
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.take_output(), vec![12]);
        assert_eq!(s.ptr(), 1);
    }

    #[test]
    fn hello_world() {
        let mut s = session(
            "++++++++[>++++[>++>+++>+++>+<<<<-]>+>+>->>+[<]<-]>>.>---.+++++++..+++.>>.<-.<.+++.------.--------.>>+.>++.",
        );
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.take_output(), b"Hello World!\n".to_vec());
    }

    #[test]
    fn an_empty_program_is_born_done() {
        let mut s = session("");
        assert_eq!(s.position(), None);
        assert_eq!(s.step(), Step::Done);
        assert_eq!(s.steps(), 0);
    }

    #[test]
    fn budget_yields_and_resumes() {
        let mut s = session("+++++");
        assert_eq!(s.run(0), Step::Yielded);
        assert_eq!(s.run(2), Step::Yielded);
        assert_eq!(s.steps(), 2);
        assert_eq!(s.run(100), Step::Done);
        assert_eq!(s.steps(), 5);
        // A finished session reports its state even with no budget.
        assert_eq!(s.run(0), Step::Done);
    }

    #[test]
    fn finishing_on_the_last_budgeted_step_is_done() {
        let mut s = session("+++");
        assert_eq!(s.run(3), Step::Done);
    }

    #[test]
    fn step_yields_until_done() {
        let mut s = session("+.");
        assert_eq!(s.step(), Step::Yielded);
        assert_eq!(s.step(), Step::Done);
        assert_eq!(s.step(), Step::Done);
        assert_eq!(s.take_output(), vec![1]);
    }

    #[test]
    fn budget_interrupts_an_infinite_loop() {
        let mut s = session("+[]");
        assert_eq!(s.run(10_000), Step::Yielded);
        assert_eq!(s.steps(), 10_000);
    }

    #[test]
    fn cancelled_movement_is_not_a_step() {
        // `><` cancels inside the run: one add, no shift op.
        let mut s = session("+><");
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.steps(), 1);
    }

    #[test]
    fn position_tracks_the_shift_as_the_last_line() {
        // `+>` is one run: the add at effect index 0, the shift at 1.
        let mut s = session("+>");
        assert_eq!(s.position(), Some(pos(0, 0)));
        s.step();
        assert_eq!(s.position(), Some(pos(0, 1)));
        s.step();
        assert_eq!(s.position(), None);
    }

    #[test]
    fn session_renumbers_its_program() {
        let s = session("++[-]+.");
        let nodes = s.program().body.nodes();
        assert_eq!(nodes[0].id, NodeId(0));
        assert_eq!(nodes[1].id, NodeId(1));
        let NodeKind::Loop(l) = &nodes[1].kind else {
            panic!("expected a loop");
        };
        assert_eq!(l.body.nodes()[0].id, NodeId(2));
        assert_eq!(nodes[2].id, NodeId(3));
    }

    #[test]
    fn read_parks_then_resumes() {
        let mut s = session(",.");
        assert_eq!(s.step(), Step::NeedInput);
        assert_eq!(s.steps(), 0);
        assert_eq!(s.position(), Some(pos(0, 0)));
        s.feed_input(b"A");
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.take_output(), b"A".to_vec());
    }

    #[test]
    fn prefed_input_never_parks() {
        let mut s = session(",.,.");
        s.feed_input(b"hi");
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.take_output(), b"hi".to_vec());
    }

    #[test]
    fn eof_behaviors() {
        for (eof, expected) in [
            (EofBehavior::Zero, 0u8),
            (EofBehavior::MinusOne, 0xff),
            (EofBehavior::Unchanged, 5),
        ] {
            let config = config().with_dialect(Dialect {
                eof,
                ..Dialect::default()
            });
            let mut s = session_with("+++++,.", &config);
            s.close_input();
            assert_eq!(finish(&mut s), Step::Done, "{eof:?}");
            assert_eq!(s.take_output(), vec![expected], "{eof:?}");
        }
    }

    #[test]
    fn close_input_unparks_a_pending_read() {
        let mut s = session(",");
        assert_eq!(s.step(), Step::NeedInput);
        s.close_input();
        assert_eq!(s.step(), Step::Done);
    }

    #[test]
    #[should_panic(expected = "close_input")]
    fn feed_after_close_panics() {
        let mut s = session(",");
        s.close_input();
        s.feed_input(b"x");
    }

    #[test]
    fn eof_minus_one_is_all_ones_at_width() {
        let config = config().with_dialect(Dialect {
            cell_width: CellWidth::U16,
            eof: EofBehavior::MinusOne,
            ..Dialect::default()
        });
        let mut s = session_with(",", &config);
        s.close_input();
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.tape().get(0), Some(0xffff));
    }

    #[test]
    fn read_zeroes_the_high_bits() {
        let config = config().with_dialect(Dialect {
            cell_width: CellWidth::U16,
            ..Dialect::default()
        });
        let mut s = session_with("-,", &config);
        s.feed_input(b"A");
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.tape().get(0), Some(b'A'.into()));
    }

    #[test]
    fn write_emits_the_low_byte() {
        let config = config().with_dialect(Dialect {
            cell_width: CellWidth::U16,
            ..Dialect::default()
        });
        let mut s = session_with("-.", &config);
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.take_output(), vec![0xff]);
    }

    #[test]
    fn cells_wrap_at_the_configured_width() {
        for (width, expected) in [
            (CellWidth::U8, 0xff_u32),
            (CellWidth::U16, 0xffff),
            (CellWidth::U32, 0xffff_ffff),
        ] {
            let config = config().with_dialect(Dialect {
                cell_width: width,
                ..Dialect::default()
            });
            let mut s = session_with("-", &config);
            assert_eq!(finish(&mut s), Step::Done);
            assert_eq!(s.tape().get(0), Some(expected), "{width:?}");
        }
    }

    #[test]
    fn set_stores_masked() {
        let mut b = Builder::new();
        b.push(
            EffKind::Store {
                at: 0,
                value: 0xabcd,
            },
            Span::SYNTHETIC,
        );
        let mut s = Session::new(b.finish(), &config());
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.tape().get(0), Some(0xcd));
    }

    #[test]
    fn add_scaled_computes_at_width() {
        for (width, expected) in [(CellWidth::U8, 600_u32 & 0xff), (CellWidth::U16, 600)] {
            let mut b = Builder::new();
            b.push(EffKind::Add { at: 0, delta: 200 }, Span::SYNTHETIC);
            b.push(
                EffKind::AddScaled {
                    at: 1,
                    from: 0,
                    factor: 3,
                },
                Span::SYNTHETIC,
            );
            let config = config().with_dialect(Dialect {
                cell_width: width,
                ..Dialect::default()
            });
            let mut s = Session::new(b.finish(), &config);
            assert_eq!(finish(&mut s), Step::Done);
            assert_eq!(s.tape().get(1), Some(expected), "{width:?}");
        }
    }

    #[test]
    fn add_scaled_negative_factor_wraps() {
        let mut b = Builder::new();
        b.push(EffKind::Add { at: 0, delta: 5 }, Span::SYNTHETIC);
        b.push(
            EffKind::AddScaled {
                at: 1,
                from: 0,
                factor: -1,
            },
            Span::SYNTHETIC,
        );
        let mut s = Session::new(b.finish(), &config());
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.tape().get(1), Some(0xfb)); // 0 - 5 mod 256
    }

    #[test]
    fn pointer_roams_but_accesses_fault() {
        let mut s = session_with("<", &tiny());
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.ptr(), -1);

        let mut s = session_with(">>>>", &tiny());
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.ptr(), 4);
    }

    #[test]
    fn underflow_faults_at_the_attempted_cell() {
        let mut s = session_with("<<+", &tiny());
        let fault = RuntimeError {
            code: FaultCode::TapeUnderflow,
            position: -2,
        };
        assert_eq!(finish(&mut s), Step::Fault(fault));
        assert_eq!(s.fault(), Some(fault));
        assert_eq!(s.position(), None);
        // The fault is permanent, but the tape can still be read.
        assert_eq!(s.step(), Step::Fault(fault));
        assert_eq!(s.tape().get(0), Some(0));
    }

    #[test]
    fn overflow_faults_at_the_attempted_cell() {
        let mut s = session_with(">>>>+", &tiny());
        assert_eq!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::TapeOverflow,
                position: 4,
            })
        );
    }

    #[test]
    fn loop_test_is_a_checked_access() {
        let mut s = session_with("<[]", &tiny());
        assert_eq!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::TapeUnderflow,
                position: -1,
            })
        );
    }

    #[test]
    fn out_of_bounds_read_faults_before_parking() {
        // An in-bounds read with no input would park, but bounds go first.
        let mut s = session_with("<,", &tiny());
        assert_eq!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::TapeUnderflow,
                position: -1,
            })
        );
    }

    #[test]
    fn read_checks_bounds_even_when_eof_leaves_the_cell() {
        let config = config().with_dialect(Dialect {
            tape_cells: 4,
            eof: EofBehavior::Unchanged,
            ..Dialect::default()
        });
        let mut s = session_with("<,", &config);
        s.close_input();
        assert_eq!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::TapeUnderflow,
                position: -1,
            })
        );
    }

    #[test]
    fn exact_fuel_completes() {
        let mut s = session_with("+++", &config().with_fuel(3));
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.steps(), 3);
    }

    #[test]
    fn out_of_fuel_faults() {
        let mut s = session_with("+++", &config().with_fuel(2));
        let step = finish(&mut s);
        assert!(
            matches!(
                step,
                Step::Fault(RuntimeError {
                    code: FaultCode::OutOfFuel,
                    ..
                })
            ),
            "{step:?}"
        );
        assert_eq!(s.steps(), 2);
    }

    #[test]
    fn fuel_terminates_an_infinite_loop() {
        let mut s = session_with("+[]", &config().with_fuel(1000));
        assert!(matches!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::OutOfFuel,
                ..
            })
        ));
    }

    fn scan_program(cells: &[(isize, CellDelta)], stride: isize) -> Program {
        let mut b = Builder::new();
        for &(at, delta) in cells {
            b.push(EffKind::Add { at, delta }, Span::SYNTHETIC);
        }
        b.scan(stride, Span::SYNTHETIC);
        b.finish()
    }

    #[test]
    fn scan_walks_to_the_first_zero() {
        let mut s = Session::new(scan_program(&[(0, 1), (1, 1)], 1), &config());
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.ptr(), 2);
        assert_eq!(s.steps(), 5); // two adds, three scan tests
    }

    #[test]
    fn scan_faults_at_the_tape_edge() {
        let program = scan_program(&[(0, 1), (1, 1), (2, 1), (3, 1)], 1);
        let mut s = Session::new(program, &tiny());
        assert_eq!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::TapeOverflow,
                position: 4,
            })
        );
        assert_eq!(s.ptr(), 4);
    }

    #[test]
    fn scans_burn_fuel() {
        let program = scan_program(&[(0, 1), (1, 1)], 1);
        let mut s = Session::new(program, &config().with_fuel(3));
        // Two adds and one scan test execute, then the next test has no fuel.
        assert!(matches!(
            finish(&mut s),
            Step::Fault(RuntimeError {
                code: FaultCode::OutOfFuel,
                ..
            })
        ));
        assert_eq!(s.steps(), 3);
    }

    #[test]
    fn breakpoints_fire_on_node_entry() {
        // Traversal order: 0 = `++`, 1 = the loop, 2 = `-`, 3 = `+.`.
        let mut s = session("++[-]+.");
        let bp = [NodeId(3)];
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Breakpoint);
        assert_eq!(s.position(), Some(pos(3, 0)));
        // Resuming does not re-hit the same stop.
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Done);
        assert_eq!(s.take_output(), vec![1]);
    }

    #[test]
    fn a_loop_head_breakpoint_fires_per_trip() {
        let mut s = session("++[-]");
        let bp = [NodeId(1)];
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Breakpoint); // entry test
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Breakpoint); // after trip 1
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Breakpoint); // after trip 2
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Done);
    }

    #[test]
    fn a_breakpoint_on_the_first_node_fires_before_execution() {
        let mut s = session("++++");
        let bp = [NodeId(0)];
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Breakpoint);
        assert_eq!(s.steps(), 0);
        // One report per stop: the run's remaining steps share the node.
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Done);
        assert_eq!(s.steps(), 4);
    }

    #[test]
    fn an_empty_loop_breakpoint_fires_only_at_entry() {
        // After the entry test, `[]`'s re-tests are not node transitions.
        let mut s = session("+[]");
        let bp = [NodeId(1)];
        assert_eq!(s.run_until(u64::MAX, &bp), Step::Breakpoint);
        assert_eq!(s.run_until(1_000, &bp), Step::Yielded);
    }

    #[test]
    fn a_breakpoint_on_a_budget_boundary_still_fires() {
        // Yield exactly at the transition into the loop, then resume.
        let mut s = session("++[-]");
        assert_eq!(s.run_until(2, &[NodeId(1)]), Step::Yielded);
        assert_eq!(s.run_until(u64::MAX, &[NodeId(1)]), Step::Breakpoint);
        assert_eq!(s.steps(), 2);
    }

    #[test]
    fn take_output_drains() {
        let mut s = session("+..");
        finish(&mut s);
        assert_eq!(s.take_output(), vec![1, 1]);
        assert_eq!(s.take_output(), Vec::<u8>::new());
    }

    #[test]
    fn the_pointer_starts_at_origin() {
        let config = config().with_dialect(Dialect {
            tape_cells: 4,
            origin: 2,
            ..Dialect::default()
        });
        let mut s = session_with("<+", &config);
        assert_eq!(s.ptr(), 2);
        assert_eq!(finish(&mut s), Step::Done);
        assert_eq!(s.tape().get(1), Some(1));
    }
}

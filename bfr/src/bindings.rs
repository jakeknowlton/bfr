//! JavaScript bindings for the browser build.

#![cfg(all(target_arch = "wasm32", feature = "wasm-bindgen"))]

use wasm_bindgen::prelude::*;

use crate::ast;
use crate::config::{Config, OptLevel};
use crate::error::FaultCode;
use crate::interp::{Session, Step};
use crate::ir::{NodeId, lower, print};
use crate::parser;

/// Optimization level
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub enum Level {
    O0 = 0,
    O1 = 1,
    O2 = 2,
    O3 = 3,
}

impl From<Level> for OptLevel {
    fn from(level: Level) -> OptLevel {
        match level {
            Level::O0 => OptLevel::O0,
            Level::O1 => OptLevel::O1,
            Level::O2 => OptLevel::O2,
            Level::O3 => OptLevel::O3,
        }
    }
}

/// [`crate::interp::Step`] mirrored for JS, without the fault payload.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub enum StepCode {
    Yielded = 0,
    Breakpoint = 1,
    NeedInput = 2,
    Done = 3,
    Fault = 4,
}

impl From<Step> for StepCode {
    fn from(step: Step) -> StepCode {
        match step {
            Step::Yielded => StepCode::Yielded,
            Step::Breakpoint => StepCode::Breakpoint,
            Step::NeedInput => StepCode::NeedInput,
            Step::Done => StepCode::Done,
            Step::Fault(_) => StepCode::Fault,
        }
    }
}

fn js_error(error: impl core::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

/// A [`crate::interp::Session`] the page owns and drives.
#[wasm_bindgen]
pub struct DebugSession {
    session: Session,
    input_closed: bool,
}

#[wasm_bindgen]
impl DebugSession {
    /// Compile `src` at `level` (default dialect) and begin executing it.
    #[wasm_bindgen(constructor)]
    pub fn new(src: &str, level: Level) -> Result<DebugSession, JsError> {
        let config = Config::new(level.into());
        let (program, _) = crate::compile_to_ir(src, &config).map_err(js_error)?;
        Ok(DebugSession {
            session: Session::new(program, &config),
            input_closed: false,
        })
    }

    /// Execute one step.
    pub fn step(&mut self) -> StepCode {
        self.session.step().into()
    }

    /// Execute up to `budget` steps.
    pub fn run(&mut self, budget: u32) -> StepCode {
        self.session.run(budget.into()).into()
    }

    /// Like `run`, but also stops on any node id in `breakpoints` (the
    /// first step of the call is exempt, so resuming works).
    #[wasm_bindgen(js_name = runUntil)]
    pub fn run_until(&mut self, budget: u32, breakpoints: &[u32]) -> StepCode {
        let ids: Vec<NodeId> = breakpoints.iter().map(|&id| NodeId(id)).collect();
        self.session.run_until(budget.into(), &ids).into()
    }

    /// Append input bytes. Each `read` takes the next byte from the queue.
    ///
    /// # Errors
    ///
    /// Throws if input has already been closed.
    #[wasm_bindgen(js_name = feedInput)]
    pub fn feed_input(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        if self.input_closed {
            return Err(JsError::new("input has been closed"));
        }
        self.session.feed_input(bytes);
        Ok(())
    }

    /// Mark input permanently exhausted.
    #[wasm_bindgen(js_name = closeInput)]
    pub fn close_input(&mut self) {
        self.input_closed = true;
        self.session.close_input();
    }

    /// Output accumulated since the last call.
    #[wasm_bindgen(js_name = takeOutput)]
    pub fn take_output(&mut self) -> Vec<u8> {
        self.session.take_output()
    }

    /// String dump of this session's program.
    pub fn dump(&self) -> String {
        print::print(self.session.program())
    }

    /// The node id owning each line of `dump()`, index-aligned.
    #[wasm_bindgen(js_name = lineIds)]
    pub fn line_ids(&self) -> Vec<u32> {
        print::lines(self.session.program())
            .iter()
            .map(|line| line.node.0)
            .collect()
    }

    /// The id of the node about to execute. `null` once the session is
    /// finished.
    #[wasm_bindgen(js_name = nodeId)]
    pub fn node_id(&self) -> Option<u32> {
        self.session.position().map(|position| position.node.0)
    }

    /// Current cell index. It can sit outside the tape, since only
    /// accesses fault.
    pub fn ptr(&self) -> i32 {
        self.session.ptr() as i32
    }

    /// A window of cell values for the tape view, clamped to the tape end.
    pub fn cells(&self, start: u32, len: u32) -> Vec<u32> {
        let tape = self.session.tape();
        (start..start.saturating_add(len))
            .map_while(|index| tape.get(index as usize))
            .collect()
    }

    /// Steps executed so far.
    pub fn steps(&self) -> f64 {
        self.session.steps() as f64
    }

    /// The fault message, once `Fault` has been returned.
    #[wasm_bindgen(js_name = faultMessage)]
    pub fn fault_message(&self) -> Option<String> {
        self.session.fault().map(|fault| fault.to_string())
    }

    /// Faulting cell index, negative for an underflow. `null` for
    /// `OutOfFuel`, whose position is meaningless.
    #[wasm_bindgen(js_name = faultPosition)]
    pub fn fault_position(&self) -> Option<i32> {
        self.session
            .fault()
            .filter(|fault| fault.code != FaultCode::OutOfFuel)
            .map(|fault| fault.position as i32)
    }
}

/// The parse, printed back as canonical source.
#[wasm_bindgen(js_name = parseDump)]
pub fn parse_dump(src: &str) -> Result<String, JsError> {
    let ast = parser::parse(src).map_err(js_error)?;
    Ok(ast::print(&ast))
}

/// Pretty-printed unoptimized IR.
#[wasm_bindgen(js_name = irDump)]
pub fn ir_dump(src: &str) -> Result<String, JsError> {
    let ast = parser::parse(src).map_err(js_error)?;
    Ok(print::print(&lower::lower(&ast)))
}

/// Optimized IR, then a blank line, then a rendering of
/// [`crate::opt::Stats`].
#[wasm_bindgen(js_name = optimizeDump)]
pub fn optimize_dump(src: &str, level: Level) -> Result<String, JsError> {
    let config = Config::new(level.into());
    let (program, stats) = crate::compile_to_ir(src, &config).map_err(js_error)?;
    Ok(format!("{}\n{stats}", print::print(&program)))
}

/// The notation key for the IR dumps.
#[wasm_bindgen(js_name = irLegend)]
pub fn ir_legend() -> String {
    print::LEGEND.to_string()
}

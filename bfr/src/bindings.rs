//! JavaScript bindings for the browser build.

#![cfg(all(target_arch = "wasm32", feature = "wasm-bindgen"))]

use wasm_bindgen::prelude::*;

/// Optimization level
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub enum Level {
    O0 = 0,
    O1 = 1,
    O2 = 2,
    O3 = 3,
}

/// [`crate::interp::Step`] mirrored for JS, without the fault payload.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub enum StepCode {
    Ran = 0,
    Breakpoint = 1,
    NeedInput = 2,
    Done = 3,
    Fault = 4,
}

/// A [`crate::interp::Session`] the page owns and drives.
#[wasm_bindgen]
pub struct DebugSession {
    _private: (),
}

#[wasm_bindgen]
impl DebugSession {
    /// Compile `src` at `level` (default dialect) and begin executing it.
    #[wasm_bindgen(constructor)]
    pub fn new(src: &str, level: Level) -> Result<DebugSession, JsError> {
        todo!("DebugSession::new")
    }

    /// Execute one step.
    pub fn step(&mut self) -> StepCode {
        todo!("DebugSession::step")
    }

    /// Execute up to `budget` steps.
    pub fn run(&mut self, budget: u32) -> StepCode {
        todo!("DebugSession::run")
    }

    /// Like `run`, but also stops on any node id in `breakpoints` (the
    /// first step of the call is exempt, so resuming works).
    #[wasm_bindgen(js_name = runUntil)]
    pub fn run_until(&mut self, budget: u32, breakpoints: &[u32]) -> StepCode {
        todo!("DebugSession::run_until")
    }

    /// Append input bytes; `read`s self-serve from the queue.
    ///
    /// # Errors
    ///
    /// Throws if input has already been closed.
    #[wasm_bindgen(js_name = feedInput)]
    pub fn feed_input(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        todo!("DebugSession::feed_input")
    }

    /// Mark input permanently exhausted.
    #[wasm_bindgen(js_name = closeInput)]
    pub fn close_input(&mut self) {
        todo!("DebugSession::close_input")
    }

    /// Output accumulated since the last call.
    #[wasm_bindgen(js_name = takeOutput)]
    pub fn take_output(&mut self) -> Vec<u8> {
        todo!("DebugSession::take_output")
    }

    /// String dump of this session's program.
    pub fn dump(&self) -> String {
        todo!("DebugSession::dump")
    }

    /// The node id owning each line of `dump()`, index-aligned.
    #[wasm_bindgen(js_name = lineIds)]
    pub fn line_ids(&self) -> Vec<u32> {
        todo!("DebugSession::line_ids")
    }

    /// `NodeId` about to execute. `null` once finished.
    #[wasm_bindgen(js_name = nodeId)]
    pub fn node_id(&self) -> Option<u32> {
        todo!("DebugSession::node_id")
    }

    /// Current cell index.
    pub fn ptr(&self) -> u32 {
        todo!("DebugSession::ptr")
    }

    /// A window of cell values for the tape view, clamped to the tape end.
    pub fn cells(&self, start: u32, len: u32) -> Vec<u32> {
        todo!("DebugSession::cells")
    }

    /// Steps retired so far.
    pub fn steps(&self) -> f64 {
        todo!("DebugSession::steps")
    }

    /// Fault message once `Fault` was returned.
    #[wasm_bindgen(js_name = faultMessage)]
    pub fn fault_message(&self) -> Option<String> {
        todo!("DebugSession::fault_message")
    }

    /// Faulting cell index. `null` for `OutOfFuel`, whose position is meaningless.
    #[wasm_bindgen(js_name = faultPosition)]
    pub fn fault_position(&self) -> Option<u32> {
        todo!("DebugSession::fault_position")
    }
}

/// Pretty-printed AST.
#[wasm_bindgen(js_name = parseDump)]
pub fn parse_dump(src: &str) -> Result<String, JsError> {
    todo!("bindings::parse_dump")
}

/// Pretty-printed unoptimized IR.
#[wasm_bindgen(js_name = irDump)]
pub fn ir_dump(src: &str) -> Result<String, JsError> {
    todo!("bindings::ir_dump")
}

/// Optimized IR plus a rendering of [`crate::opt::Stats`].
#[wasm_bindgen(js_name = optimizeDump)]
pub fn optimize_dump(src: &str, level: Level) -> Result<String, JsError> {
    todo!("bindings::optimize_dump")
}

/// The notation key for the IR dumps.
#[wasm_bindgen(js_name = irLegend)]
pub fn ir_legend() -> String {
    crate::ir::print::LEGEND.to_string()
}

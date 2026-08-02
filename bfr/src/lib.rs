//! An optimizing brainfuck engine: parser, IR optimizer, and reference
//! interpreter.
//!
//! Every stage is public so the intermediate forms can be inspected.
//!
//! ```no_run
//! use bfr::{Config, OptLevel};
//!
//! let out = bfr::run_to_vec("+++[>++++<-]>.", &Config::new(OptLevel::O2), b"")?;
//! # Ok::<(), bfr::Error>(())
//! ```

pub mod ast;
pub mod config;
pub mod error;
pub mod interp;
pub mod ir;
pub mod opt;
pub mod parser;
pub mod tape;

#[cfg(all(target_arch = "wasm32", feature = "wasm-bindgen"))]
pub mod bindings;

pub use config::{CellWidth, Config, Dialect, EofBehavior, OptLevel};
pub use error::{Error, FaultCode, ParseError, Result, RuntimeError, Span};
pub use interp::{Session, Step};
pub use opt::{Pass, Pipeline, Stats};
pub use tape::Tape;

/// Source text to optimized IR, with the [`opt::Stats`] the pipeline collected along the way.
pub fn compile(src: &str, config: &Config) -> Result<(ir::Program, Stats)> {
    todo!("bfr::compile")
}

/// Run with a fixed input buffer and collect the output.
pub fn run_to_vec(src: &str, config: &Config, input: &[u8]) -> Result<Vec<u8>> {
    todo!("bfr::run_to_vec")
}

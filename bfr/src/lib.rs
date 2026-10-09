//! An optimizing brainfuck engine: parser, IR optimizer, and interpreter.
//!
//! Every stage is public so the intermediate forms can be inspected.
//!
//! ```
//! use bfr::{Config, OptLevel};
//!
//! let out = bfr::run_to_vec("+++[>++++<-]>.", &Config::new(OptLevel::O2), b"")?;
//! # Ok::<(), bfr::Error>(())
//! ```

#![allow(rustdoc::private_intra_doc_links)]

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
pub use interp::{Position, Session, Step};
pub use opt::{Pass, Pipeline, Stats};
pub use tape::Tape;

/// Source text to optimized IR.
pub fn compile_to_ir(src: &str, config: &Config) -> Result<(ir::Program, Stats)> {
    let ast = parser::parse(src)?;
    let mut program = ir::lower::lower(&ast);
    let stats = config
        .pipeline
        .run(&mut program, &opt::Ctx::new(&config.dialect));
    Ok((program, stats))
}

/// Run with a fixed input buffer and collect the output.
pub fn run_to_vec(src: &str, config: &Config, input: &[u8]) -> Result<Vec<u8>> {
    let (program, _) = compile_to_ir(src, config)?;
    let mut session = Session::new(program, config);
    session.feed_input(input);
    session.close_input();
    loop {
        match session.run(u64::MAX) {
            Step::Done => return Ok(session.take_output()),
            Step::Fault(fault) => return Err(fault.into()),
            Step::Yielded => {}
            Step::NeedInput | Step::Breakpoint => {
                unreachable!("input is closed and no breakpoints are set")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o0() -> Config {
        Config::new(OptLevel::O0)
    }

    #[test]
    fn run_to_vec_collects_output() {
        assert_eq!(run_to_vec("+++[>++++<-]>.", &o0(), b"").unwrap(), vec![12]);
    }

    #[test]
    fn run_to_vec_reads_the_buffer_then_eof() {
        assert_eq!(run_to_vec(",.,.,.", &o0(), b"hi").unwrap(), b"hi\0");
    }

    #[test]
    fn parse_errors_surface() {
        assert!(matches!(compile_to_ir("[", &o0()), Err(Error::Parse(_))));
    }

    #[test]
    fn faults_surface() {
        assert!(matches!(
            run_to_vec("<.", &o0(), b""),
            Err(Error::Runtime(_))
        ));
    }
}

//! Compile and run a program against the terminal and report the outcome.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use bfr::ir::Program;
use bfr::opt::Stats;
use bfr::{Pipeline, Session, Step};

use crate::cli::{Args, Emit};

/// Whether a path names an IR file rather than brainfuck source.
pub fn is_ir_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "bfr")
}

/// Read a file as unoptimized IR, parsing it as brainfuck or as IR by its
/// extension. Errors are already formatted for the terminal.
pub fn load_program(path: &Path) -> Result<Program, String> {
    let src = fs::read_to_string(path).map_err(|e| format!("error: {}: {e}", path.display()))?;
    if is_ir_file(path) {
        bfr::ir::print::parse(&src)
            .map_err(|(offset, message)| format!("error: {}: byte {offset}: {message}", path.display()))
    } else {
        let ast = bfr::parser::parse(&src).map_err(|e| e.to_string())?;
        Ok(bfr::ir::lower::lower(&ast))
    }
}

pub fn run_file(path: &Path, args: &Args) -> ExitCode {
    if args.emit == Emit::Ast {
        if is_ir_file(path) {
            eprintln!("error: an IR file has no AST to emit");
            return ExitCode::FAILURE;
        }
        let src = match fs::read_to_string(path) {
            Ok(src) => src,
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        return match bfr::parser::parse(&src) {
            Ok(ast) => emit_text(args.output.as_deref(), &bfr::ast::print(&ast)),
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    let config = match args.config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut program = match load_program(path) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let stats = bfr::optimize(&mut program, &config);
    warn_unconverged(&stats, &config.pipeline);
    if args.stats {
        eprint!("{stats}");
    }

    match args.emit {
        Emit::Ast => unreachable!("handled above"),
        Emit::Ir => emit_text(args.output.as_deref(), &bfr::ir::print::print(&program)),
        Emit::Run => {
            let mut out: Box<dyn Write> = match &args.output {
                Some(path) => match fs::File::create(path) {
                    Ok(file) => Box::new(file),
                    Err(e) => {
                        eprintln!("error: {}: {e}", path.display());
                        return ExitCode::FAILURE;
                    }
                },
                None => Box::new(io::stdout()),
            };
            let mut session = Session::new(program, &config);
            match drive(&mut session, &mut out) {
                Ok((Step::Done, _)) => ExitCode::SUCCESS,
                Ok((Step::Fault(fault), _)) => {
                    eprintln!("runtime error: {fault}");
                    ExitCode::FAILURE
                }
                Ok((step, _)) => unreachable!("drive returned {step:?}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Drive `session` until it is done or faults. Returns the final step and
/// the last output byte, which the REPL uses to decide whether it needs a
/// newline.
pub fn drive(session: &mut Session, out: &mut dyn Write) -> io::Result<(Step, Option<u8>)> {
    // Run in slices so long programs stream their output
    const BUDGET: u64 = 1 << 20;
    let mut tail = None;
    loop {
        let step = session.run(BUDGET);
        let output = session.take_output();
        if !output.is_empty() {
            tail = output.last().copied();
            out.write_all(&output)?;
        }
        match step {
            Step::Yielded => {}
            Step::NeedInput => {
                out.flush()?;
                let mut line = Vec::new();
                if io::stdin().lock().read_until(b'\n', &mut line)? == 0 {
                    session.close_input();
                } else {
                    session.feed_input(&line);
                }
            }
            Step::Done | Step::Fault(_) | Step::Breakpoint => {
                out.flush()?;
                return Ok((step, tail));
            }
        }
    }
}

/// Write a dump to `--output` or stdout, newline-terminated.
fn emit_text(path: Option<&Path>, text: &str) -> ExitCode {
    let newline = if text.ends_with('\n') || text.is_empty() {
        ""
    } else {
        "\n"
    };
    match path {
        Some(path) => {
            if let Err(e) = fs::write(path, format!("{text}{newline}")) {
                eprintln!("error: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        None => print!("{text}{newline}"),
    }
    ExitCode::SUCCESS
}

pub fn warn_unconverged(stats: &Stats, pipeline: &Pipeline) {
    if !stats.reached_fixed_point(pipeline) {
        eprintln!(
            "warning: the pipeline did not reach a fixed point in {} sweeps",
            stats.sweeps
        );
    }
}

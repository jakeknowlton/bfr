//! Compile and run a program against the terminal and report the outcome.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use bfr::opt::Stats;
use bfr::{Pipeline, Session, Step};

use crate::cli::{Args, Emit};

pub fn run_file(path: &Path, args: &Args) -> ExitCode {
    let src = match fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    if args.emit == Emit::Ast {
        return match bfr::parser::parse(&src) {
            Ok(ast) => emit_text(args.output.as_deref(), &bfr::ast::print(&ast)),
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    let config = args.config();
    let (program, stats) = match bfr::compile_to_ir(&src, &config) {
        Ok(compiled) => compiled,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    warn_unconverged(&stats, &config.pipeline);
    if args.stats {
        eprint!("{}", render_stats(&stats));
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

/// Drive `session` to a terminal outcome.
/// Returns the final step result and the last output byte for the REPL.
pub fn drive(session: &mut Session, out: &mut dyn Write) -> io::Result<(Step, Option<u8>)> {
    // Slice the run so long programs stream their output
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

pub fn render_stats(stats: &Stats) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "pipeline: {} {}, {} -> {} ops\n",
        stats.sweeps,
        if stats.sweeps == 1 { "sweep" } else { "sweeps" },
        stats.ops_before,
        stats.ops_after
    );
    let width = stats.passes.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for pass in &stats.passes {
        _ = writeln!(
            out,
            "  {:width$}  runs {:>2}  changes {:>2}",
            pass.name, pass.runs, pass.changes
        );
    }
    out
}

pub fn warn_unconverged(stats: &Stats, pipeline: &Pipeline) {
    if !stats.reached_fixed_point(pipeline) {
        eprintln!(
            "warning: the pipeline did not reach a fixed point in {} sweeps",
            stats.sweeps
        );
    }
}

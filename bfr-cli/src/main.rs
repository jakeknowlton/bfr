//! Command-line driver for [`bfr`].

use std::path::PathBuf;

use bfr::{CellWidth, Config, Dialect, EofBehavior, OptLevel};
use clap::{Parser, ValueEnum};

/// An optimizing brainfuck compiler.
#[derive(Parser, Debug)]
#[command(name = "bfr", version, about)]
struct Args {
    /// Brainfuck source file.
    file: PathBuf,

    /// Optimization level.
    #[arg(short = 'O', value_parser = parse_opt_level, default_value = "1")]
    opt: OptLevel,

    /// What to produce.
    #[arg(long, value_enum, default_value_t = Emit::Run)]
    emit: Emit,

    /// Write output artifacts here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Abort after this many loop iterations.
    #[arg(long)]
    fuel: Option<u64>,

    /// Turn off a pass by name, e.g. `--no-pass PartialEval`. Repeatable.
    #[arg(long = "no-pass", value_name = "NAME")]
    disabled: Vec<String>,

    /// Print pass statistics after optimizing.
    #[arg(long)]
    stats: bool,

    /// Input behavior on EOF.
    #[arg(long, value_enum, default_value_t = Eof::Zero)]
    eof: Eof,

    /// Bitwidth of the cells in the tape.
    #[arg(long, value_enum, default_value_t = Width::U8)]
    cell_width: Width,

    /// Tape length in cells.
    #[arg(long, default_value_t = 1 << 21)]
    tape_cells: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Emit {
    /// Execute the program (interpreted).
    Run,
    /// Pretty-printed AST.
    Ast,
    /// Pretty-printed IR, with source spans.
    Ir,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Eof {
    Zero,
    Unchanged,
    MinusOne,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Width {
    U8,
    U16,
    U32,
}

fn parse_opt_level(s: &str) -> Result<OptLevel, String> {
    match s {
        "0" => Ok(OptLevel::O0),
        "1" => Ok(OptLevel::O1),
        "2" => Ok(OptLevel::O2),
        "3" => Ok(OptLevel::O3),
        other => Err(format!("optimization level must be 0-3, got {other:?}")),
    }
}

impl Args {
    fn config(&self) -> Config {
        let mut config = Config::new(self.opt).with_dialect(Dialect {
            cell_width: match self.cell_width {
                Width::U8 => CellWidth::U8,
                Width::U16 => CellWidth::U16,
                Width::U32 => CellWidth::U32,
            },
            eof: match self.eof {
                Eof::Zero => EofBehavior::Zero,
                Eof::MinusOne => EofBehavior::MinusOne,
                Eof::Unchanged => EofBehavior::Unchanged,
            },
            tape_cells: self.tape_cells,
            origin: 0,
        });
        config.fuel = self.fuel;
        for name in &self.disabled {
            if config.pipeline.disable(name) == 0 {
                eprintln!(
                    "warning: no pass named {name:?}; available: {:?}",
                    config.pipeline.names()
                );
            }
        }
        config
    }
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    _ = args.config();
    todo!("bfr-cli: read file, build config, dispatch on --emit")
}

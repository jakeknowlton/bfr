//! Command-line driver for [`bfr`].

mod cli;
mod driver;
mod repl;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Args, Emit};

fn main() -> ExitCode {
    let args = Args::parse();
    match args.file.clone() {
        Some(path) => driver::run_file(&path, &args),
        None if args.emit != Emit::Run || args.output.is_some() => {
            eprintln!("error: --emit and --output need a source file");
            ExitCode::FAILURE
        }
        None => repl::run(args),
    }
}

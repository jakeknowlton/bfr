//! Interactive read-eval-print loop.

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use bfr::ast::Ast;
use bfr::error::ParseErrorKind;
use bfr::{CellWidth, Config, EofBehavior, Error, Session, Stats, Step};

use crate::cli::{Args, parse_opt_level};
use crate::driver::{drive, render_stats, warn_unconverged};

const HELP: &str = "\
:opt [0-3]     show or set the optimization level
:config        dialect and limits for this session
:ir [code]     dump optimized IR (default: the last program run)
:ast [code]    echo the parse as canonical source (default: the last program run)
:tape          pointer, executed IR steps, and nonzero cells after the last run
:stats         pipeline statistics for the last run
:load <file>   run a source file
:cancel        discard the unfinished input at the ...> prompt
:help          this text
:quit, :q      leave";

const PROMPT_STYLE: &str = "\x1b[1m"; // bold
const OUTPUT_STYLE: &str = "\x1b[96m"; // intense cyan
const RESET_STYLE: &str = "\x1b[0m";

enum Flow {
    Continue,
    Quit,
}

struct Repl {
    args: Args,
    config: Config,
    /// Accumulated source while brackets are unbalanced.
    pending: String,
    /// The finished run, kept for the inspection commands.
    last: Option<LastRun>,
    /// The outcome of the most recent run.
    status: ExitCode,
    /// Whether program output is tinted (see [`should_style`]).
    tint: bool,
    /// Whether the prompt is styled. Decided by stderr, since that is
    /// where prompts go.
    styled_prompt: bool,
}

struct LastRun {
    session: Session,
    stats: Stats,
    ast: Ast,
}

/// Tints program output cyan so it is visually distinct from inputs and prompts.
struct Tint<W: Write>(W);

impl<W: Write> Write for Tint<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.0.write_all(OUTPUT_STYLE.as_bytes())?;
        self.0.write_all(buf)?;
        self.0.write_all(RESET_STYLE.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

fn should_style<T: IsTerminal>(stream: &T) -> bool {
    stream.is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var_os("TERM").is_none_or(|v| v != "dumb")
}

pub fn run(args: Args) -> ExitCode {
    eprintln!(
        "bfr {} repl at -{} -- :help for commands, :quit to leave",
        env!("CARGO_PKG_VERSION"),
        args.opt
    );
    let mut repl = Repl {
        config: args.config(),
        args,
        pending: String::new(),
        last: None,
        status: ExitCode::SUCCESS,
        tint: should_style(&io::stdout()),
        styled_prompt: should_style(&io::stderr()),
    };
    loop {
        let prompt = if repl.pending.is_empty() {
            "bfr>"
        } else {
            "...>"
        };
        if repl.styled_prompt {
            eprint!("{PROMPT_STYLE}{prompt}{RESET_STYLE} ");
        } else {
            eprint!("{prompt} ");
        }

        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => {
                // Make sure the user's shell prompt is on its own line
                if io::stdin().is_terminal() {
                    eprintln!();
                }
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
        if let Flow::Quit = repl.dispatch(&line) {
            break;
        }
    }
    repl.status
}

impl Repl {
    fn dispatch(&mut self, line: &str) -> Flow {
        let trimmed = line.trim();
        if let Some(command) = trimmed.strip_prefix(':') {
            return self.command(command);
        }

        self.pending.push_str(line);
        // An unclosed `[` means more lines are coming
        if let Err(Error::Parse(e)) = bfr::parser::parse(&self.pending)
            && e.kind == ParseErrorKind::UnmatchedOpen
        {
            return Flow::Continue;
        }
        let src = core::mem::take(&mut self.pending);
        if src.contains(['+', '-', '<', '>', '.', ',', '[', ']']) {
            self.eval(&src);
        }
        Flow::Continue
    }

    fn command(&mut self, command: &str) -> Flow {
        let (name, arg) = match command.split_once(char::is_whitespace) {
            Some((name, arg)) => (name, arg.trim()),
            None => (command, ""),
        };
        match name {
            // Reject extra arguments
            "help" | "quit" | "q" | "cancel" | "config" | "tape" | "stats" if !arg.is_empty() => {
                eprintln!("`:{name}` takes no arguments")
            }
            "help" => println!("{HELP}"),
            "quit" | "q" => return Flow::Quit,
            "cancel" => self.pending.clear(),
            "opt" => self.set_opt(arg),
            "config" => self.show_config(),
            "ast" => self.show_ast(arg),
            "ir" => self.show_ir(arg),
            "tape" => self.show_tape(),
            "stats" => match &self.last {
                Some(last) => print!("{}", render_stats(&last.stats)),
                None => eprintln!("nothing has run yet"),
            },
            "load" => self.load(arg),
            other => eprintln!("unknown command `:{other}`; :help to see a list of commands"),
        }
        Flow::Continue
    }

    fn eval(&mut self, src: &str) {
        let ast = match bfr::parser::parse(src) {
            Ok(ast) => ast,
            Err(e) => {
                eprintln!("{e}");
                self.status = ExitCode::FAILURE;
                return;
            }
        };
        let mut program = bfr::ir::lower::lower(&ast);
        let stats = self
            .config
            .pipeline
            .run(&mut program, &bfr::opt::Ctx::new(&self.config.dialect));
        warn_unconverged(&stats, &self.config.pipeline);
        if self.args.stats {
            eprint!("{}", render_stats(&stats));
        }

        let mut session = Session::new(program, &self.config);
        let driven = if self.tint {
            drive(&mut session, &mut Tint(io::stdout()))
        } else {
            drive(&mut session, &mut io::stdout())
        };
        self.status = match driven {
            Ok((step, tail)) => {
                // Make sure the next prompt is on its own line
                if tail.is_some_and(|byte| byte != b'\n') && io::stdout().is_terminal() {
                    println!();
                }
                if let Step::Fault(fault) = step {
                    eprintln!("runtime error: {fault}");
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
        self.last = Some(LastRun {
            session,
            stats,
            ast,
        });
    }

    fn set_opt(&mut self, arg: &str) {
        if arg.is_empty() {
            println!("{}", self.args.opt);
            return;
        }
        match parse_opt_level(arg) {
            Ok(level) => {
                self.args.opt = level;
                self.config = self.args.config();
                println!("{level}");
            }
            Err(e) => eprintln!("{e}"),
        }
    }

    fn show_config(&self) {
        let dialect = &self.config.dialect;
        let cell_width = match dialect.cell_width {
            CellWidth::U8 => "8",
            CellWidth::U16 => "16",
            CellWidth::U32 => "32",
        };
        let eof = match dialect.eof {
            EofBehavior::Zero => "zero",
            EofBehavior::Unchanged => "unchanged",
            EofBehavior::MinusOne => "minus-one",
        };
        println!("opt            {}", self.args.opt);
        println!("cell width     {cell_width}");
        println!("eof            {eof}");
        println!("tape           {} cells", dialect.tape_cells);
        match self.config.fuel {
            Some(fuel) => println!("fuel           {fuel} ir steps"),
            None => println!("fuel           unlimited"),
        }
    }

    fn show_ast(&self, code: &str) {
        if code.is_empty() {
            match &self.last {
                Some(last) => println!("{}", bfr::ast::print(&last.ast)),
                None => eprintln!("nothing has run yet"),
            }
            return;
        }
        match bfr::parser::parse(code) {
            Ok(ast) => println!("{}", bfr::ast::print(&ast)),
            Err(e) => eprintln!("{e}"),
        }
    }

    fn show_ir(&self, code: &str) {
        if code.is_empty() {
            match &self.last {
                Some(last) => print!("{}", bfr::ir::print::print(last.session.program())),
                None => eprintln!("nothing has run yet"),
            }
            return;
        }
        match bfr::compile_to_ir(code, &self.config) {
            Ok((program, stats)) => {
                warn_unconverged(&stats, &self.config.pipeline);
                print!("{}", bfr::ir::print::print(&program));
            }
            Err(e) => eprintln!("{e}"),
        }
    }

    fn show_tape(&self) {
        const CELLS_TO_SHOW: usize = 32;
        let Some(LastRun { session, .. }) = &self.last else {
            eprintln!("nothing has run yet");
            return;
        };
        println!(
            "after last run: ptr = {}, ir steps = {}",
            session.ptr(),
            session.steps()
        );

        let tape = session.tape();
        let nonzero: Vec<(usize, u32)> = (0..tape.len_cells())
            .filter_map(|i| Some((i, tape.get(i)?)))
            .filter(|&(_, value)| value != 0)
            .collect();
        if nonzero.is_empty() {
            println!("all cells are zero");
            return;
        }
        let cells: Vec<String> = nonzero
            .iter()
            .take(CELLS_TO_SHOW)
            .map(|(i, value)| format!("[{i}] = {value}"))
            .collect();
        println!("{}", cells.join("  "));
        if nonzero.len() > CELLS_TO_SHOW {
            println!(
                "... and {} more nonzero cells",
                nonzero.len() - CELLS_TO_SHOW
            );
        }
    }

    fn load(&mut self, path: &str) {
        if path.is_empty() {
            eprintln!("usage: :load <file>");
            return;
        }
        let path = expand_home(path);
        match fs::read_to_string(&path) {
            Ok(src) => self.eval(&src),
            Err(e) => eprintln!("error: {}: {e}", path.display()),
        }
    }
}

/// Expand `~` to the home directory.
fn expand_home(path: &str) -> PathBuf {
    let Some(rest) = path.strip_prefix('~') else {
        return PathBuf::from(path);
    };
    match std::env::home_dir() {
        Some(home) if rest.is_empty() => home,
        Some(home) if rest.starts_with(std::path::is_separator) => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

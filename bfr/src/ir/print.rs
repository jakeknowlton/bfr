//! Human-readable IR dumps, and their parser.

use crate::ir::Program;

/// The notation legend, for a UI to render beside a dump.
pub const LEGEND: &str = "\
p            the data pointer
[p+n]        the cell n places right of the pointer ([p] is the current cell)
[p] += n     add to a cell
[p] = n      store a constant
[a] += [b]*n add a multiple of one cell into another
[p] = read() input
write([p])   output
p += n       move the pointer; always the last thing a run does
scan p += n  step by n until the cell is zero
while [p]    loop while the current cell is nonzero";

/// Dump a whole program: one line per effect and shift, two spaces of
/// indent per loop level.
pub fn print(program: &Program) -> String {
    todo!("ir::print::print")
}

/// Dump with source spans in an aligned right-hand column.
pub fn print_annotated(program: &Program) -> String {
    todo!("ir::print::print_annotated")
}

/// Parse the notation back into IR.
///
/// # Errors
///
/// Returns the byte offset and a message on malformed input.
pub fn parse(text: &str) -> Result<Program, (usize, String)> {
    todo!("ir::print::parse")
}

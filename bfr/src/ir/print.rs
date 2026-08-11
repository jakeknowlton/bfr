//! Human-readable IR dumps, and their parser.

use crate::ir::{Block, EffKind, NodeKind, Program};

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

/// Dump a whole program.
pub fn print(program: &Program) -> String {
    let mut out = String::new();
    print_block(&mut out, &program.body, 0);
    out
}

fn print_block(out: &mut String, block: &Block, depth: usize) {
    for node in block.nodes() {
        match &node.kind {
            NodeKind::Run(run) => {
                for eff in &run.effects {
                    line(out, depth, &effect(&eff.kind));
                }
                if run.shift != 0 {
                    line(out, depth, &format!("p {}", update(run.shift as i64)));
                }
            }
            NodeKind::Loop(l) => {
                line(out, depth, "while [p] {");
                print_block(out, &l.body, depth + 1);
                line(out, depth, "}");
            }
            NodeKind::Scan(scan) => {
                line(
                    out,
                    depth,
                    &format!("scan p {}", update(scan.stride as i64)),
                );
            }
        }
    }
}

fn line(out: &mut String, depth: usize, text: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(text);
    out.push('\n');
}

/// `[p]`, `[p+2]`, `[p-1]`.
fn cell(at: isize) -> String {
    match at {
        0 => "[p]".to_string(),
        n if n > 0 => format!("[p+{n}]"),
        n => format!("[p-{}]", n.unsigned_abs()),
    }
}

/// `+= n` / `-= n`.
fn update(delta: i64) -> String {
    if delta < 0 {
        format!("-= {}", delta.unsigned_abs())
    } else {
        format!("+= {delta}")
    }
}

fn effect(kind: &EffKind) -> String {
    match kind {
        EffKind::Add { at, delta } => format!("{} {}", cell(*at), update((*delta).into())),
        EffKind::Set { at, value } => format!("{} = {value}", cell(*at)),
        EffKind::AddScaled { at, from, factor } => {
            let op = if *factor < 0 { "-=" } else { "+=" };
            let mut text = format!("{} {op} {}", cell(*at), cell(*from));
            if factor.unsigned_abs() != 1 {
                text.push_str(&format!(" * {}", factor.unsigned_abs()));
            }
            text
        }
        EffKind::Read { at } => format!("{} = read()", cell(*at)),
        EffKind::Write { at } => format!("write({})", cell(*at)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Span;
    use crate::ir::build::Builder;
    use crate::ir::lower;
    use crate::parser;

    fn dump(src: &str) -> String {
        print(&lower::lower(&parser::parse(src).expect("parses")))
    }

    #[test]
    fn the_docs_worked_example_at_o0() {
        let expected = [
            "[p] += 1\n",
            "[p] += 1\n",
            "[p] += 1\n",
            "while [p] {\n",
            "  [p+1] += 1\n",
            "  [p+1] += 1\n",
            "  [p+1] += 1\n",
            "  [p+1] += 1\n",
            "  [p] -= 1\n",
            "}\n",
            "write([p+1])\n",
            "p += 1\n",
        ];
        assert_eq!(dump("+++[>++++<-]>."), expected.concat());
    }

    #[test]
    fn every_form_prints() {
        let mut b = Builder::new();
        b.push(EffKind::Add { at: -2, delta: -4 }, Span::SYNTHETIC);
        b.push(EffKind::Set { at: 0, value: 0 }, Span::SYNTHETIC);
        b.push(
            EffKind::AddScaled {
                at: 1,
                from: 0,
                factor: 2,
            },
            Span::SYNTHETIC,
        );
        b.push(
            EffKind::AddScaled {
                at: 1,
                from: 0,
                factor: -1,
            },
            Span::SYNTHETIC,
        );
        b.push(EffKind::Read { at: 0 }, Span::SYNTHETIC);
        b.push(EffKind::Write { at: 3 }, Span::SYNTHETIC);
        b.scan(-2, Span::SYNTHETIC);
        let expected = [
            "[p-2] -= 4\n",
            "[p] = 0\n",
            "[p+1] += [p] * 2\n",
            "[p+1] -= [p]\n",
            "[p] = read()\n",
            "write([p+3])\n",
            "scan p -= 2\n",
        ];
        assert_eq!(print(&b.finish()), expected.concat());
    }

    #[test]
    fn nesting_indents_two_spaces_per_level() {
        assert_eq!(
            dump("[[-]]"),
            "while [p] {\n  while [p] {\n    [p] -= 1\n  }\n}\n"
        );
    }

    #[test]
    fn an_empty_program_prints_nothing() {
        assert_eq!(dump(""), "");
    }
}

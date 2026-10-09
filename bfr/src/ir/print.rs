//! Human-readable IR dumps, and their parser.

use crate::error::Span;
use crate::ir::build::Builder;
use crate::ir::{Block, EffKind, Node, NodeId, NodeKind, Program};

/// The notation legend, for a UI to render beside a dump.
pub const LEGEND: &str = "\
[0]          the current cell
[+n], [-n]   the cell n places right or left of the current cell
[a] += n     add to a cell
[a] = n      store a constant
[a] += [b]*n add a multiple of one cell into another
[a] = read   input
write [a]    output
shift n      move the pointer, always the last thing a run does
scan n       step by n until the cell is zero
loop {       loop while the current cell is nonzero
+n, -n       a direction always carries its sign";

/// One line of a dump, with the node it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Nesting depth, two spaces of indent per level.
    pub depth: usize,
    /// The line without its indent.
    pub text: String,
    /// The node this line belongs to.
    pub node: NodeId,
    /// The span of the effect on this line, or of the node for a shift,
    /// loop or scan line.
    pub span: Span,
}

/// Dump a whole program as lines, one per effect, shift, loop bracket or
/// scan.
pub fn lines(program: &Program) -> Vec<Line> {
    let mut out = Vec::new();
    push_block(&mut out, &program.body, 0);
    out
}

fn push_block(out: &mut Vec<Line>, block: &Block, depth: usize) {
    for node in block.nodes() {
        match &node.kind {
            NodeKind::Run(run) => {
                for eff in &run.effects {
                    push_line(out, depth, effect(&eff.kind), node, eff.span);
                }
                if run.shift != 0 {
                    let text = format!("shift {}", direction(run.shift));
                    push_line(out, depth, text, node, node.span);
                }
            }
            NodeKind::Loop(l) => {
                push_line(out, depth, "loop {".to_string(), node, node.span);
                push_block(out, &l.body, depth + 1);
                push_line(out, depth, "}".to_string(), node, node.span);
            }
            NodeKind::Scan(scan) => {
                let text = format!("scan {}", direction(scan.stride));
                push_line(out, depth, text, node, node.span);
            }
        }
    }
}

fn push_line(out: &mut Vec<Line>, depth: usize, text: String, node: &Node, span: Span) {
    out.push(Line {
        depth,
        text,
        node: node.id,
        span,
    });
}

/// Dump a whole program.
pub fn print(program: &Program) -> String {
    let mut out = String::new();
    for line in lines(program) {
        indent(&mut out, line.depth);
        out.push_str(&line.text);
        out.push('\n');
    }
    out
}

/// Dump with source spans in an aligned right-hand column. A synthetic
/// span leaves the column blank.
pub fn print_annotated(program: &Program) -> String {
    let lines = lines(program);
    let width = lines
        .iter()
        .map(|line| line.depth * 2 + line.text.len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for line in &lines {
        let start = out.len();
        indent(&mut out, line.depth);
        out.push_str(&line.text);
        if line.span != Span::SYNTHETIC {
            let used = out.len() - start;
            for _ in used..width + 2 {
                out.push(' ');
            }
            out.push_str(&format!("{}..{}", line.span.start, line.span.end));
        }
        out.push('\n');
    }
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// `0`, `+2`, `-1`. A direction always carries its sign, so a reader can
/// tell an offset or a pointer move from an amount.
fn direction(n: isize) -> String {
    if n > 0 { format!("+{n}") } else { n.to_string() }
}

/// `[0]`, `[+2]`, `[-1]`.
fn cell(at: isize) -> String {
    format!("[{}]", direction(at))
}

fn effect(kind: &EffKind) -> String {
    match kind {
        EffKind::Add { at, delta } => format!("{} += {delta}", cell(*at)),
        EffKind::Store { at, value } => format!("{} = {value}", cell(*at)),
        EffKind::AddScaled { at, from, factor } => {
            let mut text = format!("{} += {}", cell(*at), cell(*from));
            if *factor != 1 {
                text.push_str(&format!(" * {factor}"));
            }
            text
        }
        EffKind::Read { at } => format!("{} = read", cell(*at)),
        EffKind::Write { at } => format!("write {}", cell(*at)),
    }
}

/// Parse the notation back into IR. Whitespace, including newlines, only
/// separates tokens, so the layout is free, and a `;` may follow any
/// statement, so a program can be written on one line. Every node's span
/// is the byte range of its statements in `text`.
///
/// # Errors
///
/// Returns the byte offset and a message on malformed input.
pub fn parse(text: &str) -> Result<Program, (usize, String)> {
    let mut scanner = Scanner { text, pos: 0 };
    let mut builder = Builder::new();
    let mut open: Vec<usize> = Vec::new();
    loop {
        scanner.skip_separators();
        if scanner.pos == text.len() {
            break;
        }
        let start = scanner.pos;
        let statement = scanner.statement()?;
        let span = Span::new(start, scanner.pos);
        match statement {
            Statement::Effect(kind) => builder.push(kind, span),
            Statement::Shift(by) => builder.bump(by, span),
            Statement::Scan(stride) => builder.scan(stride, span),
            Statement::Open => {
                open.push(start);
                builder.begin_loop(span);
            }
            Statement::Close => {
                if open.pop().is_none() {
                    return Err((start, "`}` without an open `loop`".to_string()));
                }
                builder.end_loop(span);
            }
        }
    }
    if let Some(at) = open.pop() {
        return Err((at, "`loop` without a closing `}`".to_string()));
    }
    Ok(builder.finish())
}

/// One statement of the notation.
enum Statement {
    Effect(EffKind),
    Shift(isize),
    Scan(isize),
    /// `loop {`
    Open,
    /// `}`
    Close,
}

/// Reads the notation as a stream of tokens. Each method skips whitespace
/// first, so a failed match leaves `pos` at the token it rejected and an
/// error reports that position.
struct Scanner<'a> {
    text: &'a str,
    pos: usize,
}

impl Scanner<'_> {
    fn statement(&mut self) -> Result<Statement, (usize, String)> {
        if self.eat("}") {
            Ok(Statement::Close)
        } else if self.word("loop") {
            self.expect("{")?;
            Ok(Statement::Open)
        } else if self.word("scan") {
            let stride = self.number("a stride")?;
            if stride.value == 0 {
                return Err((stride.at, "a scan needs a nonzero stride".to_string()));
            }
            Ok(Statement::Scan(stride.fit("the stride is too large")?))
        } else if self.word("shift") {
            let by = self.number("an amount")?.fit("the amount is too large")?;
            Ok(Statement::Shift(by))
        } else if self.word("write") {
            let at = self.cell()?;
            Ok(Statement::Effect(EffKind::Write { at }))
        } else if self.peek("[") {
            let at = self.cell()?;
            if self.eat("=") {
                if self.word("read") {
                    return Ok(Statement::Effect(EffKind::Read { at }));
                }
                let value = self.number("a cell value")?;
                if value.value < 0 {
                    return Err((value.at, "a cell value cannot be negative".to_string()));
                }
                let value = value.fit("the value does not fit a cell")?;
                return Ok(Statement::Effect(EffKind::Store { at, value }));
            }
            if !self.eat("+=") {
                return Err(self.error("expected `+=` or `=`"));
            }
            if self.peek("[") {
                let from = self.cell()?;
                let factor = if self.eat("*") {
                    self.number("a factor")?
                        .fit("the factor does not fit a cell delta")?
                } else {
                    1
                };
                return Ok(Statement::Effect(EffKind::AddScaled { at, from, factor }));
            }
            let delta = self
                .number("an amount")?
                .fit("the amount does not fit a cell delta")?;
            Ok(Statement::Effect(EffKind::Add { at, delta }))
        } else {
            Err(self.error("expected a cell, `shift`, `write`, `scan`, `loop` or `}`"))
        }
    }

    fn skip_whitespace(&mut self) {
        let rest = &self.text[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// Skip semicolons between statements.
    fn skip_separators(&mut self) {
        while self.eat(";") {}
    }

    /// Whether `token` comes next.
    fn peek(&mut self, token: &str) -> bool {
        self.skip_whitespace();
        self.text[self.pos..].starts_with(token)
    }

    /// Consume `token` if it comes next.
    fn eat(&mut self, token: &str) -> bool {
        if self.peek(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), (usize, String)> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{token}`")))
        }
    }

    /// Consume `word` if it comes next as a whole word, so `p` does not
    /// match the start of `print`.
    fn word(&mut self, word: &str) -> bool {
        if !self.peek(word) {
            return false;
        }
        let after = &self.text[self.pos + word.len()..];
        if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            return false;
        }
        self.pos += word.len();
        true
    }

    fn error(&self, message: &str) -> (usize, String) {
        (self.pos, message.to_string())
    }

    /// `[0]`, `[+n]` or `[-n]`, as an offset.
    fn cell(&mut self) -> Result<isize, (usize, String)> {
        self.expect("[")?;
        let at = self.number("an offset")?.fit("the offset is too large")?;
        self.expect("]")?;
        Ok(at)
    }

    /// A number with an optional sign. `what` names it in the error.
    fn number(&mut self, what: &str) -> Result<Number, (usize, String)> {
        self.skip_whitespace();
        let at = self.pos;
        let negative = self.eat("-");
        if !negative {
            self.eat("+");
        }
        let rest = &self.text[self.pos..];
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return Err((at, format!("expected {what}")));
        }
        let magnitude = rest[..digits]
            .parse::<i64>()
            .map_err(|_| (at, format!("{what} is too large")))?;
        self.pos += digits;
        let value = if negative { -magnitude } else { magnitude };
        Ok(Number { at, value })
    }
}

/// A number and where it started, so a range error points at it.
struct Number {
    at: usize,
    value: i64,
}

impl Number {
    /// The number converted to `T`, or `message` at the number's position
    /// when it does not fit.
    fn fit<T: TryFrom<i64>>(self, message: &str) -> Result<T, (usize, String)> {
        T::try_from(self.value).map_err(|_| (self.at, message.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower;
    use crate::parser;

    fn dump(src: &str) -> String {
        print(&lower::lower(&parser::parse(src).expect("parses")))
    }

    #[test]
    fn the_docs_worked_example_at_o0() {
        let expected = [
            "[0] += 1\n",
            "[0] += 1\n",
            "[0] += 1\n",
            "loop {\n",
            "  [+1] += 1\n",
            "  [+1] += 1\n",
            "  [+1] += 1\n",
            "  [+1] += 1\n",
            "  [0] += -1\n",
            "}\n",
            "write [+1]\n",
            "shift +1\n",
        ];
        assert_eq!(dump("+++[>++++<-]>."), expected.concat());
    }

    #[test]
    fn every_form_prints() {
        let mut b = Builder::new();
        b.push(EffKind::Add { at: -2, delta: -4 }, Span::SYNTHETIC);
        b.push(EffKind::Store { at: 0, value: 0 }, Span::SYNTHETIC);
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
            "[-2] += -4\n",
            "[0] = 0\n",
            "[+1] += [0] * 2\n",
            "[+1] += [0] * -1\n",
            "[0] = read\n",
            "write [+3]\n",
            "scan -2\n",
        ];
        assert_eq!(print(&b.finish()), expected.concat());
    }

    #[test]
    fn nesting_indents_two_spaces_per_level() {
        assert_eq!(
            dump("[[-]]"),
            "loop {\n  loop {\n    [0] += -1\n  }\n}\n"
        );
    }

    #[test]
    fn an_empty_program_prints_nothing() {
        assert_eq!(dump(""), "");
    }

    fn dump_at(src: &str, level: crate::config::OptLevel) -> Program {
        let config = crate::config::Config::new(level);
        crate::compile_to_ir(src, &config).expect("parses").0
    }

    #[test]
    fn lines_carry_node_ids_and_spans() {
        let mut program = lower::lower(&parser::parse("+[>]").expect("parses"));
        program.renumber();
        let lines = lines(&program);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["[0] += 1", "loop {", "shift +1", "}"]);
        let ids: Vec<u32> = lines.iter().map(|l| l.node.0).collect();
        assert_eq!(ids, [0, 1, 2, 1]);
        assert_eq!(lines[0].span, Span::new(0, 1));
        assert_eq!(lines[1].span, Span::new(1, 4));
        assert_eq!(lines[2].span, Span::new(2, 3));
        assert_eq!(lines[2].depth, 1);
    }

    #[test]
    fn annotated_aligns_spans_in_a_column() {
        let program = lower::lower(&parser::parse("+++[>++++<-]>.").expect("parses"));
        let expected = [
            "[0] += 1     0..1\n",
            "[0] += 1     1..2\n",
            "[0] += 1     2..3\n",
            "loop {       3..12\n",
            "  [+1] += 1  5..6\n",
            "  [+1] += 1  6..7\n",
            "  [+1] += 1  7..8\n",
            "  [+1] += 1  8..9\n",
            "  [0] += -1  10..11\n",
            "}            3..12\n",
            "write [+1]   13..14\n",
            "shift +1     12..14\n",
        ];
        assert_eq!(print_annotated(&program), expected.concat());
    }

    #[test]
    fn annotated_leaves_synthetic_spans_blank() {
        let mut b = Builder::new();
        b.push(EffKind::Add { at: 0, delta: 1 }, Span::SYNTHETIC);
        b.push(EffKind::Write { at: 0 }, Span::new(4, 5));
        let expected = ["[0] += 1\n", "write [0]  4..5\n"];
        assert_eq!(print_annotated(&b.finish()), expected.concat());
    }

    #[test]
    fn parse_round_trips_every_form() {
        let text = [
            "[-2] += -4\n",
            "[0] = 0\n",
            "[+1] += [0] * 2\n",
            "[+1] += [0] * -1\n",
            "[+3] += [-1] * 300\n",
            "[0] = read\n",
            "write [+3]\n",
            "shift +2\n",
            "scan -2\n",
            "loop {\n",
            "  [0] += -1\n",
            "  loop {\n",
            "    shift +1\n",
            "  }\n",
            "}\n",
            "[0] = 65535\n",
        ]
        .concat();
        let program = parse(&text).expect("parses");
        assert_eq!(print(&program), text);
    }

    #[test]
    fn parse_ignores_layout() {
        let text = [
            "\n\t[0]+=+1[+1]+=-2  \n",
            "write [ 0 ]\n",
            "loop\n",
            "{\n",
            "[0]+=-1 }\n",
            "shift+1 scan -1 [0]=read",
        ]
        .concat();
        let expected = [
            "[0] += 1\n",
            "[+1] += -2\n",
            "write [0]\n",
            "loop {\n",
            "  [0] += -1\n",
            "}\n",
            "shift +1\n",
            "scan -1\n",
            "[0] = read\n",
        ];
        assert_eq!(print(&parse(&text).expect("parses")), expected.concat());
    }

    #[test]
    fn parse_ends_cleanly_after_separators() {
        for text in [" ; ", "[0] += 1 ; ", "[0] += 1\n;\n", ";", "", "  "] {
            assert!(parse(text).is_ok(), "{text:?}");
        }
        assert!(parse(" ; ").expect("parses").body.is_empty());
    }

    #[test]
    fn parse_accepts_semicolons_between_statements() {
        let program = parse(";[0] += 1; [+1] += 2;loop {; [0] += -1; };;").expect("parses");
        let expected = [
            "[0] += 1\n",
            "[+1] += 2\n",
            "loop {\n",
            "  [0] += -1\n",
            "}\n",
        ];
        assert_eq!(print(&program), expected.concat());
    }

    /// Every notation example in a pass's module doc has to parse, so the
    /// docs cannot drift from the grammar. An example is a backticked span
    /// on a `//!` line that starts like a statement.
    #[test]
    fn the_doc_examples_parse() {
        let docs = [
            include_str!("../opt/normalize.rs"),
            include_str!("../opt/drain_loop.rs"),
            include_str!("../opt/scan.rs"),
            include_str!("../opt/const_fold.rs"),
            include_str!("../opt/unroll.rs"),
            include_str!("../opt/partial_eval.rs"),
            include_str!("../opt/dead_code.rs"),
        ];
        let mut checked = 0;
        for doc in docs {
            for line in doc.lines().filter(|l| l.starts_with("//!")) {
                for span in line.split('`').skip(1).step_by(2) {
                    let looks_like_notation = span.starts_with("loop")
                        || span.starts_with("shift")
                        || span.starts_with("scan")
                        || span.starts_with("write")
                        || span.starts_with("[0]")
                        || span.starts_with("[+")
                        || span.starts_with("[-0")
                        || span.starts_with("[-1")
                        || span.starts_with("[-2");
                    if !looks_like_notation || span.contains("...") {
                        continue;
                    }
                    assert!(parse(span).is_ok(), "{span:?}: {:?}", parse(span).err());
                    checked += 1;
                }
            }
        }
        assert!(checked >= 20, "only {checked} examples found");
    }

    #[test]
    fn parse_matches_words_whole() {
        assert!(parse("shifts +1").is_err());
        assert!(parse("loops {}").is_err());
        assert!(parse("[0] = reader").is_err());
    }

    #[test]
    fn parse_accepts_either_sign_convention() {
        let program = parse("[1] += +2\n[0] = +3\nshift 1").expect("parses");
        assert_eq!(print(&program), "[+1] += 2\n[0] = 3\nshift +1\n");
    }

    #[test]
    fn parse_matches_the_optimizer_at_every_level() {
        use crate::config::OptLevel;
        for src in ["+++[>++++<-]>.", ",[>+<-]>.", "+>+>+<<[>]<.", "[-]+[.-]"] {
            for level in [OptLevel::O0, OptLevel::O1, OptLevel::O2, OptLevel::O3] {
                let program = dump_at(src, level);
                let text = print(&program);
                let reparsed = parse(&text).expect("parses");
                assert_eq!(print(&reparsed), text, "{src} at {level}");
            }
        }
    }

    #[test]
    fn parsed_spans_cover_each_statement() {
        let program = parse("[0] += 1\n  write [0]\n").expect("parses");
        let run = &program.body.nodes()[0];
        assert_eq!(run.span, Span::new(0, 20));
        let lines = lines(&program);
        assert_eq!(lines[0].span, Span::new(0, 8));
        assert_eq!(lines[1].span, Span::new(11, 20));
        // A loop's span runs from `loop` to its `}`.
        let program = parse("loop\n{\n  [0] += -1\n}\n").expect("parses");
        assert_eq!(program.body.nodes()[0].span, Span::new(0, 20));
    }

    #[test]
    fn parse_reports_where_it_stopped() {
        let cases: [(&str, usize, &str); 14] = [
            ("loop ;{", 5, "expected `{`"),
            ("[0] ;+= 1", 4, "expected `+=` or `=`"),
            ("[q] += 1", 1, "expected an offset"),
            ("[+] += 1", 1, "expected an offset"),
            ("[0] = 256 cows", 10, "expected a cell, `shift`, `write`, `scan`, `loop` or `}`"),
            ("[0] = 4294967296", 6, "the value does not fit a cell"),
            ("[0] = -1", 6, "a cell value cannot be negative"),
            ("[0] *= 1", 4, "expected `+=` or `=`"),
            ("scan 0", 5, "a scan needs a nonzero stride"),
            ("}", 0, "`}` without an open `loop`"),
            ("loop {\n  [0] += -1\n", 0, "`loop` without a closing `}`"),
            ("loop\n  [0] += -1", 7, "expected `{`"),
            ("[0] += 1\nhello", 9, "expected a cell, `shift`, `write`, `scan`, `loop` or `}`"),
            ("write (0)", 6, "expected `[`"),
        ];
        for (text, offset, message) in cases {
            assert_eq!(parse(text), Err((offset, message.to_string())), "{text:?}");
        }
    }
}

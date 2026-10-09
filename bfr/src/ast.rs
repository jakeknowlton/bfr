//! Syntax tree.
//!
//! This level does no optimization. It exists so that the parser can stay a
//! simple single pass, and so source spans have a natural home.

use crate::error::Span;

/// A parsed brainfuck program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ast {
    pub body: Vec<Node>,
    pub source_len: usize,
}

impl Ast {
    /// Total node count including nested loop bodies.
    pub fn node_count(&self) -> usize {
        let mut count = 0;
        let mut stack = vec![self.body.as_slice()];
        while let Some(nodes) = stack.pop() {
            count += nodes.len();
            for node in nodes {
                if let NodeKind::Loop(body) = &node.kind {
                    stack.push(body);
                }
            }
        }
        count
    }

    /// Maximum loop nesting depth. A program without loops has depth 0.
    pub fn max_depth(&self) -> usize {
        let mut deepest = 0;
        let mut stack: Vec<(&[Node], usize)> = vec![(&self.body, 0)];
        while let Some((nodes, depth)) = stack.pop() {
            deepest = deepest.max(depth);
            for node in nodes {
                if let NodeKind::Loop(body) = &node.kind {
                    stack.push((body, depth + 1));
                }
            }
        }
        deepest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub span: Span,
    pub kind: NodeKind,
}

/// The brainfuck commands. `[` and `]` collapse into [`NodeKind::Loop`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// `>`
    Right,
    /// `<`
    Left,
    /// `+`
    Inc,
    /// `-`
    Dec,
    /// `.`
    Output,
    /// `,`
    Input,
    /// `[ body ]`.
    Loop(Vec<Node>),
}

/// Render an AST back to canonical brainfuck source (comments stripped).
pub fn print(ast: &Ast) -> String {
    let mut bf = String::new();
    print_nodes(&mut bf, &ast.body);
    bf
}

fn print_nodes(bf: &mut String, nodes: &[Node]) {
    for node in nodes {
        match &node.kind {
            NodeKind::Right => bf.push('>'),
            NodeKind::Left => bf.push('<'),
            NodeKind::Inc => bf.push('+'),
            NodeKind::Dec => bf.push('-'),
            NodeKind::Output => bf.push('.'),
            NodeKind::Input => bf.push(','),
            NodeKind::Loop(body) => {
                bf.push('[');
                print_nodes(bf, body);
                bf.push(']');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn ast(src: &str) -> Ast {
        parser::parse(src).expect("parses")
    }

    #[test]
    fn node_count_includes_loops_and_their_bodies() {
        assert_eq!(ast("").node_count(), 0);
        assert_eq!(ast("+++").node_count(), 3);
        // Two loops, three leaves.
        assert_eq!(ast("+[>[-]<]").node_count(), 6);
        assert_eq!(ast("[]").node_count(), 1);
    }

    #[test]
    fn node_count_ignores_comments() {
        assert_eq!(ast("a + b").node_count(), 1);
    }

    #[test]
    fn max_depth_counts_nested_loops() {
        assert_eq!(ast("").max_depth(), 0);
        assert_eq!(ast("+-<>.,").max_depth(), 0);
        assert_eq!(ast("[]").max_depth(), 1);
        assert_eq!(ast("[[]][]").max_depth(), 2);
        assert_eq!(ast("[[[-]]]+[]").max_depth(), 3);
    }

    #[test]
    fn print_round_trips_canonical_source() {
        let src = "+++[>++++<-]>.";
        assert_eq!(print(&ast(src)), src);
        assert_eq!(print(&ast("a+b[c-d]e")), "+[-]");
    }
}

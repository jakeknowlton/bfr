//! Syntax tree
//!
//! This level is not concerned with optimization. It exists so that the parser
//! stays trivial and total and so source spans have a natural home.

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
        todo!("Ast::node_count")
    }

    /// Maximum loop nesting depth.
    pub fn max_depth(&self) -> usize {
        todo!("Ast::max_depth")
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

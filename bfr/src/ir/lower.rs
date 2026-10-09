//! [`Ast`] -> [`Program`].

use crate::ast;
use crate::ast::Ast;
use crate::ir::build::Builder;
use crate::ir::{EffKind, Program};

/// Lower an AST to unoptimized IR.
pub fn lower(ast: &Ast) -> Program {
    let mut builder = Builder::new();

    // This recurses once per nesting level. A brainfuck program deep
    // enough to overflow the stack is unlikely, but if that becomes an
    // issue this can be rewritten to lower iteratively.
    lower_nodes(&mut builder, &ast.body);

    builder.finish()
}

fn lower_nodes(builder: &mut Builder, nodes: &[ast::Node]) {
    for node in nodes {
        let span = node.span;
        match &node.kind {
            ast::NodeKind::Right => builder.bump(1, span),
            ast::NodeKind::Left => builder.bump(-1, span),
            ast::NodeKind::Inc => builder.push(EffKind::Add { at: 0, delta: 1 }, span),
            ast::NodeKind::Dec => builder.push(EffKind::Add { at: 0, delta: -1 }, span),
            ast::NodeKind::Output => builder.push(EffKind::Write { at: 0 }, span),
            ast::NodeKind::Input => builder.push(EffKind::Read { at: 0 }, span),
            ast::NodeKind::Loop(body) => {
                builder.begin_loop(span);
                lower_nodes(builder, body);
                builder.end_loop(span);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;
    use crate::ir;
    use crate::ir::{Loop, Run};

    fn expect_run(node: &ir::Node) -> &Run {
        match &node.kind {
            ir::NodeKind::Run(run) => run,
            other => panic!("expected a run, got {other:?}"),
        }
    }

    fn expect_loop(node: &ir::Node) -> &Loop {
        match &node.kind {
            crate::ir::NodeKind::Loop(l) => l,
            other => panic!("expected a loop, got {other:?}"),
        }
    }

    #[test]
    fn inc_lowers_correctly() {
        // ++
        let ast = Ast {
            body: vec![
                ast::Node {
                    span: Span::new(0, 1),
                    kind: ast::NodeKind::Inc,
                },
                ast::Node {
                    span: Span::new(1, 2),
                    kind: ast::NodeKind::Inc,
                },
            ],
            source_len: 2,
        };
        let ir = lower(&ast);
        assert_eq!(ir.body.len(), 1);
        let nodes = ir.body.nodes();
        let run = expect_run(&nodes[0]);
        assert_eq!(run.effects.len(), 2);
        assert_eq!(run.effects[0].kind, EffKind::Add { at: 0, delta: 1 });
        assert_eq!(run.effects[1].kind, EffKind::Add { at: 0, delta: 1 });
        assert_eq!(run.shift, 0);
        assert_eq!(nodes[0].span, Span::new(0, 2));
        assert_eq!(nodes[0].id, ir::NodeId::UNASSIGNED);
    }

    #[test]
    fn loop_lowers_correctly() {
        // [>-,]
        let ast = Ast {
            body: vec![ast::Node {
                span: Span::new(0, 5),
                kind: ast::NodeKind::Loop(vec![
                    ast::Node {
                        span: Span::new(1, 2),
                        kind: ast::NodeKind::Right,
                    },
                    ast::Node {
                        span: Span::new(2, 3),
                        kind: ast::NodeKind::Dec,
                    },
                    ast::Node {
                        span: Span::new(3, 4),
                        kind: ast::NodeKind::Input,
                    },
                ]),
            }],
            source_len: 5,
        };
        let ir = lower(&ast);
        assert_eq!(ir.body.len(), 1);
        let nodes = ir.body.nodes();
        let ir_loop = expect_loop(&nodes[0]);
        let loop_nodes = ir_loop.body.nodes();
        assert_eq!(loop_nodes.len(), 1);
        let loop_run = expect_run(&loop_nodes[0]);
        assert_eq!(loop_run.effects.len(), 2);
        assert_eq!(loop_run.effects[0].kind, EffKind::Add { at: 1, delta: -1 });
        assert_eq!(loop_run.effects[1].kind, EffKind::Read { at: 1 });
        assert_eq!(loop_run.shift, 1);
        assert_eq!(loop_nodes[0].span, Span::new(1, 4));
        assert_eq!(loop_nodes[0].id, ir::NodeId::UNASSIGNED);
        assert_eq!(nodes[0].span, Span::new(0, 5));
        assert_eq!(loop_nodes[0].id, ir::NodeId::UNASSIGNED);
    }

    #[test]
    fn scan_lowers_as_loop() {
        // [>><>]
        let ast = Ast {
            body: vec![ast::Node {
                span: Span::new(0, 6),
                kind: ast::NodeKind::Loop(vec![
                    ast::Node {
                        span: Span::new(1, 2),
                        kind: ast::NodeKind::Right,
                    },
                    ast::Node {
                        span: Span::new(2, 3),
                        kind: ast::NodeKind::Right,
                    },
                    ast::Node {
                        span: Span::new(3, 4),
                        kind: ast::NodeKind::Left,
                    },
                    ast::Node {
                        span: Span::new(4, 5),
                        kind: ast::NodeKind::Right,
                    },
                ]),
            }],
            source_len: 6,
        };
        let ir = lower(&ast);
        assert_eq!(ir.body.len(), 1);
        let nodes = ir.body.nodes();
        let ir_loop = expect_loop(&nodes[0]);
        let loop_nodes = ir_loop.body.nodes();
        assert_eq!(loop_nodes.len(), 1);
        let loop_run = expect_run(&loop_nodes[0]);
        assert_eq!(loop_run.effects.len(), 0);
        assert_eq!(loop_run.shift, 2);
        assert_eq!(loop_nodes[0].span, Span::new(1, 5));
        assert_eq!(loop_nodes[0].id, ir::NodeId::UNASSIGNED);
        assert_eq!(nodes[0].span, Span::new(0, 6));
        assert_eq!(loop_nodes[0].id, ir::NodeId::UNASSIGNED);
    }
}

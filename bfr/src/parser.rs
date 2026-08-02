//! Source text -> [`Ast`].
//!
//! The entire parser is a single pass over the bytes with a stack of partially
//! built loop bodies. Brainfuck has no precedence, no identifiers, and no
//! literals, so the only reachable error is an unbalanced bracket.

use crate::ast::{Ast, Node, NodeKind};
use crate::error::{ParseErrorKind, Result};
use crate::{ParseError, Span};

/// Parsing stack frame.
struct Frame {
    open: usize,
    body: Vec<Node>,
}

/// Parse brainfuck source.
///
/// # Errors
///
/// Returns an error if brackets are unbalanced.
pub fn parse(src: &str) -> Result<Ast> {
    // Each element after the first in the stack represents a nested loop body
    let mut stack = vec![Frame {
        open: 0,
        body: Vec::new(),
    }];

    for (i, b) in src.bytes().enumerate() {
        let kind = match b {
            b'>' => NodeKind::Right,
            b'<' => NodeKind::Left,
            b'+' => NodeKind::Inc,
            b'-' => NodeKind::Dec,
            b'.' => NodeKind::Output,
            b',' => NodeKind::Input,
            b'[' => {
                stack.push(Frame {
                    open: i,
                    body: Vec::new(),
                });
                continue;
            }
            b']' => {
                if stack.len() == 1 {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnmatchedClose,
                        span: Span::new(i, i + 1),
                    }
                    .into());
                }

                let frame = stack.pop().unwrap();
                push(
                    &mut stack,
                    Node {
                        span: Span::new(frame.open, i + 1),
                        kind: NodeKind::Loop(frame.body),
                    },
                );
                continue;
            }
            _ => continue, // Skip all other characters
        };

        push(
            &mut stack,
            Node {
                span: Span::new(i, i + 1),
                kind,
            },
        );
    }

    // If there is at least one other frame
    if let Some(frame) = stack.get(1) {
        return Err(ParseError {
            kind: ParseErrorKind::UnmatchedOpen,
            span: Span::new(frame.open, frame.open + 1),
        }
        .into());
    }

    Ok(Ast {
        body: stack.pop().unwrap().body,
        source_len: src.len(),
    })
}

/// Helper to push a node onto the parsing stack.
fn push(stack: &mut [Frame], node: Node) {
    stack
        .last_mut()
        .expect("bottom frame is never popped")
        .body
        .push(node);
}

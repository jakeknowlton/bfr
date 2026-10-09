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
    // The bottom frame is the program body. Every frame above it is a loop
    // body that is still open.
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
            _ => continue, // Every other byte is a comment
        };

        push(
            &mut stack,
            Node {
                span: Span::new(i, i + 1),
                kind,
            },
        );
    }

    // Any frame left above the bottom one is a loop that was never closed
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

/// Push a node onto the innermost open body.
fn push(stack: &mut [Frame], node: Node) {
    stack
        .last_mut()
        .expect("bottom frame is never popped")
        .body
        .push(node);
}

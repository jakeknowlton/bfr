//! Error and fault types.

use core::fmt;

/// A byte range in the original source text.
/// `start` is inclusive, `end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// A span attached to synthesized code that has no source text.
    pub const SYNTHETIC: Span = Span { start: 0, end: 0 };

    pub const fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// The smallest span covering both inputs.
    pub fn merge(self, other: Span) -> Span {
        if self == Span::SYNTHETIC {
            return other;
        }
        if other == Span::SYNTHETIC {
            return self;
        }
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Parse(ParseError),
    Runtime(RuntimeError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// A `[` with no matching `]`.
    UnmatchedOpen,
    /// A `]` with no matching `[`.
    UnmatchedClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: FaultCode,
    /// Cell index at which the fault occurred. Meaningless for
    /// [`FaultCode::OutOfFuel`].
    pub position: usize,
}

/// Runtime fault discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum FaultCode {
    /// Ran to completion; the `0` an entry point returns on success.
    None = 0,
    /// Pointer moved left of cell 0.
    TapeUnderflow = 1,
    /// Pointer moved past the last cell.
    TapeOverflow = 2,
    /// The fuel budget was exhausted.
    OutOfFuel = 3,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "parse error: {e}"),
            Error::Runtime(e) => write!(f, "runtime error: {e}"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ParseErrorKind::UnmatchedOpen => {
                write!(f, "unclosed `[` at byte {}", self.span.start)
            }
            ParseErrorKind::UnmatchedClose => {
                write!(f, "unmatched `]` at byte {}", self.span.start)
            }
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            FaultCode::None => write!(f, "completed without fault"),

            FaultCode::TapeUnderflow => {
                write!(
                    f,
                    "pointer moved left of the tape (at cell {})",
                    self.position
                )
            }
            FaultCode::TapeOverflow => {
                write!(
                    f,
                    "pointer moved past the end of the tape (at cell {})",
                    self.position
                )
            }
            FaultCode::OutOfFuel => write!(f, "fuel exhausted"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Parse(e)
    }
}

impl From<RuntimeError> for Error {
    fn from(e: RuntimeError) -> Self {
        Error::Runtime(e)
    }
}

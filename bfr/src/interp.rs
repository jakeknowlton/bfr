//! A resumable, pull-based interpreter over [`crate::ir::Program`].

use crate::config::Config;
use crate::error::RuntimeError;
use crate::ir::{NodeId, Program};
use crate::tape::Tape;

/// Why control returned to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The budget ran out with the program still runnable.
    Ran,
    /// Stopped before a step whose node is in the breakpoint set.
    Breakpoint,
    /// Parked at a `read`: the input queue is empty and input has not been closed.
    NeedInput,
    /// The program ran off the end of its last node.
    Done,
    /// Faulted; the session is finished. The tape remains inspectable.
    Fault(RuntimeError),
}

/// A program in mid-execution.
pub struct Session {
    _private: (),
}

impl Session {
    /// Begin executing `program` under `config`'s dialect and fuel.
    pub fn new(program: Program, config: &Config) -> Session {
        todo!("Session::new")
    }

    /// Execute one step. Never returns [`Step::Ran`] or [`Step::Breakpoint`].
    pub fn step(&mut self) -> Step {
        todo!("Session::step")
    }

    /// Execute up to `budget` steps, returning early on anything that needs the caller.
    pub fn run(&mut self, budget: u64) -> Step {
        todo!("Session::run")
    }

    /// Like [`Session::run`], but also stops before executing a step
    /// whose node is in `breakpoints`, returning [`Step::Breakpoint`]. The
    /// first step of the call is exempt, so resuming from a breakpoint
    /// does not immediately re-hit it.
    pub fn run_until(&mut self, budget: u64, breakpoints: &[NodeId]) -> Step {
        todo!("Session::run_until")
    }

    /// Append bytes to the input queue.
    ///
    /// # Panics
    ///
    /// Panics if input has been closed.
    pub fn feed_input(&mut self, bytes: &[u8]) {
        todo!("Session::feed_input")
    }

    /// Mark input as permanently exhausted: any pending and every later
    /// `read` resolves per [`crate::config::EofBehavior`].
    pub fn close_input(&mut self) {
        todo!("Session::close_input")
    }

    /// Drain the output accumulated since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        todo!("Session::take_output")
    }

    /// The node about to execute.
    pub fn position(&self) -> Option<(NodeId, usize)> {
        todo!("Session::position")
    }

    pub fn tape(&self) -> &Tape {
        todo!("Session::tape")
    }

    /// Current cell index.
    pub fn ptr(&self) -> usize {
        todo!("Session::ptr")
    }

    /// Steps retired so far.
    pub fn steps(&self) -> u64 {
        todo!("Session::steps")
    }
}

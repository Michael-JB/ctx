pub mod tmux;
pub mod zellij;

use crate::multiplexer::MultiplexerError;

/// A failed multiplexer subprocess, kept structured so callers can match
/// on the tool's own error text before folding it into a message.
#[derive(Debug)]
pub(crate) struct CmdError {
    pub argv: Vec<String>,
    pub stderr: String,
}

impl From<CmdError> for MultiplexerError {
    fn from(err: CmdError) -> MultiplexerError {
        let detail = err.stderr.trim();
        let message = format!("command failed ({})", err.argv.join(" "));
        MultiplexerError(if detail.is_empty() {
            message
        } else {
            format!("{message}: {detail}")
        })
    }
}

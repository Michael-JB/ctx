use crate::git::GitError;
use crate::multiplexer::MultiplexerError;

/// A failure with a user-facing message; the CLI and TUI show it verbatim.
#[derive(Debug, thiserror::Error)]
pub enum CtxError {
    #[error("{0}")]
    Msg(String),
    #[error("{0}")]
    Git(#[from] GitError),
    #[error("{0}")]
    Mux(#[from] MultiplexerError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub fn msg<T>(message: impl Into<String>) -> Result<T> {
    Err(CtxError::Msg(message.into()))
}

pub type Result<T> = std::result::Result<T, CtxError>;

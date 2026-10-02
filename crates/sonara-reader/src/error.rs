//! Facade errors. Messages are written for a log or a protocol reply.
use crate::settings::Key;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Unknown engine or voice, or an engine that cannot be used.
    #[error(transparent)]
    Engine(#[from] sonara_engine::Error),
    /// A setting value of the wrong type or out of range.
    #[error("bad value for '{key}': {reason}")]
    BadValue { key: Key, reason: String },
    /// The registry has no engine to speak with.
    #[error("no engine is registered")]
    NoEngine,
    /// The reader was shut down.
    #[error("the reader is shut down")]
    Closed,
    /// The worker thread could not be started.
    #[error("could not start the reader: {0}")]
    Start(String),
}

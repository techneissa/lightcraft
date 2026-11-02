//! The crate's error type.

/// Why a LaMa call failed. Every failure is an error the caller can show; nothing panics.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The model file is missing, unreadable, not a LaMa checkpoint, or its tensors have the wrong shape.
    #[error("LaMa model: {0}")]
    Model(String),
    /// The picture or hole handed in can't be filled (wrong buffer length, empty, too large).
    #[error("LaMa input: {0}")]
    Input(String),
    /// The caller asked to stop.
    #[error("LaMa fill cancelled")]
    Cancelled,
    /// The tensor library failed (out of memory, a missing GPU op).
    #[error("LaMa compute: {0}")]
    Compute(String),
}

impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Self::Compute(e.to_string())
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

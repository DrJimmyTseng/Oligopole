use oligopole_core::CoreError;
use thiserror::Error;

/// Canonical error type for `oligopole-store` persistence and replay verification.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StoreError {
    #[error("core error: {0}")]
    Core(#[from] CoreError),

    #[error("sqlite error: {0}")]
    Sqlite(String),

    #[error("postcard serialization error: {0}")]
    Postcard(String),

    #[error("zstd compression error: {0}")]
    Zstd(String),

    #[error("missing world metadata or snapshot in .oligopole container")]
    MissingSnapshot,

    #[error("invalid blob length: expected {expected} bytes, got {actual}")]
    InvalidBlobLength { expected: usize, actual: usize },

    #[error("deterministic replay hash mismatch at tick {tick}: expected {expected_hex}, got {actual_hex}")]
    ReplayHashMismatch {
        tick: u64,
        expected_hex: String,
        actual_hex: String,
    },
}

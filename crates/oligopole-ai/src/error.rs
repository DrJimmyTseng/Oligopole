use oligopole_core::CoreError;
use oligopole_ledger::LedgerError;
use thiserror::Error;

/// Canonical error type for `oligopole-ai` candidate generation, persona sampling, and `Clef-Flash` IPC.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AiError {
    #[error("core simulation error: {0}")]
    Core(#[from] CoreError),

    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),

    #[error("empty candidate list for actor decision")]
    EmptyCandidates,

    #[error("msgpack serialization error: {0}")]
    MsgpackEncode(String),

    #[error("msgpack deserialization error: {0}")]
    MsgpackDecode(String),

    #[error("clef-flash worker I/O error: {0}")]
    WorkerIo(String),

    #[error("clef-flash protocol error: {0}")]
    ProtocolError(String),
}

use oligopole_core::PlayerCommand;
use serde::{Deserialize, Serialize};

/// Origin of a recorded command/directive in the `decision_outbox_journal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionSourceKind {
    PlayerManual = 1,
    ClefFlashSlowTick = 2,
    System0Heuristic = 3,
    AuthorityWebhook = 4,
    OracleFeed = 5,
}

impl DecisionSourceKind {
    pub const fn to_i64(self) -> i64 {
        self as i64
    }

    pub const fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::PlayerManual,
            2 => Self::ClefFlashSlowTick,
            4 => Self::AuthorityWebhook,
            5 => Self::OracleFeed,
            _ => Self::System0Heuristic,
        }
    }
}

/// Append-only Transactional Outbox entry recording a `PlayerCommand` or Slow-Tick `StrategicDirective`
/// at `tick` (executed at the start of `tick`) so any `.oligopole` save file can be deterministically
/// replayed and verified bit-for-bit from `world_seed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOutboxEntry {
    pub tick: u64,
    pub seq_no: u32,
    pub source_kind: DecisionSourceKind,
    pub command: PlayerCommand,
    pub probs_blake3_hash: [u8; 32],
}
